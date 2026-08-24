use crate::processor::{BinaryFrame, ScanFormat};

/// 导出配置
pub struct ExportConfig {
	/// 数组名（生成的变量名）
	pub name: String,
	/// 是否启用条件编译（IMG2C_XXX宏）
	pub use_conditional: bool,
	/// 每行输出的字节数
	pub bytes_per_line: usize,
}

impl Default for ExportConfig {
	fn default() -> Self {
		Self {
			name: "img".to_string(),
			use_conditional: true,
			bytes_per_line: 16,
		}
	}
}

/// 将名称转换为安全的C标识符
fn to_c_ident(name: &str) -> String {
	let mut result = String::new();
	let mut first = true;
	for c in name.chars() {
		if first {
			if c.is_ascii_alphabetic() || c == '_' {
				result.push(c);
			} else if c.is_ascii_digit() {
				result.push('_');
				result.push(c);
			} else {
				result.push('_');
			}
			first = false;
		} else {
			if c.is_ascii_alphanumeric() || c == '_' {
				result.push(c);
			} else {
				result.push('_');
			}
		}
	}
	if result.is_empty() {
		result.push_str("img_data");
	}
	result.to_uppercase()
}

/// 生成头文件保护宏的名称
fn guard_name(name: &str) -> String {
	format!("{}_H", to_c_ident(name))
}

/// 将字节数组格式化为C语言初始化列表
fn format_bytes(bytes: &[u8], bytes_per_line: usize) -> String {
	let mut lines = Vec::new();
	let mut line = String::new();

	for (i, byte) in bytes.iter().enumerate() {
		if i > 0 && i % bytes_per_line == 0 {
			lines.push(line.clone());
			line.clear();
		}
		line.push_str(&format!("0x{:02X},", byte));
	}
	if !line.is_empty() {
		lines.push(line);
	}

	// 每行加缩进
	lines
		.iter()
		.map(|l| format!("    {}", l))
		.collect::<Vec<_>>()
		.join("\n")
}

/// 导出多帧为C头文件内容
pub fn export_c_header(
	frames: &[BinaryFrame],
	format: ScanFormat,
	reverse_bits: bool,
	pad_unused_bits_1: bool,
	config: &ExportConfig,
) -> String {
	if frames.is_empty() {
		return String::from("/* 错误：没有帧数据 */");
	}

	let name_ident = to_c_ident(&config.name);
	let guard = guard_name(&config.name);
	let width = frames[0].width;
	let height = frames[0].height;
	let frame_size = frames[0].frame_size(format);
	let frame_count = frames.len();
	let total_size = frame_size * frame_count;

	let mut out = String::new();

	// 文件头注释
	out.push_str("/* 自动生成 by img2c\n");
	out.push_str(&format!(" * 尺寸: {} x {}\n", width, height));
	out.push_str(&format!(" * 格式: {}\n", format.name()));
	if reverse_bits {
		out.push_str(" * 字节内位序: 已反转\n");
	}
	out.push_str(&format!(" * 单帧字节数: {}\n", frame_size));
	out.push_str(&format!(" * 总字节数: {}\n", total_size));
	out.push_str(" */\n");

	// 头文件保护
	out.push_str(&format!("#ifndef {}\n", guard));
	out.push_str(&format!("#define {}\n", guard));
	out.push('\n');
	out.push_str("#include <stdint.h>\n");
	out.push('\n');

	// 条件编译开关 + 尺寸宏定义统一对齐
	// 收集所有宏：(宏名, 值, 注释)
	let mut macros: Vec<(String, String, &str)> = Vec::new();
	if config.use_conditional {
		macros.push((format!("IMG2C_{}", name_ident), "1".to_string(), "是否编译"));
	}
	macros.push((
		format!("{}_WIDTH", name_ident),
		width.to_string(),
		"图像宽度(像素)",
	));
	macros.push((
		format!("{}_HEIGHT", name_ident),
		height.to_string(),
		"图像高度(像素)",
	));
	macros.push((
		format!("{}_FRAME_SIZE", name_ident),
		frame_size.to_string(),
		"单帧字节数",
	));
	macros.push((
		format!("{}_FRAME_COUNT", name_ident),
		frame_count.to_string(),
		"帧数",
	));
	macros.push((
		format!("{}_TOTAL_SIZE", name_ident),
		total_size.to_string(),
		"总字节数",
	));

	// 计算宏名列和数字列的最大宽度
	let name_w = macros.iter().map(|(n, _, _)| n.len()).max().unwrap_or(0);
	let val_w = macros.iter().map(|(_, v, _)| v.len()).max().unwrap_or(0);

	// 生成每行：#define 宏名(左对齐填充到最长) \t 数字(居中) \t // 注释
	let mut cond_emitted = false;
	for (name, val, comment) in &macros {
		if config.use_conditional && !cond_emitted && name.starts_with("IMG2C_") {
			// 条件编译宏单独输出
			out.push_str(&format!(
				"#define {:<name_w$}\t{:^val_w$}\t// {}\n",
				name,
				val,
				comment,
				name_w = name_w,
				val_w = val_w
			));
			out.push_str(&format!("#if {}\n", name));
			out.push('\n');
			cond_emitted = true;
		} else if !name.starts_with("IMG2C_") {
			out.push_str(&format!(
				"#define {:<name_w$}\t{:^val_w$}\t// {}\n",
				name,
				val,
				comment,
				name_w = name_w,
				val_w = val_w
			));
		}
	}
	out.push('\n');

	// 数组声明
	let array_name = format!("{}_data", config.name.to_lowercase());
	out.push_str(&format!(
		"static const uint8_t {}[{}_FRAME_COUNT][{}_FRAME_SIZE] = {{\n",
		array_name, name_ident, name_ident
	));

	// 每一帧数据
	for (fi, frame) in frames.iter().enumerate() {
		let bytes = frame.to_bytes(format, reverse_bits, pad_unused_bits_1);
		out.push_str("  {\n");
		out.push_str(&format_bytes(&bytes, config.bytes_per_line));
		out.push('\n');
		if fi + 1 < frame_count {
			out.push_str("  },\n");
		} else {
			out.push_str("  },\n");
		}
	}
	out.push_str("};\n");

	// 条件编译结束
	if config.use_conditional {
		let cond_macro = format!("IMG2C_{}", name_ident);
		out.push_str(&format!("#endif /* {} */\n", cond_macro));
		out.push('\n');
	}

	// 头文件保护结束
	out.push_str(&format!("#endif /* {} */\n", guard));

	out
}
