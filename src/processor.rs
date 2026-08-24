use anyhow::{anyhow, Result};
use image::{imageops::FilterType, DynamicImage, GenericImageView};

use crate::video::extract_video_frames;
pub use crate::video::{
	has_ffmpeg, is_video, probe_duration, probe_total_frames, ExtractMode, VideoExtractParams,
};

/// 取模扫描格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanFormat {
	/// 纵向LSB（按列扫描，每列8像素，最低位在上）
	VerticalLsb,
	/// 纵向MSB（按列扫描，每列8像素，最高位在上）
	VerticalMsb,
	/// 横向LSB（按行扫描，每行8像素，最低位在左）
	HorizontalLsb,
	/// 横向MSB（按行扫描，每行8像素，最高位在左）
	HorizontalMsb,
}

impl ScanFormat {
	/// 所有格式列表
	pub fn all() -> &'static [ScanFormat] {
		&[
			ScanFormat::VerticalLsb,
			ScanFormat::VerticalMsb,
			ScanFormat::HorizontalLsb,
			ScanFormat::HorizontalMsb,
		]
	}

	/// 格式名称
	pub fn name(&self) -> &'static str {
		match self {
			ScanFormat::VerticalLsb => "vertical_lsb",
			ScanFormat::VerticalMsb => "vertical_msb",
			ScanFormat::HorizontalLsb => "horizontal_lsb",
			ScanFormat::HorizontalMsb => "horizontal_msb",
		}
	}

	/// 是否按列扫描（高度方向存在未满字节）
	pub fn is_vertical(&self) -> bool {
		matches!(self, ScanFormat::VerticalLsb | ScanFormat::VerticalMsb)
	}
}

/// 缩放模式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScaleMode {
	/// 等比缩放（保持比例，居中留白）
	KeepAspect,
	/// 拉伸填充（不保持比例）
	Stretch,
	/// 等比裁剪（保持比例，裁剪超出部分）
	Crop,
}

impl ScaleMode {
	pub fn all() -> &'static [ScaleMode] {
		&[ScaleMode::KeepAspect, ScaleMode::Stretch, ScaleMode::Crop]
	}

	pub fn name(&self) -> &'static str {
		match self {
			ScaleMode::KeepAspect => "等比缩放",
			ScaleMode::Stretch => "拉伸填充",
			ScaleMode::Crop => "等比裁剪",
		}
	}
}

/// 处理参数
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessParams {
	/// 输出宽度
	pub width: u32,
	/// 输出高度
	pub height: u32,
	/// 二值化阈值 (0-255)
	pub threshold: u8,
	/// 扫描格式
	pub format: ScanFormat,
	/// 缩放模式
	pub scale_mode: ScaleMode,
	/// 是否反色（白变黑，黑变白）
	pub invert: bool,
	/// 缩放算法
	pub filter: FilterType,
	/// 字节内位序反转（每个字节内部 bit0↔bit7 颠倒）
	pub reverse_bits: bool,
	/// 未满8位的末尾字节，空闲位填 1（默认填 0，打开后兼容老取模工具）
	pub pad_unused_bits_1: bool,
}

impl Default for ProcessParams {
	fn default() -> Self {
		Self {
			width: 128,
			height: 64,
			threshold: 128,
			format: ScanFormat::VerticalLsb,
			scale_mode: ScaleMode::KeepAspect,
			invert: false,
			filter: FilterType::Lanczos3,
			reverse_bits: false,
			pad_unused_bits_1: false,
		}
	}
}

/// 单帧图像数据（二值化后的像素矩阵）
/// pixels按行存储：pixels[y * width + x] = true 表示前景（亮）
#[derive(Debug, Clone)]
pub struct BinaryFrame {
	pub width: u32,
	pub height: u32,
	pub pixels: Vec<bool>,
}

impl BinaryFrame {
	/// 创建全黑帧
	pub fn new(width: u32, height: u32) -> Self {
		Self {
			width,
			height,
			pixels: vec![false; (width * height) as usize],
		}
	}

	/// 设置像素值
	pub fn set_pixel(&mut self, x: u32, y: u32, v: bool) {
		if x >= self.width || y >= self.height {
			return;
		}
		self.pixels[(y as usize) * (self.width as usize) + (x as usize)] = v;
	}

	/// 获取像素值
	pub fn get_pixel(&self, x: u32, y: u32) -> bool {
		if x >= self.width || y >= self.height {
			return false;
		}
		self.pixels[(y as usize) * (self.width as usize) + (x as usize)]
	}

	/// 计算单帧字节数（根据扫描格式）
	pub fn frame_size(&self, format: ScanFormat) -> usize {
		match format {
			ScanFormat::VerticalLsb | ScanFormat::VerticalMsb => {
				// 按列，每8行1字节
				let bytes_per_col = ((self.height + 7) / 8) as usize;
				(self.width as usize) * bytes_per_col
			}
			ScanFormat::HorizontalLsb | ScanFormat::HorizontalMsb => {
				// 按行，每8列1字节
				let bytes_per_row = ((self.width + 7) / 8) as usize;
				(self.height as usize) * bytes_per_row
			}
		}
	}

	/// 按指定格式取模，返回字节数组（可选字节内位序反转、可选填1空闲位）
	pub fn to_bytes(
		&self,
		format: ScanFormat,
		reverse_bits: bool,
		pad_unused_bits_1: bool,
	) -> Vec<u8> {
		let mut result = Vec::with_capacity(self.frame_size(format));

		let pad_mask_high = |mut byte: u8, from_bit: u8| -> u8 {
			if pad_unused_bits_1 {
				for i in from_bit..=7 {
					byte |= 1u8 << i;
				}
			}
			byte
		};
		let pad_mask_low = |mut byte: u8, to_bit: u8| -> u8 {
			if pad_unused_bits_1 {
				for i in 0..=to_bit {
					byte |= 1u8 << i;
				}
			}
			byte
		};

		match format {
			ScanFormat::VerticalLsb => {
				// 页优先（SSD1306 页面寻址模式）：先输出一页的所有列（每页=8行），
				// 再输出下一页。每字节按列方向拼 8 行，LSB 对应最上面一行。
				let pages = (self.height + 7) / 8;
				let rem = self.height % 8;
				for p in 0..pages {
					for c in 0..self.width {
						let mut byte = 0u8;
						for b in 0..8u8 {
							let row = (p as u32) * 8 + (b as u32);
							if row < self.height && self.get_pixel(c, row) {
								byte |= 1u8 << b;
							}
						}
						if rem != 0 && p == pages - 1 {
							// 最后一页未满：有效位 bit0..rem-1，空闲高位 bit..=7 可选填 1
							byte = pad_mask_high(byte, rem as u8);
						}
						result.push(byte);
					}
				}
			}
			ScanFormat::VerticalMsb => {
				// 页优先（SSD1306 页面寻址模式）：先输出一页的所有列，再下一页。
				// 每字节 MSB 对应最上面一行。
				let pages = (self.height + 7) / 8;
				let rem = self.height % 8;
				for p in 0..pages {
					for c in 0..self.width {
						let mut byte = 0u8;
						for b in 0..8u8 {
							let row = (p as u32) * 8 + (b as u32);
							let bit = 7u8 - b;
							if row < self.height && self.get_pixel(c, row) {
								byte |= 1u8 << bit;
							}
						}
						if rem != 0 && p == pages - 1 {
							// 最后一页未满：高位从 MSB(bit7) 起写了 rem 行，
							// 空闲低位是 bit (7-rem+1)..=0，可选填 1。
							let first_empty = (8u8 - rem as u8) as u8;
							byte = pad_mask_low(byte, first_empty);
						}
						result.push(byte);
					}
				}
			}
			ScanFormat::HorizontalLsb => {
				// 按行扫描，每行从左到右，每8像素为1字节，LSB对应最左边的像素
				for row in 0..self.height {
					let mut byte = 0u8;
					let mut bit = 0u8;
					for col in 0..self.width {
						if self.get_pixel(col, row) {
							byte |= 1 << bit;
						}
						bit += 1;
						if bit == 8 {
							result.push(byte);
							byte = 0;
							bit = 0;
						}
					}
					if bit > 0 {
						// bit = width%8，空闲高位 bit..=7 可选填 1
						result.push(pad_mask_high(byte, bit));
					}
				}
			}
			ScanFormat::HorizontalMsb => {
				// 按行扫描，每行从左到右，每8像素为1字节，MSB对应最左边的像素
				for row in 0..self.height {
					let mut byte = 0u8;
					let mut bit = 7i8;
					for col in 0..self.width {
						if bit >= 0 && self.get_pixel(col, row) {
							byte |= 1 << bit;
						}
						bit -= 1;
						if bit < 0 {
							result.push(byte);
							byte = 0;
							bit = 7;
						}
					}
					if bit < 7 {
						// 空闲低位 bit..=0 可选填 1
						result.push(pad_mask_low(byte, bit as u8));
					}
				}
			}
		}

		if reverse_bits {
			// 查表法反转每个字节内位序（快）
			#[rustfmt::skip]
			const REV: [u8; 16] = [
				0x0, 0x8, 0x4, 0xC, 0x2, 0xA, 0x6, 0xE,
				0x1, 0x9, 0x5, 0xD, 0x3, 0xB, 0x7, 0xF,
			];
			for b in &mut result {
				*b = (REV[(*b & 0x0F) as usize] << 4) | REV[((*b >> 4) & 0x0F) as usize];
			}
		}

		result
	}

	/// 转换为egui可显示的颜色纹理（RGBA）
	pub fn to_rgba8(&self) -> Vec<u8> {
		let mut rgba = Vec::with_capacity((self.width * self.height * 4) as usize);
		for y in 0..self.height {
			for x in 0..self.width {
				if self.get_pixel(x, y) {
					rgba.extend_from_slice(&[0xFF, 0xFF, 0xFF, 0xFF]); // 白
				} else {
					rgba.extend_from_slice(&[0x00, 0x00, 0x00, 0xFF]); // 黑
				}
			}
		}
		rgba
	}
}

/// 计算等比缩放后的尺寸和偏移
pub fn calculate_scaled_size(
	src_w: u32,
	src_h: u32,
	dst_w: u32,
	dst_h: u32,
	mode: ScaleMode,
) -> (u32, u32, i32, i32) {
	match mode {
		ScaleMode::Stretch => (dst_w, dst_h, 0, 0),
		ScaleMode::KeepAspect => {
			let scale_w = dst_w as f64 / src_w as f64;
			let scale_h = dst_h as f64 / src_h as f64;
			let scale = scale_w.min(scale_h);
			let new_w = ((src_w as f64 * scale) as u32).max(1);
			let new_h = ((src_h as f64 * scale) as u32).max(1);
			let offset_x = (dst_w as i32 - new_w as i32) / 2;
			let offset_y = (dst_h as i32 - new_h as i32) / 2;
			(new_w, new_h, offset_x, offset_y)
		}
		ScaleMode::Crop => {
			let scale_w = dst_w as f64 / src_w as f64;
			let scale_h = dst_h as f64 / src_h as f64;
			let scale = scale_w.max(scale_h);
			let new_w = ((src_w as f64 * scale) as u32).max(1);
			let new_h = ((src_h as f64 * scale) as u32).max(1);
			let offset_x = (dst_w as i32 - new_w as i32) / 2;
			let offset_y = (dst_h as i32 - new_h as i32) / 2;
			(new_w, new_h, offset_x, offset_y)
		}
	}
}

/// 处理单张图像为二值帧
pub fn process_image(img: &DynamicImage, params: &ProcessParams) -> Result<BinaryFrame> {
	let (src_w, src_h) = img.dimensions();
	if src_w == 0 || src_h == 0 {
		return Err(anyhow!("图像尺寸为0"));
	}

	let (scaled_w, scaled_h, offset_x, offset_y) =
		calculate_scaled_size(src_w, src_h, params.width, params.height, params.scale_mode);

	// 缩放图像
	let resized = img.resize_exact(scaled_w, scaled_h, params.filter);
	let gray = resized.to_luma8();

	// 创建目标帧（初始化为全黑/背景）
	let mut pixels = vec![false; (params.width * params.height) as usize];

	// 将缩放后的图像粘贴到目标画布，进行二值化
	for y in 0..scaled_h {
		let dst_y = y as i32 + offset_y;
		if dst_y < 0 || dst_y >= params.height as i32 {
			continue;
		}
		for x in 0..scaled_w {
			let dst_x = x as i32 + offset_x;
			if dst_x < 0 || dst_x >= params.width as i32 {
				continue;
			}
			let luma = gray.get_pixel(x, y)[0];
			let mut is_foreground = if params.invert {
				luma < params.threshold
			} else {
				luma >= params.threshold
			};
			// Crop模式下超出部分不绘制
			if matches!(params.scale_mode, ScaleMode::Crop) {
				if dst_x < 0
					|| dst_y < 0 || dst_x >= params.width as i32
					|| dst_y >= params.height as i32
				{
					is_foreground = false;
				}
			}
			let idx = (dst_y as usize) * (params.width as usize) + (dst_x as usize);
			if idx < pixels.len() {
				pixels[idx] = is_foreground;
			}
		}
	}

	Ok(BinaryFrame {
		width: params.width,
		height: params.height,
		pixels,
	})
}

/// 加载静态图片
pub fn load_image(path: &std::path::Path) -> Result<Vec<DynamicImage>> {
	let img = image::open(path)?;
	Ok(vec![img])
}

/// 加载GIF动图（返回每一帧）
pub fn load_gif(path: &std::path::Path) -> Result<Vec<DynamicImage>> {
	use image::codecs::gif::GifDecoder;
	use image::AnimationDecoder;
	use std::fs::File;
	use std::io::BufReader;

	let file = File::open(path)?;
	let reader = BufReader::new(file);
	let decoder = GifDecoder::new(reader)?;
	let frames = decoder.into_frames();
	let mut result = Vec::new();
	for frame in frames {
		let frame = frame?;
		let buffer = frame.into_buffer();
		result.push(DynamicImage::ImageRgba8(buffer));
	}
	if result.is_empty() {
		return Err(anyhow!("GIF中没有帧"));
	}
	Ok(result)
}

/// 根据文件扩展名加载，自动识别类型
/// 加载任意媒体（图片/GIF/视频），返回帧序列
pub fn load_media(path: &std::path::Path) -> Result<Vec<DynamicImage>> {
	let ext = path
		.extension()
		.and_then(|s| s.to_str())
		.unwrap_or("")
		.to_lowercase();

	// 视频：走 ffmpeg，使用默认抽帧参数
	if is_video(path) {
		let params = VideoExtractParams::default();
		return extract_video_frames(path, &params);
	}

	match ext.as_str() {
		"gif" => load_gif(path),
		_ => load_image(path),
	}
}

/// 使用自定义帧区间加载媒体（视频/GIF/长图序列）
/// 视频：按 VideoExtractParams 抽帧
/// 其他多帧媒体（GIF 等）：按帧区间截取
pub fn load_media_with_range(
	path: &std::path::Path,
	video_params: Option<&VideoExtractParams>,
) -> Result<Vec<DynamicImage>> {
	let ext = path
		.extension()
		.and_then(|s| s.to_str())
		.unwrap_or("")
		.to_lowercase();

	if is_video(path) {
		let params = video_params.cloned().unwrap_or_default();
		return extract_video_frames(path, &params);
	}

	// GIF / 单图
	let mut frames = match ext.as_str() {
		"gif" => load_gif(path)?,
		_ => load_image(path)?,
	};

	// 对于多帧（GIF），按 start_frame / end_frame 截取
	if let Some(vp) = video_params {
		if frames.len() > 1 {
			let total = frames.len() as u32;
			let s = vp.start_frame.unwrap_or(0).min(total.saturating_sub(1)) as usize;
			let e = vp
				.end_frame
				.unwrap_or(total.saturating_sub(1))
				.min(total.saturating_sub(1)) as usize;
			let (s, e) = if s <= e { (s, e) } else { (e, s) };
			if s == 0 && e as usize + 1 == frames.len() {
				// 未裁剪
			} else {
				let keep: Vec<DynamicImage> = frames.drain(s..=e).collect();
				frames = keep;
			}

			// 再按抽帧模式进行二次采样
			match vp.mode {
				ExtractMode::Fps(_) => {
					// 对 GIF 不额外再做 fps 处理（按原始帧时序即可）
				}
				ExtractMode::EveryN(n) if n > 1 => {
					let n = n as usize;
					let mut kept = Vec::new();
					for (i, f) in frames.into_iter().enumerate() {
						if i % n == 0 {
							kept.push(f);
						}
					}
					frames = kept;
				}
				ExtractMode::TotalFrames(t) => {
					let t = t as usize;
					if frames.len() > t && t > 0 {
						let orig = frames;
						let n = orig.len() as u32;
						let take = t as u32;
						let mut sampled = Vec::with_capacity(t);
						for i in 0..take {
							let idx = ((i as u64 * n as u64) / take as u64) as usize;
							sampled.push(orig[idx.min(orig.len() - 1)].clone());
						}
						frames = sampled;
					}
				}
				_ => {}
			}
		}
	}

	Ok(frames)
}
