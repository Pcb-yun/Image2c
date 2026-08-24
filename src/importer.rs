use anyhow::{anyhow, bail, Result};
use std::collections::BTreeMap;
use std::path::Path;

use crate::processor::{BinaryFrame, ScanFormat};

/// 解析结果
pub struct ParsedHeader {
	pub name: String,
	pub format: ScanFormat,
	pub reverse_bits: bool,
	pub frames: Vec<BinaryFrame>,
}

/// 读取并解析C头文件
pub fn parse_c_header(path: &Path) -> Result<ParsedHeader> {
	let text = std::fs::read_to_string(path).map_err(|e| anyhow!("读取文件失败: {}", e))?;
	parse_header_text(&text)
}

/// 找到前缀并填入对应值
fn collect_prefix_maps(text: &str) -> BTreeMap<String, (String, [Option<u64>; 4])> {
	// key: 前缀小写，value: (原始前缀, [width, height, frame_size, frame_count])
	let mut map: BTreeMap<String, (String, [Option<u64>; 4])> = BTreeMap::new();

	let re_w = regex::Regex::new(r"#define\s+(\w+)_WIDTH\s+(\d+)").unwrap();
	let re_h = regex::Regex::new(r"#define\s+(\w+)_HEIGHT\s+(\d+)").unwrap();
	let re_s = regex::Regex::new(r"#define\s+(\w+)_FRAME_SIZE\s+(\d+)").unwrap();
	let re_c = regex::Regex::new(r"#define\s+(\w+)_FRAME_COUNT\s+(\d+)").unwrap();
	let patterns: [(&regex::Regex, usize); 4] = [(&re_w, 0), (&re_h, 1), (&re_s, 2), (&re_c, 3)];

	for (re, idx) in patterns.iter() {
		for cap in re.captures_iter(text) {
			let prefix = &cap[1];
			let val = cap[2].parse::<u64>().ok();
			let key = prefix.to_ascii_lowercase();
			let entry = map
				.entry(key)
				.or_insert_with(|| (prefix.to_string(), [None, None, None, None]));
			// 如果前缀大小写不同，保留第一次遇到的原始形式作为主名称
			entry.1[*idx] = val;
		}
	}

	map
}

/// 从头文件里找所有的 `TYPE NAME_data[D1][D2] = {` 声明。NAME_data 前缀用 _data 截尾。
/// 返回列表：(数组声明起始偏移在文本中的位置, 原始前缀名, 可能的d1, d2)
fn find_2d_arrays(text: &str) -> Vec<(usize, String, Option<u64>, Option<u64>)> {
	let mut out = Vec::new();

	// 匹配 uint8_t / unsigned char / const uint8_t / static const uint8_t / char / u8
	// 变量名后 [D1 or 宏名 or 空] [D2 or 宏名 or 空] = {
	let re = regex::RegexBuilder::new(
		r#"(?i)(?:uint8_t|unsigned\s+char|char|u8)\s+(?:\s*\*\s*)?([A-Za-z_]\w*)\s*\[\s*([A-Za-z_]\w*|\d*)\s*\]\s*\[\s*([A-Za-z_]\w*|\d*)\s*\]\s*="#
	).build().unwrap();
	for cap in re.captures_iter(text) {
		let ident = &cap[1];
		// 要求名字以 _data 结尾，或者名字里不带后缀就直接把整个名字当前缀
		let prefix = if let Some(p) = ident.strip_suffix("_data") {
			p.to_string()
		} else {
			// 不以 _data 结尾的二维数组也接受，用原名当前缀
			ident.to_string()
		};
		let d1 = cap.get(2).and_then(|m| m.as_str().parse::<u64>().ok());
		let d2 = cap.get(3).and_then(|m| m.as_str().parse::<u64>().ok());
		out.push((cap.get(0).unwrap().start(), prefix, d1, d2));
	}
	// 按起始偏移升序排列
	out.sort_by_key(|(off, _, _, _)| *off);
	out
}

/// 在 text 中从某个位置开始找第一个 `{`，并从该 `{` 起按层级扫描字节
/// 返回 (收集到的字节数组, 每帧起始偏移列表)
fn scan_bytes_from(text: &str, start_offset: usize) -> (Vec<u8>, Vec<usize>) {
	let data_slice = &text[start_offset..];
	let first_open = match data_slice.find('{') {
		Some(i) => i,
		None => return (Vec::new(), Vec::new()),
	};
	let data = &data_slice[first_open..];

	let mut bytes_total: Vec<u8> = Vec::new();
	let mut frame_starts: Vec<usize> = Vec::new();
	let mut brace_depth = 0i32;
	let mut in_hex = String::new();
	let mut in_token = false;

	for ch in data.chars() {
		match ch {
			'{' => {
				brace_depth += 1;
				if brace_depth == 2 {
					frame_starts.push(bytes_total.len());
				}
			}
			'}' => {
				if in_token {
					if let Ok(b) = parse_byte_token(&in_hex) {
						bytes_total.push(b);
					}
					in_hex.clear();
					in_token = false;
				}
				brace_depth -= 1;
				if brace_depth <= 0 {
					break;
				}
			}
			c if c.is_ascii_hexdigit() || c == 'x' || c == 'X' => {
				in_token = true;
				in_hex.push(c);
			}
			',' | ' ' | '\t' | '\r' | '\n' | ';' => {
				if in_token {
					if let Ok(b) = parse_byte_token(&in_hex) {
						bytes_total.push(b);
					}
					in_hex.clear();
					in_token = false;
				}
			}
			_ => {
				if in_token {
					if let Ok(b) = parse_byte_token(&in_hex) {
						bytes_total.push(b);
					}
					in_hex.clear();
					in_token = false;
				}
			}
		}
	}

	// 如果没有找到任何 二级 brace（一维数组 {{...}} 的另一种写法可能只是 {...}），
	// 就视为单帧，从 offset 0 开始
	if frame_starts.is_empty() && !bytes_total.is_empty() {
		frame_starts.push(0);
	}

	(bytes_total, frame_starts)
}

fn parse_byte_token(tok: &str) -> Result<u8> {
	let clean: String = tok.chars().filter(|c| c.is_ascii_hexdigit()).collect();
	if clean.is_empty() {
		bail!("空token");
	}
	let val = u64::from_str_radix(&clean, 16)
		.or_else(|_| clean.parse::<u64>().map_err(|e| anyhow!("{}", e)))?;
	if val > 0xFF {
		bail!("字节值过大: {}", val);
	}
	Ok(val as u8)
}

/// 从文本解析头文件
pub fn parse_header_text(text: &str) -> Result<ParsedHeader> {
	// 1. 收集所有 *_WIDTH / *_HEIGHT / *_FRAME_SIZE / *_FRAME_COUNT 宏
	let prefixes = collect_prefix_maps(text);
	if prefixes.is_empty() {
		bail!("找不到任何 *_WIDTH / *_HEIGHT / *_FRAME_SIZE / *_FRAME_COUNT 宏");
	}

	// 2. 选出"最佳前缀"：拥有 WIDTH 和 HEIGHT 两者齐全的；如有多个挑信息最多者
	let mut best: Option<(String, [Option<u64>; 4])> = None;
	for (_key, (orig_pref, vals)) in prefixes.iter() {
		let filled = vals.iter().filter(|v| v.is_some()).count();
		let has_size = vals[0].is_some() && vals[1].is_some();
		let score = filled + if has_size { 4 } else { 0 };
		match &best {
			None => {
				best = Some((orig_pref.clone(), *vals));
			}
			Some((cur_name, cur_vals)) => {
				let cur_filled = cur_vals.iter().filter(|v| v.is_some()).count();
				let cur_has_size = cur_vals[0].is_some() && cur_vals[1].is_some();
				let cur_score = cur_filled + if cur_has_size { 4 } else { 0 };
				if score > cur_score {
					best = Some((orig_pref.clone(), *vals));
				} else if score == cur_score && cur_name != orig_pref {
					// 平手时按字典序稳定选择（保持第一个遇到的）
				}
			}
		}
	}
	let (name_pref, vals) = best.expect("已检查非空");
	let width = vals[0].ok_or_else(|| anyhow!("缺少 *_WIDTH 定义"))? as u32;
	let height = vals[1].ok_or_else(|| anyhow!("缺少 *_HEIGHT 定义"))? as u32;
	let frame_size = vals[2].map(|x| x as usize);
	let frame_count = vals[3].map(|x| x as usize);

	// 3. 解析 ScanFormat
	let mut format = ScanFormat::VerticalLsb;
	let re_fmt = regex::Regex::new(r"\*\s*格式:\s*([A-Za-z0-9_]+)").unwrap();
	if let Some(c) = re_fmt.captures(text) {
		match &c[1] {
			"vertical_lsb" => format = ScanFormat::VerticalLsb,
			"vertical_msb" => format = ScanFormat::VerticalMsb,
			"horizontal_lsb" => format = ScanFormat::HorizontalLsb,
			"horizontal_msb" => format = ScanFormat::HorizontalMsb,
			_ => {}
		}
	}

	// 4. reverse_bits
	let reverse_bits = text.contains("字节内位序: 已反转")
		|| text.contains("bit-reversed")
		|| text.contains("字节内位序反转: 是")
		|| text.contains("reverse_bits: 1");

	// 5. 找出所有二维数组声明
	let arrays = find_2d_arrays(text);

	// 计算期望总字节数（FRAME_SIZE * FRAME_COUNT，若缺失则通过宽高格式估计）
	let mut expected_frame_size: usize = frame_size.unwrap_or(0);
	if expected_frame_size == 0 {
		expected_frame_size = compute_frame_size(width, height, format);
	}
	let expected_total: usize = match (frame_size, frame_count) {
		(Some(s), Some(c)) => s * c,
		(Some(s), None) => s, // 没有帧数时假设至少1帧
		(None, Some(c)) => compute_frame_size(width, height, format) * c,
		(None, None) => compute_frame_size(width, height, format),
	};
	let expected_frames: usize = frame_count.unwrap_or(1);

	// 若注释没给格式且 frame_size 明确，先按字节数反推最可能格式
	if format == ScanFormat::VerticalLsb {
		// 如果 frame_size 存在，先根据它判断是否 horizontal 更合理
		if let Some(fs) = frame_size {
			format = guess_format(width, height, fs);
			expected_frame_size = fs;
		}
	}

	// 6. 数组声明匹配：
	//    优先顺序：
	//    A. 前缀（忽略大小写）与 name_pref 完全匹配或匹配 (name_pref + _data)
	//    B. 数组维度大小乘积 = expected_total
	//    C. 第一个 uint8_t 二维数组 + 扫描字节数能对得上 expected_total
	let mut chosen: Option<(usize, String, usize, usize)> = None; // (offset, 前缀名, frame_size, frame_count)
															   // 构造一个小写比较的前缀
	let name_pref_lc = name_pref.to_ascii_lowercase();
	let name_data_lc = format!("{}_data", name_pref_lc);

	for (off, prefix, d1, d2) in arrays.iter() {
		let prefix_lc = prefix.to_ascii_lowercase();
		// 候选名（如果前缀为 MyData，则实际数组名是 MyData_data 还是 MyData[][]都接受）
		let matched = prefix_lc == name_pref_lc
			|| format!("{}_data", prefix_lc) == name_data_lc
			|| format!("{}_data", name_pref_lc) == format!("{}_data", prefix_lc);
		// 维度：[frame_count][frame_size] 或 [frame_size][frame_count]
		let calc_total =
			|a: Option<u64>, b: Option<u64>| -> (Option<usize>, Option<usize>, usize) {
				// 返回 (推断出的frame_count, 推断出的frame_size, 总字节数)
				match (a, b) {
					(Some(x), Some(y)) => {
						let x = x as usize;
						let y = y as usize;
						let total = x * y;
						// 优先认为 [frame_count][frame_size]；如果反了就调换
						if y == expected_frame_size {
							(Some(x), Some(y), total)
						} else if x == expected_frame_size {
							(Some(y), Some(x), total)
						} else {
							// 没对上，按顺序返回原值
							(Some(x), Some(y), total)
						}
					}
					_ => (None, None, 0),
				}
			};
		let (maybe_fc, maybe_fs, total_from_dims) = calc_total(*d1, *d2);

		if matched {
			// A 类匹配：前缀一致。直接选定
			let final_fs = maybe_fs.unwrap_or(expected_frame_size);
			let final_fc = match maybe_fc {
				Some(c) => c,
				None => {
					if total_from_dims > 0 && final_fs > 0 {
						total_from_dims / final_fs
					} else {
						expected_frames
					}
				}
			};
			chosen = Some((*off, prefix.clone(), final_fs, final_fc));
			break;
		}
	}

	if chosen.is_none() {
		// B 类匹配：按维度总字节数 == expected_total
		for (off, prefix, d1, d2) in arrays.iter() {
			let (maybe_fc, maybe_fs, total) = match (d1, d2) {
				(Some(x), Some(y)) => {
					let total = *x as usize * *y as usize;
					if *y as usize == expected_frame_size {
						(Some(*x as usize), Some(*y as usize), total)
					} else if *x as usize == expected_frame_size {
						(Some(*y as usize), Some(*x as usize), total)
					} else if total == expected_total {
						// 总字节匹配
						let fc = if frame_count == Some(*x as usize) {
							Some(*x as usize)
						} else if frame_count == Some(*y as usize) {
							Some(*y as usize)
						} else {
							None
						};
						let fs = if fc.is_some() && total / fc.unwrap() > 0 {
							Some(total / fc.unwrap())
						} else {
							None
						};
						(fc, fs, total)
					} else {
						continue;
					}
				}
				_ => continue,
			};
			if maybe_fs.is_some() {
				let final_fs = maybe_fs.unwrap();
				let final_fc = maybe_fc.unwrap_or(if final_fs > 0 { total / final_fs } else { 1 });
				chosen = Some((*off, prefix.clone(), final_fs, final_fc));
				break;
			}
		}
	}

	// C 类：选择第一个 uint8_t 二维数组，并直接扫描字节
	let (scan_offset, chosen_prefix, mut confirmed_fs, mut confirmed_fc) = match chosen {
		Some(c) => c,
		None => {
			if arrays.is_empty() {
				bail!("头文件中找不到任何 uint8_t / unsigned char 类型的二维数组声明");
			}
			// 扫描第一个数组字节数，再按 expected_frame_size 估算
			let (off, pref, _, _) = &arrays[0];
			let (bytes, _fstarts) = scan_bytes_from(text, *off);
			let fs = expected_frame_size;
			let fc = if fs > 0 {
				(bytes.len() + fs - 1) / fs
			} else {
				1
			};
			(*off, pref.clone(), fs, fc.max(1))
		}
	};

	// 如果帧数/帧大小仍是 0 就用估计值
	if confirmed_fc == 0 {
		confirmed_fc = expected_frames.max(1);
	}
	if confirmed_fs == 0 {
		confirmed_fs = expected_frame_size;
	}

	// 7. 真正扫描字节
	let (bytes, frame_starts) = scan_bytes_from(text, scan_offset);

	// 可能扫描到的字节比需要的少；补齐或报错
	if bytes.len() < confirmed_fs * confirmed_fc {
		// 再次尝试：如果声明没有维度，frame_fs/fc 可能不匹配。尝试通过 dimensions 回退
		let total = expected_total.max(confirmed_fs * confirmed_fc);
		if bytes.len() < total {
			bail!(
				"字节数不足：声明 {}×{}={} 字节，实际扫描到 {} 字节。\n数组名前缀: {}，解析宏前缀: {}",
				confirmed_fc, confirmed_fs,
				confirmed_fc * confirmed_fs,
				bytes.len(),
				chosen_prefix, name_pref,
			);
		}
	}

	// 最终采用的 frame_size/frame_count：
	//   如果声明维度和宏都有，以宏为准；否则用扫描到的维度
	let final_frame_count = frame_count.unwrap_or(confirmed_fc.max(1));
	let final_frame_size = frame_size.unwrap_or(confirmed_fs.max(1));

	// 根据 frame_starts 分配每帧的字节切片
	let mut frames = Vec::with_capacity(final_frame_count);
	for i in 0..final_frame_count {
		let off = frame_starts.get(i).copied().unwrap_or(i * final_frame_size);
		let end = (off + final_frame_size).min(bytes.len());
		let data: Vec<u8> = if end - off == final_frame_size {
			bytes[off..end].to_vec()
		} else {
			// 长度不够就补 0
			let mut v = vec![0u8; final_frame_size];
			let copy_len = (end - off).min(final_frame_size);
			v[..copy_len].copy_from_slice(&bytes[off..off + copy_len]);
			v
		};
		let frame = bytes_to_binary_frame(data, width, height, format, reverse_bits)?;
		frames.push(frame);
	}

	// 最终 NAME：如果 chosen_prefix 和 name_pref 去掉 _data 前缀相等（忽略大小写），
	// 就用 name_pref；否则用 chosen_prefix 不带末尾 _data 的版本
	let final_name = {
		let cp = chosen_prefix
			.strip_suffix("_data")
			.unwrap_or(&chosen_prefix)
			.to_string();
		if cp.to_ascii_lowercase() == name_pref.to_ascii_lowercase() {
			// 大小写不同时优先用宏里的名字（宏是用户定义的）
			name_pref.clone()
		} else {
			cp
		}
	};

	Ok(ParsedHeader {
		name: final_name,
		format,
		reverse_bits,
		frames,
	})
}

fn compute_frame_size(w: u32, h: u32, format: ScanFormat) -> usize {
	let v_bytes = ((h + 7) / 8) as usize * w as usize;
	let h_bytes = ((w + 7) / 8) as usize * h as usize;
	match format {
		ScanFormat::VerticalLsb | ScanFormat::VerticalMsb => v_bytes,
		ScanFormat::HorizontalLsb | ScanFormat::HorizontalMsb => h_bytes,
	}
}

fn guess_format(w: u32, h: u32, frame_size: usize) -> ScanFormat {
	let v = compute_frame_size(w, h, ScanFormat::VerticalLsb);
	let hz = compute_frame_size(w, h, ScanFormat::HorizontalLsb);
	if frame_size == v {
		ScanFormat::VerticalLsb
	} else if frame_size == hz {
		ScanFormat::HorizontalLsb
	} else {
		ScanFormat::VerticalLsb
	}
}

/// 字节数组 -> BinaryFrame（与取模操作相反）
pub fn bytes_to_binary_frame(
	bytes: Vec<u8>,
	w: u32,
	h: u32,
	format: ScanFormat,
	reverse_bits: bool,
) -> Result<BinaryFrame> {
	let mut buf: Vec<u8> = bytes;
	if reverse_bits {
		#[rustfmt::skip]
		const REV: [u8; 16] = [
			0x0, 0x8, 0x4, 0xC, 0x2, 0xA, 0x6, 0xE,
			0x1, 0x9, 0x5, 0xD, 0x3, 0xB, 0x7, 0xF,
		];
		for b in &mut buf {
			*b = (REV[(*b & 0x0F) as usize] << 4) | REV[((*b >> 4) & 0x0F) as usize];
		}
	}

	let mut frame = BinaryFrame::new(w, h);
	let get_bit = |byte: u8, bit: u8| -> bool { (byte >> bit) & 1 == 1 };

	match format {
		ScanFormat::VerticalLsb => {
			// 页优先：字节号 = p * W + c，每页=8行，LSB=页内顶行
			let pages = (h + 7) as usize / 8;
			let w_usize = w as usize;
			for p in 0..pages {
				for c in 0..w_usize {
					let byte_idx = p * w_usize + c;
					if let Some(&b) = buf.get(byte_idx) {
						for k in 0..8u8 {
							let row = (p as u32) * 8 + (k as u32);
							if row < h && get_bit(b, k) {
								frame.set_pixel(c as u32, row, true);
							}
						}
					}
				}
			}
		}
		ScanFormat::VerticalMsb => {
			// 页优先：字节号 = p * W + c，每页=8行，MSB=页内顶行
			let pages = (h + 7) as usize / 8;
			let w_usize = w as usize;
			for p in 0..pages {
				for c in 0..w_usize {
					let byte_idx = p * w_usize + c;
					if let Some(&b) = buf.get(byte_idx) {
						for k in 0..8u8 {
							let row = (p as u32) * 8 + (k as u32);
							let bit = 7u8 - k;
							if row < h && get_bit(b, bit) {
								frame.set_pixel(c as u32, row, true);
							}
						}
					}
				}
			}
		}
		ScanFormat::HorizontalLsb => {
			let bytes_per_row = ((w + 7) / 8) as usize;
			for row in 0..h {
				let base = row as usize * bytes_per_row;
				for col in 0..w {
					let byte_idx = base + (col / 8) as usize;
					let bit = (col % 8) as u8;
					if let Some(&b) = buf.get(byte_idx) {
						if get_bit(b, bit) {
							frame.set_pixel(col, row, true);
						}
					}
				}
			}
		}
		ScanFormat::HorizontalMsb => {
			let bytes_per_row = ((w + 7) / 8) as usize;
			for row in 0..h {
				let base = row as usize * bytes_per_row;
				for col in 0..w {
					let byte_idx = base + (col / 8) as usize;
					let bit = 7 - ((col % 8) as u8);
					if let Some(&b) = buf.get(byte_idx) {
						if get_bit(b, bit) {
							frame.set_pixel(col, row, true);
						}
					}
				}
			}
		}
	}

	Ok(frame)
}
