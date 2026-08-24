use anyhow::{bail, Result};
use image::DynamicImage;
use std::path::{Path, PathBuf};
use std::process::Command;
use walkdir::WalkDir;

/// 视频取模/抽帧参数
#[derive(Debug, Clone)]
pub struct VideoExtractParams {
	/// 起始时间（秒），None 表示从开头
	pub start_time: Option<f64>,
	/// 结束时间（秒），None 表示到结尾
	pub end_time: Option<f64>,
	/// 起始帧索引（优先于start_time，同时设置时用帧号）
	pub start_frame: Option<u32>,
	/// 结束帧索引（包含）
	pub end_frame: Option<u32>,
	/// 抽帧方式
	pub mode: ExtractMode,
}

/// 抽帧模式
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ExtractMode {
	/// 按固定 FPS 抽取（比如每秒 5 帧）
	Fps(f64),
	/// 每隔 N 帧抽 1 帧
	EveryN(u32),
	/// 只抽固定总帧数（均分时间段）
	TotalFrames(u32),
}

impl Default for VideoExtractParams {
	fn default() -> Self {
		Self {
			start_time: None,
			end_time: None,
			start_frame: None,
			end_frame: None,
			mode: ExtractMode::Fps(10.0),
		}
	}
}

impl VideoExtractParams {
	pub fn mode_name(&self) -> String {
		match self.mode {
			ExtractMode::Fps(v) => format!("{} FPS", v),
			ExtractMode::EveryN(v) => format!("每{}帧抽1", v),
			ExtractMode::TotalFrames(v) => format!("共{}帧", v),
		}
	}
}

/// 视频文件扩展名列表
pub fn video_extensions() -> &'static [&'static str] {
	&[
		"mp4", "avi", "mov", "mkv", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts", "3gp",
	]
}

/// 判断路径是否为视频
pub fn is_video(path: &Path) -> bool {
	path.extension()
		.and_then(|s| s.to_str())
		.map(|s| video_extensions().iter().any(|v| v.eq_ignore_ascii_case(s)))
		.unwrap_or(false)
}

/// 检测 ffmpeg 是否可用
pub fn has_ffmpeg() -> bool {
	Command::new("ffmpeg")
		.arg("-version")
		.output()
		.map(|o| o.status.success())
		.unwrap_or(false)
}

/// 查找视频总帧数（通过 ffprobe 或 ffmpeg），失败返回 None
pub fn probe_total_frames(path: &Path) -> Option<u32> {
	// 先尝试 ffprobe
	if let Ok(out) = Command::new("ffprobe")
		.args([
			"-v",
			"error",
			"-select_streams",
			"v:0",
			"-count_frames",
			"-show_entries",
			"stream=nb_read_frames",
			"-of",
			"csv=p=0",
		])
		.arg(path)
		.output()
	{
		if out.status.success() {
			let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
			if let Ok(n) = s.parse::<u32>() {
				return Some(n);
			}
		}
	}
	// 再尝试 ffmpeg 计数
	if let Ok(out) = Command::new("ffmpeg")
		.args(["-i"])
		.arg(path)
		.args(["-map", "0:v:0", "-c", "copy", "-f", "null", "-"])
		.output()
	{
		let stderr = String::from_utf8_lossy(&out.stderr).to_string();
		// 找 "frame= 1234"
		for line in stderr.lines().rev() {
			if let Some(rest) = line.strip_prefix("frame=") {
				let trimmed = rest.trim().split_whitespace().next().unwrap_or("");
				if let Ok(n) = trimmed.parse::<u32>() {
					return Some(n);
				}
			}
		}
	}
	None
}

/// 查找视频时长（秒），失败返回 None
pub fn probe_duration(path: &Path) -> Option<f64> {
	if let Ok(out) = Command::new("ffprobe")
		.args([
			"-v",
			"error",
			"-show_entries",
			"format=duration",
			"-of",
			"csv=p=0",
		])
		.arg(path)
		.output()
	{
		if out.status.success() {
			let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
			if let Ok(d) = s.parse::<f64>() {
				return Some(d);
			}
		}
	}
	// ffprobe 不可用时退化为解析 ffmpeg -i stderr
	if let Ok(out) = Command::new("ffmpeg")
		.args(["-hide_banner", "-i"])
		.arg(path)
		.output()
	{
		let s = String::from_utf8_lossy(&out.stderr).to_string();
		for line in s.lines() {
			let line = line.trim_start();
			if let Some(rest) = line.strip_prefix("Duration:") {
				let part = rest.split(',').next().unwrap_or("").trim();
				// HH:MM:SS.xx
				let segs: Vec<&str> = part.split(':').collect();
				if segs.len() == 3 {
					let h: f64 = segs[0].parse().unwrap_or(0.0);
					let m: f64 = segs[1].parse().unwrap_or(0.0);
					let s: f64 = segs[2].parse().unwrap_or(0.0);
					return Some(h * 3600.0 + m * 60.0 + s);
				}
			}
		}
	}
	None
}

/// 从视频抽帧到临时目录，返回图片路径列表
pub fn extract_video_frames(path: &Path, params: &VideoExtractParams) -> Result<Vec<DynamicImage>> {
	if !has_ffmpeg() {
		bail!("未检测到 ffmpeg，请先安装 ffmpeg 并添加到 PATH。\n下载: https://www.gyan.dev/ffmpeg/builds/");
	}

	let tmpdir = tempfile::tempdir()?;
	let out_pattern = tmpdir.path().join("frame_%06d.png");

	let mut cmd = Command::new("ffmpeg");
	cmd.arg("-y")
		.arg("-hide_banner")
		.arg("-loglevel")
		.arg("error");

	// 输入起点（优先帧号，其次时间）
	if let Some(_sf) = params.start_frame {
		// ffmpeg 的 -vf select=gte(n\,sf) 在输入后
	} else if let Some(st) = params.start_time {
		cmd.arg("-ss").arg(format!("{:.3}", st));
	}
	cmd.arg("-i").arg(path);

	// 输出结束点
	if let Some(et) = params.end_time {
		let dur = match params.start_time {
			Some(s) => (et - s).max(0.0),
			None => et,
		};
		cmd.arg("-t").arg(format!("{:.3}", dur));
	}

	// 构建视频滤镜
	let mut filters: Vec<String> = Vec::new();
	// 帧区间（如果用 start_frame / end_frame 限制）
	match (params.start_frame, params.end_frame) {
		(Some(s), Some(e)) if e >= s => {
			filters.push(format!(
				"select=between(n\\,{}\\,{})+gte(n\\,{})*lte(n\\,{})",
				s, e, s, e
			));
			filters.push("setpts=N/FRAME_RATE/TB".to_string());
		}
		(Some(s), None) => {
			filters.push(format!("select=gte(n\\,{})", s));
			filters.push("setpts=N/FRAME_RATE/TB".to_string());
		}
		(None, Some(e)) => {
			filters.push(format!("select=lte(n\\,{})", e));
		}
		_ => {}
	}

	// 抽帧模式
	match params.mode {
		ExtractMode::Fps(fps) => {
			filters.push(format!("fps={}", fps));
		}
		ExtractMode::EveryN(n) => {
			if n > 1 {
				filters.push(format!("select=not(mod(n\\,{}))", n));
				filters.push("setpts=N/FRAME_RATE/TB".to_string());
			}
		}
		ExtractMode::TotalFrames(_total) => {
			// 先知道总帧数/时长后均匀采样最简单的方式：用 fps=total/duration
			// 这里我们退化为先用 select=not(mod(n\,ceil(total_frames/total)))
			// 为保证结果，先抽取后再截断？这里采用近似策略：将 fps 设为极高并 truncate
			// 但我们不知道总帧数，先用 fps 模式把所有帧抽出，后面再在加载时均匀截断
			// 实际：先按高 fps 抽然后等距采样在后续进行
		}
	}

	if !filters.is_empty() {
		cmd.arg("-vf").arg(filters.join(","));
	}
	cmd.arg(out_pattern);

	let output = cmd.output()?;
	if !output.status.success() {
		let err = String::from_utf8_lossy(&output.stderr);
		bail!("ffmpeg 抽帧失败: {}", err);
	}

	// 收集 PNG 帧（按文件名排序）
	let mut frames_png: Vec<PathBuf> = WalkDir::new(tmpdir.path())
		.into_iter()
		.filter_map(|e| e.ok())
		.filter(|e| e.file_type().is_file())
		.filter(|e| {
			e.path()
				.extension()
				.and_then(|s| s.to_str())
				.map(|s| s.eq_ignore_ascii_case("png"))
				.unwrap_or(false)
		})
		.map(|e| e.path().to_path_buf())
		.collect();
	frames_png.sort();

	// TotalFrames 模式：等距采样
	if let ExtractMode::TotalFrames(total) = params.mode {
		if frames_png.len() > total as usize {
			let orig = std::mem::take(&mut frames_png);
			let n = orig.len() as u32;
			let take = total.min(n);
			let mut sampled = Vec::with_capacity(take as usize);
			for i in 0..take {
				let idx = ((i as u64 * n as u64) / take as u64) as usize;
				sampled.push(orig[idx.min(orig.len() - 1)].clone());
			}
			frames_png = sampled;
		}
	}

	if frames_png.is_empty() {
		bail!("ffmpeg 没有抽到任何帧，请检查视频和参数");
	}

	// 解码为 DynamicImage
	let mut imgs = Vec::new();
	for p in frames_png {
		let img = image::open(&p)?;
		imgs.push(img);
	}

	// tmpdir 在这里 drop 自动清理
	Ok(imgs)
}
