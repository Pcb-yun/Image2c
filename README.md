# Img2C — 单色屏幕取模工具

针对 OLED / LCD 等单色屏幕（12864 等）的取模软件。支持 **图片 / GIF 动图 / 视频** 三种输入，可自由调整输出尺寸、阈值、扫描格式、字节反转等参数，并输出为符合单片机开发习惯的 **C 语言头文件（.h）**。

- GUI：Rust + eframe / egui（跨平台，已在 Windows 下验证）
- 输入：PNG / JPG / BMP / GIF / TIFF / WEBP / MP4 / AVI / MOV / MKV / …
- 输出：带尺寸宏 + 帧二维数组的 `.h` 文件（支持多帧动画）
- 取模字节序：已对 SSD1306 / SSD1309 页面寻址模式做原生对齐（**页优先 p×W+c**），烧录即可正确显示

## License

本项目基于 [MIT License](LICENSE) 开源，可自由使用、修改、分发。

---

## 目录结构

```
Image2c/
├── Cargo.toml
├── Cargo.lock
├── rustfmt.toml        // 代码格式化配置（hard_tabs = true）
├── README.md           // 本说明文档
├── src/
│   ├── main.rs        // GUI 主程序：面板、参数、预览、导入/导出
│   ├── processor.rs   // 核心取模：缩放、二值化、4 种扫描格式、字节反转
│   ├── importer.rs    // 头文件解析：读取 .h → 反解像素 → 预览
│   ├── exporter.rs    // 头文件生成：格式化宏 + 字节二维数组
│   └── video.rs       // 视频 / GIF 抽帧（视频依赖 ffmpeg）
└── target/
    └── release/
        └── img2c.exe  // 编译产物（发布版）
```

---

## 编译方法

### 环境要求
- Windows 10/11 x64
- [Rust 工具链](https://rustup.rs/)（`rustup default stable-x86_64-pc-windows-msvc`）
- （可选，处理视频时需要）[ffmpeg](https://ffmpeg.org/download.html) 已加入 `PATH`，或把 `ffmpeg.exe` 放到 `img2c.exe` 同目录

### 发布版编译（推荐）
```powershell
cd Image2c
cargo build --release
# 产物：target/release/img2c.exe
```

### 开发版编译（编译更快，带调试符号）
```powershell
cargo build
# 产物：target/debug/img2c.exe
```

### 运行内置单元测试
```powershell
cargo test --release
```
> 单元测试已移除，此命令目前无测试执行。如需恢复测试请参考 Git 历史。

---

## 使用说明

### 1. 启动
双击运行 `target/release/img2c.exe`。程序已配置 `windows_subsystem = "windows"`，启动时不会弹出终端窗口。首次运行会自动在系统字体目录里查找中文字体（按优先级：微软雅黑 → 黑体 → 宋体 → 等线），无需另外配置字体。

### 2. 打开文件
点击左上角 **📂 打开…**，选择：
- **图片**：PNG / JPG / BMP / TIFF / WEBP → 单帧取模
- **GIF**：动图 → 自动拆分所有帧，支持选择帧区间
- **视频**：MP4 / AVI / MOV / MKV / WMV / FLV / WEBM / M4V / MPG / TS / 3GP → 调用 `ffmpeg` 抽帧（需 ffmpeg 可用）
- **C 头文件（.h）**：读入之前导出的头文件 → 显示预览，可直接换参数重新导出

### 3. 常用驱动器快捷配置
右侧「**常用驱动器**」面板一键设置参数：

| 快捷配置 | 尺寸 | 扫描格式 | 适用屏幕 |
| --- | --- | --- | --- |
| SSD1306 / SSD1309 128×64 | 128×64 | vertical_lsb | 通用 0.96" 12864 OLED |
| SSD1306 128×32 | 128×32 | vertical_lsb | 窄款 0.91" OLED |
| SSD1306 64×48 | 64×48 | vertical_lsb | 小尺寸 OLED |
| SH1106 128×64 | 128×64 | vertical_lsb | 兼容 SSD1306 的 1.3" OLED |
| SH1106 132×64 | 132×64 | vertical_lsb | 1.3" OLED 原生分辨率 |
| SSD1315 128×64 | 128×64 | vertical_lsb | 同 SSD1306 |
| ST7567 / ST7565 128×64 | 128×64 | vertical_lsb | 串行/并行 12864 LCD |
| UC1701 128×64 | 128×64 | vertical_lsb | 12864 LCD |
| PCD8544 (Nokia5110) 84×48 | 84×48 | vertical_msb | 诺基亚屏，MSB 在上 |
| ST7920 128×64 (串/并) | 128×64 | horizontal_msb | 字库型 12864 |
| SSD1327 (灰阶单色) 128×128 | 128×128 | horizontal_msb | 水平写入 |
| WS0010 / OLED 128×64 | 128×64 | horizontal_msb | 字库型 OLED |
| SSD1675A / 墨水屏 2.13" | 250×122 | horizontal_msb | 水平扫 MSB |
| SSD1322 256×64 (灰阶) | 256×64 | horizontal_msb | 0.95"–2.8" 黄蓝 OLED |

> **关键说明**：SSD1306 / SSD1309 / SH1106 / ST7567 等大多数 12864，页面寻址模式下字节序均为「页优先」（先 Page 0 全部 Column 0…127，再 Page 1…），本工具的 `vertical_lsb` / `vertical_msb` 均按 **页优先** 输出，与屏幕 DMA 顺序一致，无需在单片机侧再做重排。

### 4. 取模参数（手动微调）
| 参数 | 说明 |
| --- | --- |
| 输出宽度 / 高度 | 最终生成数组的像素尺寸。默认「🔒 等比缩放」勾选，改一边另一边按比例自动计算；取消后可拉伸 |
| 缩放方式 | 可选 Nearest（像素硬切，推荐小图）/ Triangle / CatmullRom / Gaussian / Lanczos3 |
| 阈值 | 0–255，灰度 ≥ 阈值的像素记为白（点亮）。默认 128 |
| 反色 | 勾选后：黑变白、白变黑 |
| 扫描格式 | vertical_lsb / vertical_msb / horizontal_lsb / horizontal_msb（四种组合全覆盖） |
| 字节位反转 | 勾选后每个字节 bit0↔bit7 对调，兼容特殊控制器接法 |
| 未满 8 位填 1 | 宽/高非 8 倍数时，最后一字节的空闲位默认填 0；勾上填 1，用于与老取模工具输出完全一致 |
| 导出名称 | C 宏名/数组名前缀，默认 `IMG`，会自动生成 `IMG_WIDTH` `IMG_HEIGHT` `IMG_FRAME_COUNT` `IMG_FRAME_SIZE` `IMG_DATA` |

### 5. 帧区间 / 视频抽帧
GIF 或视频导入后，「帧区间 / 视频抽帧」面板激活：

- **帧范围**：起始帧 / 结束帧，GIF 以帧号为准；视频可按「帧号」或「时间（秒）」两种模式裁剪
- **抽帧模式**（仅视频）：
  - 按 FPS：比如 `8` = 每秒抽 8 帧
  - 每 N 帧：比如 `5` = 每 5 帧取 1 帧
  - 固定总帧数：直接指定最终想得到多少帧，按目标总数在区间内均匀采样
- **🔄 按当前范围重新抽帧**：参数调好后点一下即生效，左侧预览同步更新

### 6. 预览
主界面中央水平分两栏：**原图 ↔ 取模结果**，参数变化后自动刷新（有 debounce，避免抖动）。
- 自适应窗口大小显示，不会出现像素「平滑糊化」，保持单色方块清晰
- 预览窗下方有「原始分辨率查看」按钮，可弹出新窗口以 1:1 精确查看取模像素

### 7. 导出 / 复制
- **💾 导出 C 头文件**：保存为 `.h` 文件。内容格式示例：

  ```c
  #ifndef IMG_H
  #define IMG_H

  #include <stdint.h>

  #define IMG_WIDTH           128    // 图像宽度
  #define IMG_HEIGHT          64     // 图像高度
  #define IMG_FRAME_COUNT     1      // 帧数
  #define IMG_FRAME_SIZE      1024   // 单帧字节数

  static const uint8_t img_data[IMG_FRAME_COUNT][IMG_FRAME_SIZE] = {
      {
          0x01,0x02,0x04,...
      },
  };

  #endif
  ```

  - 宏定义使用数字**居中对齐**、注释**同列**，宏名与数字之间至少保留一个制表符宽度
  - 二维数组最后一个字节也**保留逗号**，方便追加修改

- **📋 复制到剪贴板**：把完整的 `.h` 内容直接复制到剪贴板，可粘贴进 Keil / IAR / VS Code / Arduino IDE 中使用

---

## 输出格式技术细节

### 四种扫描格式的字节定义
通用：宽 W 高 H，每 8 个相邻像素（在扫描方向上）打包为 1 字节，第 1 个像素对应 bit0（LSB 模式）或 bit7（MSB 模式）。

#### vertical_lsb（SSD1306 12864 原生，推荐）
- 页优先：字节号 `i = p * W + c`，p = 页号（0…⌈H/8⌉−1），c = 列号（0…W−1）
- 每字节 8 个像素按列方向堆叠：bit0 = 该页最顶行，bit7 = 该页最底行
- 例：128×64 → 1024 字节 = 8 页 × 128 列

#### vertical_msb（PCD8544 等）
- 页优先：同上 i = p×W + c
- bit7 = 该页最顶行，bit0 = 该页最底行

#### horizontal_lsb
- 行优先：字节号 `i = r * ⌈W/8⌉ + ⌊c/8⌋`
- 每字节 8 个像素按行方向排列：bit0 = 最左，bit7 = 最右

#### horizontal_msb（ST7920 字库屏等）
- 行优先：同上
- bit7 = 最左，bit0 = 最右

### 字节反转
勾选后对每个输出字节做 `bit_reverse8()`：`0b00000001 → 0b10000000`。可与四种扫描格式任意叠加，共 8 种实际输出组合。

### 未满 8 位填充
H 非 8 倍数时（如 63），最后一页每页字节的「空闲高位」默认填 0。勾上「未满 8 位填 1」则填 1。W 非 8 倍数时同理对应水平方向空闲位。

---

## 常见问题

### Q：烧录到 SSD1306 / SSD1309 上显示乱码、线条错位？
A：
1. 确保选的是「SSD1306 / SSD1309 128×64」快捷配置（扫描格式 vertical_lsb、字节反转关）
2. 确保屏幕初始化使用「页面寻址模式」（`0x20, 0x02`，或不写 0x20 命令即默认页寻址）
3. 如果是水平寻址模式（`0x20, 0x00`），同样适用，因为页优先的内存布局恰好是屏幕的线性顺序

### Q：视频点了没反应？
A：必须安装 `ffmpeg`。在命令行执行 `ffmpeg -version`，有正常输出版本号即可。Windows 用户建议从 [gyan.dev](https://www.gyan.dev/ffmpeg/builds/) 下载 essentials 版解压并把 `bin` 目录加入 `PATH`。

### Q：生成的 .h 头文件再打开，会不会识别不到数组？
A：
- 数组名前缀 **大小写不敏感**；也可直接识别任意 `uint8_t / unsigned char` 类型二维数组
- 数组维度用宏名（如 `[MY_FRAME_COUNT][MY_FRAME_SIZE]`）也能识别，前提是同一个头文件里有用 `#define` 给出这两个宏的数字

### Q：如何做动图 / 视频动画？
A：
1. 导入 GIF 或视频 → 设置帧区间 + 抽帧模式 → 点「🔄 重新抽帧」
2. 预览面板下方的滑块可浏览每帧，确认没问题后导出
3. 单片机侧按固定时间间隔切换 `data[frame_idx]` 即可播放动画

### Q：想改默认参数 / 加一个新驱动器快捷配置？
A：在 [src/main.rs](file:///f:/Image2c/src/main.rs#L628-L643) 的 `drivers` 数组里按格式追加一条 `(名称, 宽, 高, 扫描格式, 反转位, 反色, 说明)` 即可。
