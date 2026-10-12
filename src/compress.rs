use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, RgbImage};

/// 单边像素上限，避免一次压缩占用过多内存。
const MAX_EDGE: u32 = 16_384;
const NORMAL_QUALITY: u8 = 92;
const HIGH_QUALITY: u8 = 80;

/// 图片压缩方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressMode {
    /// 压缩幅度小，文件相对更大。
    Normal,
    /// 在仍然清晰的前提下尽量缩小体积。
    HighQuality,
    /// 每张图片不超过给定字节数。
    MaxBytes(u64),
}

/// 压缩完成后的实际结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressResult {
    pub width: u32,
    pub height: u32,
    pub bytes: u64,
    pub quality: u8,
}

/// 这种扩展名不能压缩。GIF 和 SVG 会给出单独说明。
pub fn unsupported_reason(path: &Path) -> Option<&'static str> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase());
    match ext.as_deref() {
        Some("gif" | "svg") => Some("不支持 GIF、SVG 格式"),
        Some("png" | "jpg" | "jpeg" | "webp" | "bmp") => None,
        _ => Some("只支持 PNG、JPEG、WebP、BMP"),
    }
}

/// 读取图片宽高。调用前应先用 [`unsupported_reason`] 过滤格式。
pub fn read_dimensions(path: &Path) -> Result<(u32, u32)> {
    image::image_dimensions(path).with_context(|| format!("无法读取图片尺寸：{}", path.display()))
}

/// 压缩结果文件名：与原图同一目录，扩展名为 jpg。
pub fn output_jpeg_path(src: &Path) -> PathBuf {
    let dir = src.parent().filter(|dir| !dir.as_os_str().is_empty());
    let stem = src
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "image".to_string());
    let file_name = src
        .file_name()
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let file_name = if file_name.ends_with("-compressed.jpg") {
        format!("{stem}-2.jpg")
    } else {
        format!("{stem}-compressed.jpg")
    };
    match dir {
        Some(dir) => dir.join(file_name),
        None => PathBuf::from(file_name),
    }
}

/// 把图片缩放到指定像素后压成 JPEG，写到 `dest`。
///
/// 体积上限在当前分辨率下无法达到时，会按比例缩小，并在结果里返回实际宽高。
pub fn compress_to(
    src: &Path,
    dest: &Path,
    width: u32,
    height: u32,
    mode: CompressMode,
) -> Result<CompressResult> {
    if let Some(reason) = unsupported_reason(src) {
        bail!("{reason}");
    }
    validate_edge(width, height)?;
    if src == dest {
        bail!("压缩结果不能覆盖原图");
    }

    let image = image::open(src).with_context(|| format!("无法打开图片：{}", src.display()))?;
    let rgb = composite_on_white(&image.to_rgba8());
    let resized = if rgb.width() == width && rgb.height() == height {
        rgb
    } else {
        image::imageops::resize(&rgb, width, height, FilterType::Lanczos3)
    };

    let (image, quality, bytes) = match mode {
        CompressMode::Normal => encode_fixed(resized, NORMAL_QUALITY)?,
        CompressMode::HighQuality => encode_fixed(resized, HIGH_QUALITY)?,
        CompressMode::MaxBytes(limit) => {
            if limit < 1024 {
                bail!("体积上限至少为 1 KB");
            }
            fit_under_limit(resized, limit)?
        }
    };

    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("无法创建目录：{}", parent.display()))?;
        }
    }
    std::fs::write(dest, &bytes).with_context(|| format!("无法保存图片：{}", dest.display()))?;

    Ok(CompressResult {
        width: image.width(),
        height: image.height(),
        bytes: bytes.len() as u64,
        quality,
    })
}

/// 按原图宽高比，由宽度推出高度。
pub fn height_for_width(source_w: u32, source_h: u32, width: u32) -> u32 {
    if source_w == 0 {
        return source_h.max(1);
    }
    ((u64::from(width) * u64::from(source_h)) / u64::from(source_w)).max(1) as u32
}

/// 按原图宽高比，由高度推出宽度。
pub fn width_for_height(source_w: u32, source_h: u32, height: u32) -> u32 {
    if source_h == 0 {
        return source_w.max(1);
    }
    ((u64::from(height) * u64::from(source_w)) / u64::from(source_h)).max(1) as u32
}

fn validate_edge(width: u32, height: u32) -> Result<()> {
    if width == 0 || height == 0 {
        bail!("分辨率必须大于 0");
    }
    if width > MAX_EDGE || height > MAX_EDGE {
        bail!("分辨率单边不能超过 {MAX_EDGE} 像素");
    }
    Ok(())
}

fn composite_on_white(rgba: &image::RgbaImage) -> RgbImage {
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (dst, src) in rgb.pixels_mut().zip(rgba.pixels()) {
        let alpha = u16::from(src[3]);
        let inverse = 255 - alpha;
        dst[0] = ((u16::from(src[0]) * alpha + 255 * inverse) / 255) as u8;
        dst[1] = ((u16::from(src[1]) * alpha + 255 * inverse) / 255) as u8;
        dst[2] = ((u16::from(src[2]) * alpha + 255 * inverse) / 255) as u8;
    }
    rgb
}

fn encode_fixed(image: RgbImage, quality: u8) -> Result<(RgbImage, u8, Vec<u8>)> {
    let quality = quality.clamp(1, 100);
    let bytes = encode_jpeg(&image, quality)?;
    Ok((image, quality, bytes))
}

fn encode_jpeg(image: &RgbImage, quality: u8) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    JpegEncoder::new_with_quality(&mut bytes, quality)
        .write_image(image.as_raw(), image.width(), image.height(), ExtendedColorType::Rgb8)
        .context("JPEG 编码失败")?;
    Ok(bytes)
}

fn fit_under_limit(mut image: RgbImage, limit: u64) -> Result<(RgbImage, u8, Vec<u8>)> {
    for _ in 0..16 {
        if let Some((quality, bytes)) = best_quality_within(&image, limit)? {
            return Ok((image, quality, bytes));
        }
        let width = image.width();
        let height = image.height();
        if width <= 64 || height <= 64 {
            bail!("按当前分辨率无法压到指定体积，请调低分辨率或增大体积上限");
        }
        let next_w = ((f64::from(width) * 0.85).round() as u32).max(1);
        let next_h = ((f64::from(height) * 0.85).round() as u32).max(1);
        if next_w == width && next_h == height {
            bail!("按当前分辨率无法压到指定体积，请调低分辨率或增大体积上限");
        }
        image = image::imageops::resize(&image, next_w, next_h, FilterType::Lanczos3);
    }
    bail!("按当前分辨率无法压到指定体积，请调低分辨率或增大体积上限")
}

/// 在不超过 `limit` 的前提下选择最高 JPEG 质量。当前尺寸怎么压都超限时返回 `None`。
fn best_quality_within(image: &RgbImage, limit: u64) -> Result<Option<(u8, Vec<u8>)>> {
    let high = encode_jpeg(image, 95)?;
    if high.len() as u64 <= limit {
        return Ok(Some((95, high)));
    }
    let low = encode_jpeg(image, 1)?;
    if low.len() as u64 > limit {
        return Ok(None);
    }

    let mut best_quality = 1;
    let mut best_bytes = low;
    let mut low_q = 2u8;
    let mut high_q = 94u8;
    while low_q <= high_q {
        let mid = (low_q + high_q) / 2;
        let bytes = encode_jpeg(image, mid)?;
        if bytes.len() as u64 <= limit {
            best_quality = mid;
            best_bytes = bytes;
            low_q = mid.saturating_add(1);
        } else if mid == 0 {
            break;
        } else {
            high_q = mid - 1;
        }
    }
    Ok(Some((best_quality, best_bytes)))
}

#[cfg(test)]
mod tests {
    use super::{
        compress_to, height_for_width, output_jpeg_path, unsupported_reason, width_for_height,
        CompressMode,
    };
    use image::{Rgb, RgbImage};

    fn noisy(width: u32, height: u32) -> RgbImage {
        let mut image = RgbImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            *pixel = Rgb([
                (x.wrapping_mul(13).wrapping_add(y.wrapping_mul(7))) as u8,
                (x.wrapping_mul(3).wrapping_add(y.wrapping_mul(11))) as u8,
                x.wrapping_mul(y).wrapping_add(x) as u8,
            ]);
        }
        image
    }

    fn write_png(dir: &std::path::Path, name: &str, image: &RgbImage) -> std::path::PathBuf {
        let path = dir.join(name);
        image.save(&path).unwrap();
        path
    }

    #[test]
    fn rejects_gif_and_svg_but_accepts_common_formats() {
        assert_eq!(
            unsupported_reason(std::path::Path::new("a.GIF")),
            Some("不支持 GIF、SVG 格式")
        );
        assert_eq!(
            unsupported_reason(std::path::Path::new("a.svg")),
            Some("不支持 GIF、SVG 格式")
        );
        assert_eq!(unsupported_reason(std::path::Path::new("a.PnG")), None);
        assert_eq!(unsupported_reason(std::path::Path::new("a.txt")), Some("只支持 PNG、JPEG、WebP、BMP"));
    }

    #[test]
    fn keeps_aspect_ratio_when_one_side_changes() {
        assert_eq!(height_for_width(2380, 1684, 1190), 842);
        assert_eq!(width_for_height(2380, 1684, 842), 1190);
    }

    #[test]
    fn output_name_stays_beside_the_source() {
        let path = output_jpeg_path(std::path::Path::new(r"D:\pics\scan.png"));
        assert_eq!(path, std::path::PathBuf::from(r"D:\pics\scan-compressed.jpg"));
        let again = output_jpeg_path(std::path::Path::new("scan-compressed.jpg"));
        assert_eq!(again, std::path::PathBuf::from("scan-compressed-2.jpg"));
    }

    #[test]
    fn high_quality_is_smaller_than_normal_and_limit_is_respected() {
        let dir = std::env::temp_dir().join(format!("pdf2img-compress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = write_png(&dir, "noisy.png", &noisy(320, 200));

        let normal_path = dir.join("normal.jpg");
        let normal = compress_to(&src, &normal_path, 320, 200, CompressMode::Normal).unwrap();
        let high_path = dir.join("high.jpg");
        let high = compress_to(&src, &high_path, 320, 200, CompressMode::HighQuality).unwrap();
        assert!(high.bytes < normal.bytes);
        assert_eq!((normal.width, normal.height), (320, 200));

        let limit = high.bytes / 2;
        let limited_path = dir.join("limited.jpg");
        let limited = compress_to(&src, &limited_path, 320, 200, CompressMode::MaxBytes(limit.max(1024))).unwrap();
        assert!(limited.bytes <= limit.max(1024));
        assert!(limited_path.exists());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
