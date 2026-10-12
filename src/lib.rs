mod compress;
mod encode;
mod pages;
mod scale;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, PixmapSettings, RenderCache, RenderSettings};

use pages::parse_pages;
use scale::fit_page;

pub use compress::{
    compress_to, height_for_width, output_jpeg_path, read_dimensions, unsupported_reason,
    width_for_height, CompressMode, CompressResult,
};
pub use scale::{FittedSize, SizeLimits};

/// 支持的输出图片格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// 无损 PNG，保留透明通道。
    Png,
    /// 有损 JPEG。透明区域会铺在白底上。
    #[value(alias = "jpg")]
    Jpeg,
    /// 无损 WebP。
    Webp,
    /// Windows 位图。透明区域会铺在白底上。
    Bmp,
}

impl OutputFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
        }
    }
}

/// 一次转换使用的参数。
#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub format: OutputFormat,
    pub limits: SizeLimits,
    /// 未触及尺寸上限时使用的分辨率。
    pub dpi: f32,
    /// JPEG 质量，1 到 100。WebP 目前按无损编码，不使用该值。
    pub quality: u8,
    /// 从 1 开始的页码表达式。`None` 表示全部页面。
    pub pages: Option<String>,
    pub password: Option<String>,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self {
            format: OutputFormat::Png,
            limits: SizeLimits::default(),
            dpi: 144.0,
            quality: 85,
            pages: None,
            password: None,
        }
    }
}

/// 成功写出的一页图片。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedImage {
    /// 从 1 开始的页码。
    pub page: usize,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub below_minimum: bool,
}

impl OutputFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP",
            Self::Bmp => "BMP",
        }
    }
}

/// 某一页写出之后的进度。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertProgress {
    /// 本次任务中已完成的页数，从 1 开始。
    pub index: usize,
    /// 本次任务要转换的页数。
    pub total: usize,
    pub image: SavedImage,
}

/// 把 PDF 的选定页面渲染成图片，保存到该 PDF 所在目录。
pub fn convert_pdf(pdf_path: &Path, options: &ConvertOptions) -> Result<Vec<SavedImage>> {
    convert_pdf_with_progress(pdf_path, options, |_| {})
}

/// 与 [`convert_pdf`] 相同，每写出一页就回调一次。
pub fn convert_pdf_with_progress(
    pdf_path: &Path,
    options: &ConvertOptions,
    mut on_progress: impl FnMut(ConvertProgress),
) -> Result<Vec<SavedImage>> {
    if !(1..=100).contains(&options.quality) {
        bail!("图片质量必须在 1 到 100 之间");
    }
    if !pdf_path.is_file() {
        bail!("找不到 PDF 文件: {}", pdf_path.display());
    }

    let bytes = fs::read(pdf_path)
        .with_context(|| format!("无法读取 PDF: {}", pdf_path.display()))?;
    let pdf = match options.password.as_deref() {
        Some(password) => Pdf::new_with_password(bytes, password),
        None => Pdf::new(bytes),
    }
    .map_err(|error| anyhow::anyhow!("无法打开 PDF: {error:?}"))?;

    let page_count = pdf.pages().iter().count();
    if page_count == 0 {
        bail!("这份 PDF 没有页面");
    }

    let selected = select_pages(page_count, options.pages.as_deref())?;
    let output_dir = output_directory(pdf_path);
    let stem = pdf_path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "page".to_string());
    let digits = page_count.to_string().len();

    let cache = RenderCache::new();
    let interpreter = InterpreterSettings::default();
    let render_settings = RenderSettings::default();
    let mut saved = Vec::with_capacity(selected.len());

    for (index, page) in pdf.pages().iter().enumerate() {
        let page_number = index + 1;
        if !selected.contains(&page_number) {
            continue;
        }
        let (page_width, page_height) = page.render_dimensions();
        let fitted = fit_page(page_width, page_height, options.dpi, options.limits)
            .with_context(|| format!("第 {page_number} 页无法按指定大小范围缩放"))?;

        let pixmap = render(
            page,
            &cache,
            &interpreter,
            &render_settings,
            &PixmapSettings {
                x_scale: fitted.scale,
                y_scale: fitted.scale,
                bg_color: WHITE,
            },
        );

        let width = u32::from(pixmap.width());
        let height = u32::from(pixmap.height());
        let file_name = format!(
            "{stem}-{page_number:0digits$}.{}",
            options.format.extension()
        );
        let path = output_dir.join(file_name);
        encode::write_image(
            &path,
            options.format,
            width,
            height,
            pixmap.data_as_u8_slice(),
            options.quality,
        )?;

        let image = SavedImage {
            page: page_number,
            path,
            width,
            height,
            below_minimum: fitted.below_minimum,
        };
        let index = saved.len() + 1;
        on_progress(ConvertProgress {
            index,
            total: selected.len(),
            image: image.clone(),
        });
        saved.push(image);
    }

    Ok(saved)
}

fn select_pages(page_count: usize, spec: Option<&str>) -> Result<Vec<usize>> {
    let Some(spec) = spec else {
        return Ok((1..=page_count).collect());
    };

    let pages = parse_pages(spec).map_err(anyhow::Error::msg)?;
    if let Some(page) = pages.iter().copied().find(|page| *page > page_count) {
        bail!("页码 {page} 超出范围，这份 PDF 共 {page_count} 页");
    }
    if pages.is_empty() {
        bail!("没有可转换的页面");
    }
    Ok(pages)
}

fn output_directory(pdf_path: &Path) -> PathBuf {
    match pdf_path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use super::{convert_pdf, ConvertOptions, OutputFormat};
    use crate::scale::SizeLimits;

    fn rectangle_pdf() -> Vec<u8> {
        let content = b"1 0 0 rg\n20 20 120 60 re\nf\n";
        let objects = [
            b"<< /Type /Catalog /Pages 2 0 R >>".to_vec(),
            b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_vec(),
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 300 200] /Contents 4 0 R >>".to_vec(),
            format!(
                "<< /Length {} >>\nstream\n{}endstream",
                content.len(),
                std::str::from_utf8(content).unwrap()
            )
            .into_bytes(),
        ];
        build_pdf(&objects)
    }

    fn build_pdf(objects: &[Vec<u8>]) -> Vec<u8> {
        let mut body = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::with_capacity(objects.len());
        for (index, object) in objects.iter().enumerate() {
            offsets.push(body.len());
            body.extend(format!("{} 0 obj\n", index + 1).into_bytes());
            body.extend_from_slice(object);
            body.extend_from_slice(b"\nendobj\n");
        }

        let xref_at = body.len();
        body.extend(format!("xref\n0 {}\n", objects.len() + 1).into_bytes());
        body.extend(b"0000000000 65535 f \n");
        for offset in offsets {
            body.extend(format!("{offset:010} 00000 n \n").into_bytes());
        }
        body.extend(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
                objects.len() + 1
            )
            .into_bytes(),
        );
        body
    }

    #[test]
    fn renders_a_page_into_the_pdf_directory_within_the_size_range() {
        let dir = std::env::temp_dir().join(format!("pdf2img-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pdf_path = dir.join("sample.pdf");
        std::fs::write(&pdf_path, rectangle_pdf()).unwrap();

        let saved = convert_pdf(
            &pdf_path,
            &ConvertOptions {
                format: OutputFormat::Png,
                limits: SizeLimits {
                    min_width: Some(120),
                    max_width: Some(150),
                    min_height: None,
                    max_height: Some(120),
                },
                dpi: 72.0,
                ..ConvertOptions::default()
            },
        )
        .unwrap();

        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].page, 1);
        assert!(saved[0].width <= 150);
        assert!(saved[0].height <= 120);
        assert!(saved[0].width >= 120 || saved[0].below_minimum);
        assert_eq!(saved[0].path, dir.join("sample-1.png"));

        let image = image::open(&saved[0].path).unwrap().to_rgba8();
        assert_eq!((image.width(), image.height()), (saved[0].width, saved[0].height));
        let has_red = image.pixels().any(|pixel| pixel[0] > 180 && pixel[1] < 40 && pixel[2] < 40);
        assert!(has_red, "渲染结果里应该能看到红色矩形");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
