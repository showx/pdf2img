# pdf2img

把 PDF 每一页转成常见图片，也可以把已有图片压缩到指定体积。带图形界面，Windows 上双击即可使用。

## 功能

- 将 PDF 按页导出为 PNG、JPEG、WebP、BMP
- 用像素宽高范围控制输出大小，并保持页面原始比例
- 压缩 PNG、JPEG、WebP、BMP，支持普通压缩、高品质压缩，以及限制每张图片的体积
- 压缩后可调整分辨率；打开「保持比例」时，改一边另一边会跟着变

图片都保存在源文件所在的文件夹里。

## 直接使用

把 `pdf2img.exe` 拷到 64 位 Windows 电脑上，双击打开。不需要安装 Rust 或其他依赖。界面使用系统中文字体，适合中文版 Windows。

窗口顶部可以在两个功能之间切换：

- **PDF 转图片**
- **图片压缩**

也可以把文件直接拖进窗口。

### PDF 转图片

选择 PDF 后设置格式和像素范围，再点「开始转换」。

- 宽高留空表示不限制这一边
- 没有填写上限时，按分辨率渲染，默认 144（每点 2 像素）
- 同时填了下限和上限时，会在不超过上限的前提下尽量达到下限
- 页码从 1 开始，例如 `1-3,5`；留空则转换全部页面
- 加密 PDF 可以填写打开密码。密码只在当次使用，不会被记住

生成的文件名为「原名-页码.扩展名」，例如 `notes-01.png`。

### 图片压缩

支持 PNG、JPEG、WebP、BMP。不支持 GIF 和 SVG。

- **普通压缩**：压缩比例较小，文件相对更大
- **高品质压缩**：在尽量保持清晰的前提下缩小体积
- **每张图片体积小于**：指定上限，单位可以是 KB 或 MB。当前分辨率压不进去时，会按比例再缩小

结果保存为 JPEG，文件名以 `-compressed` 结尾，不会覆盖原图。例如 `scan.png` 会得到 `scan-compressed.jpg`。

## 从源码构建

需要 [Rust](https://rustup.rs/) 1.92 或更高版本。

```powershell
cargo build --release
```

图形界面程序在 `target\release\pdf2img.exe`。

## 命令行

图形界面是主要用法。仓库里还提供了命令行程序：

```powershell
cargo build --release --bin pdf2img-cli
```

```text
pdf2img-cli notes.pdf
pdf2img-cli notes.pdf -f jpeg --max-width 1920 --max-height 1080
pdf2img-cli notes.pdf -f png --min-width 800 --max-width 1600 --pages 1-3
pdf2img-cli notes.pdf -f webp --dpi 200 --pages 1,4,6-8
```

JPEG 质量用 `-q` 指定，范围是 1 到 100。WebP 按无损编码，不使用该参数。

## 说明

PDF 渲染使用 [hayro](https://github.com/LaurenzV/hayro)。常见页面可以正常转换；个别使用了冷门特性的 PDF，渲染结果可能和原件不完全一致。

## 许可

本项目以 MIT 许可证发布，见 `Cargo.toml` 中的 `license` 字段。
