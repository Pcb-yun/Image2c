// Windows 下不显示终端窗口
#![windows_subsystem = "windows"]

pub mod exporter;
pub mod importer;
pub mod processor;
pub mod video;

use eframe::egui;
use egui::{ColorImage, TextureHandle, TextureOptions};
use exporter::*;
use image::{DynamicImage, GenericImageView, RgbaImage};
use importer::parse_c_header;
use processor::*;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use video::{is_video, probe_duration, probe_total_frames, ExtractMode, VideoExtractParams};

fn main() -> Result<(), eframe::Error> {
	let options = eframe::NativeOptions {
		viewport: egui::ViewportBuilder::default()
			.with_inner_size([1100.0, 720.0])
			.with_min_inner_size([900.0, 600.0]),
		..Default::default()
	};

	eframe::run_native(
		"Img2C - 单色屏幕取模工具",
		options,
		Box::new(|cc| {
			// 安装中文字体
			install_chinese_fonts(&cc.egui_ctx);
			Ok(Box::new(App::default()))
		}),
	)
}

/// 加载系统中文字体（优先微软雅黑，找不到再试其他）
fn install_chinese_fonts(ctx: &egui::Context) {
	use egui::FontDefinitions;
	use std::fs::File;
	use std::io::Read;

	// Windows 常用中文字体路径（按优先级排序）
	let font_candidates: &[&str] = &[
		r"C:\Windows\Fonts\msyh.ttc",   // 微软雅黑
		r"C:\Windows\Fonts\msyhbd.ttc", // 微软雅黑粗体
		r"C:\Windows\Fonts\simhei.ttf", // 黑体
		r"C:\Windows\Fonts\simsun.ttc", // 宋体
		r"C:\Windows\Fonts\Deng.ttf",   // 等线
		r"C:\Windows\Fonts\msyh.ttf",
	];

	let mut fonts = FontDefinitions::default();
	let mut loaded = false;

	for path in font_candidates {
		if let Ok(mut file) = File::open(path) {
			let mut buf = Vec::new();
			if file.read_to_end(&mut buf).is_ok() {
				let name = format!("chinese_{}", loaded);
				fonts
					.font_data
					.insert(name.clone(), egui::FontData::from_owned(buf));
				fonts
					.families
					.entry(egui::FontFamily::Proportional)
					.or_default()
					.insert(0, name.clone());
				fonts
					.families
					.entry(egui::FontFamily::Monospace)
					.or_default()
					.push(name);
				loaded = true;
				// 微软雅黑足够，加载一个就好
				if path.contains("msyh") {
					break;
				}
			}
		}
	}

	if loaded {
		ctx.set_fonts(fonts);
	}
}

struct App {
	params: ProcessParams,
	last_applied_params: Option<ProcessParams>,
	export_cfg: ExportConfig,
	source_images: Vec<DynamicImage>,
	source_textures: Vec<TextureHandle>,
	source_path: Option<PathBuf>,
	source_is_video: bool,
	source_is_header: bool,
	source_total_frames: Option<u32>,
	source_duration_s: Option<f64>,
	video_params: VideoExtractParams,
	processed_frames: Vec<BinaryFrame>,
	processed_textures: Vec<TextureHandle>,
	preview_idx: usize,
	last_frame_change: Instant,
	status_msg: String,
	error_msg: Option<String>,
	aspect_lock: bool,
	keep_aspect_ratio: Option<f32>,
	show_original_window: bool,
}

impl Default for App {
	fn default() -> Self {
		Self {
			params: ProcessParams::default(),
			last_applied_params: None,
			export_cfg: ExportConfig::default(),
			source_images: Vec::new(),
			source_textures: Vec::new(),
			source_path: None,
			source_is_video: false,
			source_is_header: false,
			source_total_frames: None,
			source_duration_s: None,
			video_params: VideoExtractParams::default(),
			processed_frames: Vec::new(),
			processed_textures: Vec::new(),
			preview_idx: 0,
			last_frame_change: Instant::now(),
			status_msg: "请打开图片/GIF/视频/头文件".to_string(),
			error_msg: None,
			aspect_lock: true,
			keep_aspect_ratio: None,
			show_original_window: false,
		}
	}
}

impl App {
	/// 入口：打开任意文件
	fn load_file(&mut self, ctx: &egui::Context, path: PathBuf) {
		let ext = path
			.extension()
			.and_then(|s| s.to_str())
			.unwrap_or("")
			.to_lowercase();
		if ext == "h" {
			self.load_header_file(ctx, path);
		} else {
			self.load_media_file(ctx, path);
		}
	}

	/// 从C头文件反解回帧并显示预览
	fn load_header_file(&mut self, ctx: &egui::Context, path: PathBuf) {
		self.error_msg = None;
		self.source_is_header = true;
		self.source_is_video = false;
		self.source_total_frames = None;
		self.source_duration_s = None;
		match parse_c_header(&path) {
			Ok(parsed) => {
				let count = parsed.frames.len();
				let w = parsed.frames[0].width;
				let h = parsed.frames[0].height;
				// 同步参数：宽高/格式/反转
				self.params.width = w;
				self.params.height = h;
				self.params.format = parsed.format;
				self.params.reverse_bits = parsed.reverse_bits;
				self.keep_aspect_ratio = Some(w as f32 / h as f32);
				// 把解析出来的帧当做源（动态图像）= 同一幅二值图
				// 由于 BinaryFrame -> DynamicImage 需要转换
				self.source_images.clear();
				self.source_textures.clear();
				self.processed_frames.clear();
				self.processed_textures.clear();
				for (i, frame) in parsed.frames.iter().enumerate() {
					let rgba = frame.to_rgba8();
					let img = DynamicImage::ImageRgba8(
						RgbaImage::from_raw(w, h, rgba).expect("Rgba尺寸不匹配"),
					);
					self.source_images.push(img);
					// 原图像纹理（同转换后）
					let color_img = ColorImage::from_rgba_unmultiplied(
						[w as usize, h as usize],
						&frame.to_rgba8(),
					);
					let src_tex = ctx.load_texture(
						format!("source_{}", i),
						color_img.clone(),
						TextureOptions::LINEAR,
					);
					let dst_tex = ctx.load_texture(
						format!("processed_{}", i),
						color_img,
						TextureOptions::NEAREST,
					);
					self.source_textures.push(src_tex);
					self.processed_textures.push(dst_tex);
					self.processed_frames.push(frame.clone());
				}
				// 导出名称用头文件内的名字
				self.export_cfg.name = parsed.name.clone();
				self.source_path = Some(path.clone());
				self.preview_idx = 0;
				let fs = self.processed_frames[0].frame_size(self.params.format);
				self.status_msg = format!(
					"已读入头文件 {:?}，共{}帧，单帧{}字节，格式：{}{}",
					path.file_name().unwrap_or_default(),
					count,
					fs,
					self.params.format.name(),
					if self.params.reverse_bits {
						" + 位序反转"
					} else {
						""
					}
				);
				// 标记参数已应用（否则下一帧会再次process_all，导致二值化覆盖，这里source_images是合成的RGBA但process_image也能跑，问题不大，但避免多跑）
				self.last_applied_params = Some(self.params.clone());
			}
			Err(e) => {
				self.error_msg = Some(format!("头文件解析失败: {}", e));
				self.status_msg = "头文件读取失败".to_string();
			}
		}
	}

	/// 加载媒体文件（自动根据范围参数）
	fn load_media_file(&mut self, ctx: &egui::Context, path: PathBuf) {
		self.source_is_header = false;
		self.error_msg = None;
		let is_vid = is_video(&path);
		self.source_is_video = is_vid;

		// 视频：先探测原始信息
		if is_vid {
			self.source_total_frames = probe_total_frames(&path);
			self.source_duration_s = probe_duration(&path);
			// 首次加载：重置范围为全量
			self.video_params.start_frame = None;
			self.video_params.end_frame = None;
			self.video_params.start_time = None;
			self.video_params.end_time = None;
		} else {
			self.source_total_frames = None;
			self.source_duration_s = None;
		}

		match load_media_with_range(
			&path,
			if is_vid {
				Some(&self.video_params)
			} else {
				None
			},
		) {
			Ok(imgs) => {
				self.source_path = Some(path.clone());
				self.keep_aspect_ratio = if !imgs.is_empty() {
					let (w, h) = imgs[0].dimensions();
					Some(w as f32 / h as f32)
				} else {
					None
				};
				self.source_images = imgs;
				let count = self.source_images.len();
				self.status_msg = format!(
					"已加载 {} 帧 ({:?}) {}",
					count,
					path.file_name().unwrap_or_default(),
					if is_vid { "[视频]" } else { "" }
				);
				self.preview_idx = 0;
				// 生成原图预览纹理
				self.source_textures.clear();
				for (i, img) in self.source_images.iter().enumerate() {
					let rgba = img.to_rgba8();
					let (w, h) = rgba.dimensions();
					let color_img =
						ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
					let tex = ctx.load_texture(
						format!("source_{}", i),
						color_img,
						TextureOptions::LINEAR,
					);
					self.source_textures.push(tex);
				}
				self.process_all(ctx);
			}
			Err(e) => {
				self.error_msg = Some(format!("加载失败: {}", e));
				self.status_msg = "加载失败".to_string();
			}
		}
	}

	/// 重新以当前范围参数抽取帧（视频/GIF 多帧）
	fn reload_with_range(&mut self, ctx: &egui::Context) {
		let Some(path) = self.source_path.clone() else {
			return;
		};
		self.error_msg = None;

		let is_vid = self.source_is_video;
		let params_ref = if is_vid {
			Some(&self.video_params)
		} else if self.source_images.len() > 1 {
			Some(&self.video_params)
		} else {
			None
		};

		match load_media_with_range(&path, params_ref) {
			Ok(imgs) => {
				self.keep_aspect_ratio = if !imgs.is_empty() {
					let (w, h) = imgs[0].dimensions();
					Some(w as f32 / h as f32)
				} else {
					None
				};
				self.source_images = imgs;
				let count = self.source_images.len();
				self.status_msg = format!(
					"重新抽取完成，共 {} 帧 （模式: {}）",
					count,
					self.video_params.mode_name()
				);
				self.preview_idx = 0;
				self.source_textures.clear();
				for (i, img) in self.source_images.iter().enumerate() {
					let rgba = img.to_rgba8();
					let (w, h) = rgba.dimensions();
					let color_img =
						ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
					let tex = ctx.load_texture(
						format!("source_{}", i),
						color_img,
						TextureOptions::LINEAR,
					);
					self.source_textures.push(tex);
				}
				self.process_all(ctx);
			}
			Err(e) => {
				self.error_msg = Some(format!("重新抽帧失败: {}", e));
				self.status_msg = "抽帧失败".to_string();
			}
		}
	}

	/// 处理所有帧
	fn process_all(&mut self, ctx: &egui::Context) {
		if self.source_images.is_empty() {
			return;
		}
		self.processed_frames.clear();
		self.processed_textures.clear();

		for img in &self.source_images {
			match process_image(img, &self.params) {
				Ok(frame) => {
					let rgba = frame.to_rgba8();
					let color_img = ColorImage::from_rgba_unmultiplied(
						[frame.width as usize, frame.height as usize],
						&rgba,
					);
					let tex = ctx.load_texture(
						format!("processed_{}", self.processed_frames.len()),
						color_img,
						TextureOptions::NEAREST,
					);
					self.processed_textures.push(tex);
					self.processed_frames.push(frame);
				}
				Err(e) => {
					self.error_msg = Some(format!("处理失败: {}", e));
					break;
				}
			}
		}

		if !self.processed_frames.is_empty() {
			let fs = self.processed_frames[0].frame_size(self.params.format);
			let total = fs * self.processed_frames.len();
			self.status_msg = format!(
				"完成: {} 帧, 单帧 {} 字节, 总 {} 字节",
				self.processed_frames.len(),
				fs,
				total
			);
		}
		// 记录已应用的参数
		self.last_applied_params = Some(self.params.clone());
	}

	/// 导出为C头文件
	fn export_file(&mut self) {
		if self.processed_frames.is_empty() {
			self.error_msg = Some("没有可导出的数据".to_string());
			return;
		}
		let default_name = self
			.source_path
			.as_ref()
			.and_then(|p| p.file_stem().and_then(|s| s.to_str()))
			.unwrap_or("img")
			.to_string();

		// 如果导出名为默认，自动用源文件名
		if self.export_cfg.name == "img" || self.export_cfg.name.is_empty() {
			self.export_cfg.name = default_name;
		}

		let suggested_name = format!("{}.h", self.export_cfg.name);
		if let Some(path) = rfd::FileDialog::new()
			.add_filter("C头文件", &["h"])
			.set_file_name(&suggested_name)
			.save_file()
		{
			let content = export_c_header(
				&self.processed_frames,
				self.params.format,
				self.params.reverse_bits,
				self.params.pad_unused_bits_1,
				&self.export_cfg,
			);
			match std::fs::write(&path, content) {
				Ok(_) => {
					self.status_msg = format!("已导出: {:?}", path);
				}
				Err(e) => {
					self.error_msg = Some(format!("导出失败: {}", e));
				}
			}
		}
	}

	/// 复制到剪贴板（C代码）
	fn copy_to_clipboard(&mut self, ctx: &egui::Context) {
		if self.processed_frames.is_empty() {
			self.error_msg = Some("没有可复制的数据".to_string());
			return;
		}
		let content = export_c_header(
			&self.processed_frames,
			self.params.format,
			self.params.reverse_bits,
			self.params.pad_unused_bits_1,
			&self.export_cfg,
		);
		ctx.output_mut(|o| o.copied_text = content);
		self.status_msg = "已复制到剪贴板".to_string();
	}

	/// 应用等比缩放约束
	fn apply_aspect_lock(&mut self, changed_width: bool) {
		if !self.aspect_lock {
			return;
		}
		let ratio = if let Some(r) = self.keep_aspect_ratio {
			r
		} else if !self.source_images.is_empty() {
			let (w, h) = self.source_images[0].dimensions();
			let r = w as f32 / h as f32;
			self.keep_aspect_ratio = Some(r);
			r
		} else {
			return;
		};

		if changed_width {
			let new_h = (self.params.width as f32 / ratio).round() as u32;
			self.params.height = new_h.max(1);
		} else {
			let new_w = (self.params.height as f32 * ratio).round() as u32;
			self.params.width = new_w.max(1);
		}
	}
}

impl eframe::App for App {
	fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
		// 动画帧自动切换
		if self.processed_textures.len() > 1 {
			let now = Instant::now();
			if now.duration_since(self.last_frame_change) > Duration::from_millis(100) {
				self.preview_idx = (self.preview_idx + 1) % self.processed_textures.len();
				self.last_frame_change = now;
				ctx.request_repaint();
			}
		}

		egui::TopBottomPanel::top("menu").show(ctx, |ui| {
			ui.horizontal(|ui| {
				if ui.button("📂 打开文件").clicked() {
					if let Some(path) = rfd::FileDialog::new()
						.add_filter(
							"所有支持",
							&[
								"h", "png", "jpg", "jpeg", "bmp", "gif", "tiff", "webp", "mp4",
								"avi", "mov", "mkv", "wmv", "flv", "webm", "m4v", "mpg", "mpeg",
								"ts", "3gp",
							],
						)
						.add_filter("C头文件（读入预览）", &["h"])
						.add_filter(
							"图片",
							&["png", "jpg", "jpeg", "bmp", "gif", "tiff", "webp"],
						)
						.add_filter(
							"视频（需ffmpeg）",
							&[
								"mp4", "avi", "mov", "mkv", "wmv", "flv", "webm", "m4v", "mpg",
								"mpeg", "ts", "3gp",
							],
						)
						.pick_file()
					{
						self.load_file(ctx, path);
					}
				}

				ui.separator();

				if ui
					.add_enabled(
						!self.processed_frames.is_empty(),
						egui::Button::new("💾 导出C头文件"),
					)
					.clicked()
				{
					self.export_file();
				}

				if ui
					.add_enabled(
						!self.processed_frames.is_empty(),
						egui::Button::new("📋 复制到剪贴板"),
					)
					.clicked()
				{
					self.copy_to_clipboard(ctx);
				}

				ui.separator();

				ui.label(egui::RichText::new(&self.status_msg).weak());

				if let Some(err) = &self.error_msg {
					ui.separator();
					ui.colored_label(egui::Color32::RED, format!("⚠ {}", err));
					if ui.small_button("清除").clicked() {
						self.error_msg = None;
					}
				}
			});
		});

		egui::SidePanel::left("params_panel")
			.default_width(320.0)
			.show(ctx, |ui| {
				ui.add_space(8.0);
				ui.heading("参数设置");
				ui.separator();

				// 宽高设置
				ui.label(egui::RichText::new("输出尺寸").strong());
				ui.horizontal(|ui| {
					ui.label("宽:");
					let old_w = self.params.width;
					ui.add(egui::DragValue::new(&mut self.params.width).range(1..=4096).speed(1.0));
					if self.aspect_lock && old_w != self.params.width {
						self.apply_aspect_lock(true);
					}
				});
				ui.horizontal(|ui| {
					ui.label("高:");
					let old_h = self.params.height;
					ui.add(egui::DragValue::new(&mut self.params.height).range(1..=4096).speed(1.0));
					if self.aspect_lock && old_h != self.params.height {
						self.apply_aspect_lock(false);
					}
				});
				ui.horizontal(|ui| {
					ui.checkbox(&mut self.aspect_lock, "锁定宽高比");
					if ui.button("重置比例").on_hover_text("重新从原图计算比例").clicked() {
						self.keep_aspect_ratio = None;
						if !self.source_images.is_empty() {
							let (w, h) = self.source_images[0].dimensions();
							self.params.width = w;
							self.params.height = h;
							self.keep_aspect_ratio = Some(w as f32 / h as f32);
						}
					}
				});

				// 常用尺寸快捷按钮
				ui.add_space(4.0);
				ui.label("常用尺寸:");
				ui.horizontal_wrapped(|ui| {
					let presets = [
						("128x64", 128u32, 64u32),
						("128x32", 128, 32),
						("64x48", 64, 48),
						("160x80", 160, 80),
						("240x135", 240, 135),
						("96x96", 96, 96),
					];
					for (name, w, h) in presets {
						if ui.small_button(name).clicked() {
							self.params.width = w;
							self.params.height = h;
							self.keep_aspect_ratio = None;
							self.aspect_lock = false;
						}
					}
				});

				// 常用屏幕驱动器快捷配置
				ui.add_space(10.0);
				ui.heading("常用驱动器");
				ui.separator();
				ui.label("选择驱动器后自动套用 尺寸/取模格式/位序反转:");

				// (显示名称, 宽, 高, 扫描格式, 字节反转, 反色, 说明)
				let drivers: &[(&str, u32, u32, ScanFormat, bool, bool, &str)] = &[
					("SSD1306 / SSD1309 128×64",    128, 64,  ScanFormat::VerticalLsb,   false, false, "通用12864 OLED"),
					("SSD1306 128×32",              128, 32,  ScanFormat::VerticalLsb,   false, false, "窄款0.91\" OLED"),
					("SSD1306 64×48",                64, 48,  ScanFormat::VerticalLsb,   false, false, "小尺寸 OLED"),
					("SH1106 128×64",               128, 64,  ScanFormat::VerticalLsb,   false, false, "兼容SSD1306"),
					("SH1106 132×64",               132, 64,  ScanFormat::VerticalLsb,   false, false, "1.3\" OLED原生"),
					("SSD1315 128×64",              128, 64,  ScanFormat::VerticalLsb,   false, false, "同SSD1306"),
					("ST7567 / ST7565 128×64",      128, 64,  ScanFormat::VerticalLsb,   false, false, "LCD 12864 并/串"),
					("UC1701 128×64",               128, 64,  ScanFormat::VerticalLsb,   false, false, "LCD 12864"),
					("PCD8544 (Nokia5110) 84×48",    84, 48,  ScanFormat::VerticalMsb,   false, false, "诺基亚屏 MSB在上"),
					("ST7920 128×64 (串/并)",       128, 64,  ScanFormat::HorizontalMsb, false, false, "字库型12864"),
					("SSD1327 (灰阶单色) 128×128",  128, 128, ScanFormat::HorizontalMsb, false, false, "水平写入"),
					("WS0010 / OLED 128×64",       128, 64,  ScanFormat::HorizontalMsb, false, false, "字库型 OLED"),
					("SSD1675A / 墨水屏 2.13\"",    250, 122, ScanFormat::HorizontalMsb, false, false, "水平扫MSB"),
					("SSD1322 256×64 (灰阶)",       256, 64,  ScanFormat::HorizontalMsb, false, false, "0.95\"-2.8\" 黄蓝OLED"),
				];
				let mut cur_idx: usize = 0;
				let selected_name = format!(
					"{}x{} / {}{}",
					self.params.width, self.params.height,
					self.params.format.name(),
					if self.params.reverse_bits { "+rev" } else { "" }
				);
				egui::ComboBox::new("driver_preset", "")
					.selected_text(&selected_name)
					.show_ui(ui, |ui| {
						for (i, (name, w, h, fmt, rev, inv, _hint)) in drivers.iter().enumerate() {
							let mut bits = Vec::new();
							bits.push(fmt.name().to_string());
							if *rev { bits.push("bit反序".to_string()); }
							bits.push(if *inv { "反色".to_string() } else { "不反色".to_string() });
							let label = format!("{}   [{}]", name, bits.join(", "));
							let same = self.params.width == *w
								&& self.params.height == *h
								&& self.params.format == *fmt
								&& self.params.reverse_bits == *rev
								&& self.params.invert == *inv;
							let res = ui.selectable_label(same, label);
							if res.clicked() {
								cur_idx = i;
								self.params.width = *w;
								self.params.height = *h;
								self.params.format = *fmt;
								self.params.reverse_bits = *rev;
								self.params.invert = *inv;
								self.keep_aspect_ratio = None;
								self.aspect_lock = false;
								self.status_msg = format!("已套用驱动器预设: {}", name);
							}
						}
					});
				// 显示当前格式提示
				let hint = drivers.iter().find(|(_n, w, h, f, r, i, _)| {
					self.params.width == *w && self.params.height == *h
						&& self.params.format == *f && self.params.reverse_bits == *r
						&& self.params.invert == *i
				}).map(|(_, _, _, _, _, _, h)| *h);
				if let Some(h) = hint {
					ui.label(egui::RichText::new(format!("💡 {}", h)).small().weak());
				} else {
					ui.label(egui::RichText::new("💡 若未列出可在下方手动调参数").small().weak());
				}

				ui.add_space(12.0);
				ui.heading("帧区间 / 视频抽帧");
				ui.separator();

				let multi_frame = !self.source_images.is_empty() && self.source_images.len() > 1;
				if !self.source_is_video && !multi_frame {
					ui.label(egui::RichText::new("当前为单张图片，无需设置帧区间").weak());
				} else {
					// 源信息展示
					ui.horizontal(|ui| {
						let mut info = String::new();
						if self.source_is_video {
							info.push_str("🎬 视频");
							if let Some(n) = self.source_total_frames {
								info.push_str(&format!(" 原始: {}帧", n));
							}
							if let Some(d) = self.source_duration_s {
								info.push_str(&format!(" 时长: {:.1}s", d));
							}
						} else if multi_frame {
							info.push_str(&format!("🖼️ GIF 原始: {}帧", self.source_images.len()));
						}
						if !info.is_empty() {
							ui.label(egui::RichText::new(info).small());
						}
					});

					ui.add_space(4.0);
					ui.label("起始帧号（0开始，留空=从头）:");
					let mut sf_enable = self.video_params.start_frame.is_some();
					let mut sf_val = self.video_params.start_frame.unwrap_or(0);
					ui.horizontal(|ui| {
						ui.checkbox(&mut sf_enable, "启用");
						ui.add_enabled(sf_enable, egui::DragValue::new(&mut sf_val).range(0..=1_000_000).speed(1.0));
					});
					self.video_params.start_frame = if sf_enable { Some(sf_val) } else { None };

					ui.add_space(2.0);
					ui.label("结束帧号（包含，留空=到尾）:");
					let mut ef_enable = self.video_params.end_frame.is_some();
					let mut ef_val = self.video_params.end_frame.unwrap_or(0);
					ui.horizontal(|ui| {
						ui.checkbox(&mut ef_enable, "启用");
						ui.add_enabled(ef_enable, egui::DragValue::new(&mut ef_val).range(0..=1_000_000).speed(1.0));
					});
					self.video_params.end_frame = if ef_enable { Some(ef_val) } else { None };

					// 视频专用：时间区间
					if self.source_is_video {
						ui.add_space(4.0);
						ui.label("起始时间(秒)（与帧号同时设置时优先帧号）:");
						let mut st_enable = self.video_params.start_time.is_some();
						let mut st_val = self.video_params.start_time.unwrap_or(0.0);
						ui.horizontal(|ui| {
							ui.checkbox(&mut st_enable, "启用");
							ui.add_enabled(st_enable, egui::DragValue::new(&mut st_val).range(0.0..=86400.0).speed(0.1).prefix("s "));
						});
						self.video_params.start_time = if st_enable { Some(st_val) } else { None };

						ui.label("结束时间(秒):");
						let mut et_enable = self.video_params.end_time.is_some();
						let mut et_val = self.video_params.end_time.unwrap_or(0.0);
						ui.horizontal(|ui| {
							ui.checkbox(&mut et_enable, "启用");
							ui.add_enabled(et_enable, egui::DragValue::new(&mut et_val).range(0.0..=86400.0).speed(0.1).prefix("s "));
						});
						self.video_params.end_time = if et_enable { Some(et_val) } else { None };
					}

					// 抽帧模式
					ui.add_space(6.0);
					ui.label("抽帧模式:");
					let mut cur_mode = self.video_params.mode;
					let (fps_v, every_v, total_v) = match cur_mode {
						ExtractMode::Fps(v) => (v, 3u32, 50u32),
						ExtractMode::EveryN(n) => (10.0f64, n, 50u32),
						ExtractMode::TotalFrames(t) => (10.0f64, 3u32, t),
					};
					let mode_label = |m: &ExtractMode| match m {
						ExtractMode::Fps(_) => "按 FPS 抽取",
						ExtractMode::EveryN(_) => "每 N 帧抽 1",
						ExtractMode::TotalFrames(_) => "均匀抽总帧数",
					};
					egui::ComboBox::new("extract_mode", "")
						.selected_text(mode_label(&cur_mode))
						.show_ui(ui, |ui| {
							if ui.selectable_label(matches!(cur_mode, ExtractMode::Fps(_)), "按 FPS 抽取").clicked() {
								cur_mode = ExtractMode::Fps(fps_v);
							}
							if ui.selectable_label(matches!(cur_mode, ExtractMode::EveryN(_)), "每 N 帧抽 1").clicked() {
								cur_mode = ExtractMode::EveryN(every_v);
							}
							if ui.selectable_label(matches!(cur_mode, ExtractMode::TotalFrames(_)), "均匀抽总帧数").clicked() {
								cur_mode = ExtractMode::TotalFrames(total_v);
							}
						});

					match &mut cur_mode {
					ExtractMode::Fps(v) => {
						ui.add(egui::Slider::new(v, 0.1..=120.0).prefix("FPS: "));
					}
					ExtractMode::EveryN(n) => {
						ui.add(egui::Slider::new(n, 1..=1000).prefix("每隔: ").suffix(" 帧抽一帧"));
					}
					ExtractMode::TotalFrames(t) => {
						ui.add(egui::Slider::new(t, 1..=10000).prefix("共抽: ").suffix(" 帧"));
					}
				}
					self.video_params.mode = cur_mode;

					ui.add_space(6.0);
					if self.source_is_header {
						ui.add_enabled(false, egui::Button::new("🔄 按当前范围重新抽帧"))
							.on_disabled_hover_text("头文件来源：已无原始媒体，无法重新抽帧");
						ui.label(egui::RichText::new("（当前从头文件导入，仅预览/导出）").weak().small());
					} else {
						if ui.button("🔄 按当前范围重新抽帧").on_hover_text("视频：调用ffmpeg重新抽帧；GIF：按帧号裁剪").clicked() {
							self.reload_with_range(ctx);
						}
					}
				}

				ui.add_space(12.0);
				ui.heading("图像处理");
				ui.separator();

				// 缩放模式
				ui.label("缩放模式:");
				egui::ComboBox::new("scale_mode", "")
					.selected_text(self.params.scale_mode.name())
					.show_ui(ui, |ui| {
						for mode in ScaleMode::all() {
							ui.selectable_value(&mut self.params.scale_mode, *mode, mode.name());
						}
					});

				// 二值化阈值
				ui.add_space(6.0);
				ui.label(format!("二值化阈值: {}", self.params.threshold));
				ui.add(egui::Slider::new(&mut self.params.threshold, 0..=255));

				// 反色
				ui.checkbox(&mut self.params.invert, "反色（黑底白字 ↔ 白底黑字）");
				// 字节内位序反转
				ui.checkbox(&mut self.params.reverse_bits, "字节内位序反转（bit0↔bit7）");
				ui.checkbox(&mut self.params.pad_unused_bits_1, "未满8位填1（老工具兼容）")
					.on_hover_text("宽/高不是8的倍数时，最后一字节的空闲位默认填0；勾上则填1。\n用于和某些老取模工具输出完全一致");
				if self.params.width % 8 != 0 || self.params.height % 8 != 0 {
					ui.label(egui::RichText::new(format!(
						"⚠ 宽{}高{}，{}方向存在未满字节，此选项生效",
						self.params.width, self.params.height,
						if self.params.format.is_vertical() { "高度" } else { "宽度" }
					)).small().color(egui::Color32::from_rgb(0xD0, 0xA0, 0x30)));
				}

				// 扫描格式
				ui.add_space(6.0);
				ui.label("扫描取模格式:");
				egui::ComboBox::new("scan_format", "")
					.selected_text(self.params.format.name())
					.show_ui(ui, |ui| {
						for fmt in ScanFormat::all() {
							let label = match fmt {
								ScanFormat::VerticalLsb => "vertical_lsb (按列, LSB上)",
								ScanFormat::VerticalMsb => "vertical_msb (按列, MSB上)",
								ScanFormat::HorizontalLsb => "horizontal_lsb (按行, LSB左)",
								ScanFormat::HorizontalMsb => "horizontal_msb (按行, MSB左)",
							};
							ui.selectable_value(&mut self.params.format, *fmt, label);
						}
					});

				// 缩放算法
				ui.add_space(6.0);
				ui.label("缩放算法:");
				egui::ComboBox::new("filter", "")
					.selected_text(match self.params.filter {
						image::imageops::FilterType::Nearest => "Nearest (最快)",
						image::imageops::FilterType::Triangle => "Triangle",
						image::imageops::FilterType::CatmullRom => "CatmullRom",
						image::imageops::FilterType::Gaussian => "Gaussian",
						image::imageops::FilterType::Lanczos3 => "Lanczos3 (质量最好)",
					})
					.show_ui(ui, |ui| {
						use image::imageops::FilterType::*;
						ui.selectable_value(&mut self.params.filter, Nearest, "Nearest (最快, 像素风)");
						ui.selectable_value(&mut self.params.filter, Triangle, "Triangle (线性)");
						ui.selectable_value(&mut self.params.filter, CatmullRom, "CatmullRom");
						ui.selectable_value(&mut self.params.filter, Gaussian, "Gaussian");
						ui.selectable_value(&mut self.params.filter, Lanczos3, "Lanczos3 (质量最好)");
					});

				ui.add_space(8.0);
				if ui.button("🔄 重新处理").clicked() {
					self.process_all(ctx);
				}

				ui.separator();
				ui.add_space(8.0);
				ui.heading("导出设置");
				ui.separator();

				ui.horizontal(|ui| {
					ui.label("数组名称:");
					ui.text_edit_singleline(&mut self.export_cfg.name);
				});
				ui.checkbox(&mut self.export_cfg.use_conditional, "使用条件编译宏 (IMG2C_XXX)");
				ui.horizontal(|ui| {
					ui.label("每行字节数:");
					ui.add(egui::DragValue::new(&mut self.export_cfg.bytes_per_line).range(4..=64));
				});

				ui.add_space(12.0);
				ui.separator();
				// 帧统计
				if !self.processed_frames.is_empty() {
					let fs = self.processed_frames[0].frame_size(self.params.format);
					let total = fs * self.processed_frames.len();
					ui.label(egui::RichText::new("📊 帧信息").strong());
					ui.label(format!("帧数: {}", self.processed_frames.len()));
					ui.label(format!("单帧字节: {}", fs));
					ui.label(format!("总字节数: {} ({} KB)", total, total / 1024));
				}
			});

		egui::CentralPanel::default().show(ctx, |ui| {
			ui.vertical_centered(|ui| {
				ui.heading("预览（原图 ↔ 转换后对比）");

				if self.processed_textures.is_empty() || self.source_textures.is_empty() {
					ui.add_space(80.0);
					ui.label(
						egui::RichText::new("未加载图像\n点击「打开文件」选择图片或GIF")
							.size(18.0)
							.weak(),
					);
				} else {
					// 帧选择器（多帧）
					if self.processed_textures.len() > 1 {
						ui.horizontal(|ui| {
							ui.label(format!(
								"帧: {}/{}",
								self.preview_idx + 1,
								self.processed_textures.len()
							));
							if ui.small_button("⏮").clicked() {
								self.preview_idx = 0;
							}
							if ui.small_button("◀").clicked() {
								self.preview_idx = if self.preview_idx == 0 {
									self.processed_textures.len() - 1
								} else {
									self.preview_idx - 1
								};
							}
							if ui.small_button("▶").clicked() {
								self.preview_idx =
									(self.preview_idx + 1) % self.processed_textures.len();
							}
							if ui.small_button("⏭").clicked() {
								self.preview_idx = self.processed_textures.len() - 1;
							}
							ui.separator();
							ui.add(egui::Slider::new(
								&mut self.preview_idx,
								0..=self.processed_textures.len() - 1,
							));
						});
						ui.add_space(6.0);
					}

					// 水平并排显示：原图 + 转换图
					let idx = self
						.preview_idx
						.min(self.source_textures.len().saturating_sub(1));
					let src_tex = &self.source_textures[idx];
					let dst_tex = &self.processed_textures[idx];
					let src_size = src_tex.size_vec2();
					let dst_size = dst_tex.size_vec2();

					// 计算自适应缩放（完全填充可用空间，不限制最小/最大倍率）
					let avail = ui.available_size();
					// 预留标题、标签等空间
					let avail_w = (avail.x - 60.0).max(200.0);
					let avail_h = (avail.y - 70.0).max(150.0);
					let half_w = avail_w * 0.5;

					// 各自按自己的半区宽高自适应（保持图像比例）
					let src_scale_w = half_w / src_size.x;
					let src_scale_h = avail_h / src_size.y;
					let src_scale = src_scale_w.min(src_scale_h);

					let dst_scale_w = half_w / dst_size.x;
					let dst_scale_h = avail_h / dst_size.y;
					let dst_scale = dst_scale_w.min(dst_scale_h);

					let src_disp = src_size * src_scale;
					let dst_disp = dst_size * dst_scale;

					ui.add_space(10.0);
					ui.horizontal(|ui| {
						ui.add_space(10.0);
						// 原图列
						ui.vertical(|ui| {
							ui.label(egui::RichText::new("📷 原图").strong());
							ui.add_space(4.0);
							let (sw, sh) = self
								.source_images
								.get(idx)
								.map(|i| i.dimensions())
								.unwrap_or((0, 0));
							ui.add(
								egui::Image::from_texture(src_tex)
									.fit_to_exact_size(src_disp)
									.sense(egui::Sense::hover()),
							);
							ui.add_space(4.0);
							ui.label(
								egui::RichText::new(format!(
									"尺寸: {}×{}  |  缩放: {:.1}x",
									sw, sh, src_scale
								))
								.weak()
								.small(),
							);
						});

						ui.separator();

						// 转换图列
						ui.vertical(|ui| {
							ui.horizontal(|ui| {
								ui.label(egui::RichText::new("🎯 转换后").strong());
								// 当缩放比例不为1时，提供原始分辨率查看按钮
								if (dst_scale - 1.0).abs() > 0.01 {
									if ui.small_button("🔍 以原始分辨率查看").clicked() {
										self.show_original_window = true;
									}
								}
							});
							ui.add_space(4.0);
							ui.add(
								egui::Image::from_texture(dst_tex)
									.fit_to_exact_size(dst_disp)
									.sense(egui::Sense::hover()),
							);
							ui.add_space(4.0);
							ui.label(
								egui::RichText::new(format!(
									"尺寸: {}×{}  |  缩放: {:.1}x  |  {}",
									self.params.width,
									self.params.height,
									dst_scale,
									self.params.format.name()
								))
								.weak()
								.small(),
							);
						});
					}); // 闭合 ui.horizontal
				} // 闭合 else
			}); // 闭合 vertical_centered
		}); // 闭合 CentralPanel

		// 参数变更自动重新处理预览
		let needs_reprocess = !self.source_images.is_empty()
			&& match &self.last_applied_params {
				Some(last) => last != &self.params,
				None => true,
			};
		if needs_reprocess {
			self.process_all(ctx);
			ctx.request_repaint();
		}

		// 原始分辨率预览窗口
		if self.show_original_window && !self.processed_textures.is_empty() {
			let idx = self
				.preview_idx
				.min(self.processed_textures.len().saturating_sub(1));
			let tex = self.processed_textures[idx].clone();
			let img_size = tex.size_vec2();
			let w = self.params.width;
			let h = self.params.height;
			let fmt = self.params.format.name();
			let frame_idx = idx;
			let frame_count = self.processed_textures.len();
			let open = &mut self.show_original_window;

			ctx.show_viewport_immediate(
				egui::ViewportId::from_hash_of("original_preview"),
				egui::ViewportBuilder::default()
					.with_title(format!("原始分辨率预览 ({}×{})", w, h))
					.with_inner_size([img_size.x + 20.0, img_size.y + 60.0])
					.with_resizable(true),
				|ctx, class| {
					// 不支持独立窗口时退化为嵌入显示
					if class == egui::ViewportClass::Embedded {
						return;
					}
					egui::CentralPanel::default().show(ctx, |ui| {
						ui.vertical(|ui| {
							ui.horizontal(|ui| {
								ui.label(format!(
									"帧 {}/{} | {} | {}×{}",
									frame_idx + 1,
									frame_count,
									fmt,
									w,
									h
								));
								ui.separator();
								if ui.button("关闭").clicked() {
									*open = false;
								}
							});
							ui.separator();
							egui::ScrollArea::both().show(ui, |ui| {
								ui.add(egui::Image::from_texture(&tex).fit_to_exact_size(img_size));
							});
						});
					});
				},
			);
		}
	}
}
