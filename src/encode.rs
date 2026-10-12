use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result};
use image::codecs::bmp::BmpEncoder;
use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, ImageEncoder};

use crate::OutputFormat;

/// 把预乘 alpha 的 RGBA8 位图写成目标格式。
pub fn write_image(
    path: &Path,
    format: OutputFormat,
    width: u32,
    height: u32,
    premul_rgba: &[u8],
    quality: u8,
) -> Result<()> {
    if premul_rgba.len() != width as usize * height as usize * 4 {
        anyhow::bail!("像素缓冲长度与图片尺寸不一致");
    }

    let file = File::create(path)
        .with_context(|| format!("无法创建图片: {}", path.display()))?;
    let mut writer = BufWriter::new(file);

    let result = match format {
        OutputFormat::Png => {
            let rgba = unpremultiply_rgba(premul_rgba);
            PngEncoder::new(&mut writer).write_image(&rgba, width, height, ExtendedColorType::Rgba8)
        }
        OutputFormat::Jpeg => {
            let rgb = premul_over_white_rgb(premul_rgba);
            JpegEncoder::new_with_quality(&mut writer, quality)
                .write_image(&rgb, width, height, ExtendedColorType::Rgb8)
        }
        OutputFormat::Webp => {
            let rgba = unpremultiply_rgba(premul_rgba);
            WebPEncoder::new_lossless(&mut writer)
                .write_image(&rgba, width, height, ExtendedColorType::Rgba8)
        }
        OutputFormat::Bmp => {
            let rgb = premul_over_white_rgb(premul_rgba);
            BmpEncoder::new(&mut writer).write_image(&rgb, width, height, ExtendedColorType::Rgb8)
        }
    };

    if let Err(error) = result {
        drop(writer);
        let _ = std::fs::remove_file(path);
        return Err(error).with_context(|| format!("无法编码图片: {}", path.display()));
    }

    writer
        .into_inner()
        .context("写入图片时出错")?
        .sync_all()
        .with_context(|| format!("无法保存图片: {}", path.display()))?;
    Ok(())
}

fn unpremultiply_rgba(src: &[u8]) -> Vec<u8> {
    let mut out = vec![0; src.len()];
    for (src_px, dst_px) in src.chunks_exact(4).zip(out.chunks_exact_mut(4)) {
        let alpha = src_px[3] as u16;
        if alpha == 0 {
            continue;
        }
        dst_px[0] = ((src_px[0] as u16 * 255) / alpha) as u8;
        dst_px[1] = ((src_px[1] as u16 * 255) / alpha) as u8;
        dst_px[2] = ((src_px[2] as u16 * 255) / alpha) as u8;
        dst_px[3] = src_px[3];
    }
    out
}

/// 预乘颜色覆盖在白底上，得到不含 alpha 的 RGB。
fn premul_over_white_rgb(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() / 4 * 3);
    for pixel in src.chunks_exact(4) {
        let inverse_alpha = 255 - pixel[3] as u16;
        out.push((pixel[0] as u16 + inverse_alpha) as u8);
        out.push((pixel[1] as u16 + inverse_alpha) as u8);
        out.push((pixel[2] as u16 + inverse_alpha) as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{premul_over_white_rgb, unpremultiply_rgba};

    #[test]
    fn unpremultiply_restores_straight_color() {
        let premul = [128, 0, 0, 128];
        let straight = unpremultiply_rgba(&premul);
        assert_eq!(straight, [255, 0, 0, 128]);
    }

    #[test]
    fn transparent_pixel_becomes_white() {
        assert_eq!(premul_over_white_rgb(&[0, 0, 0, 0]), vec![255, 255, 255]);
    }

    #[test]
    fn opaque_pixel_keeps_its_color() {
        assert_eq!(premul_over_white_rgb(&[10, 20, 30, 255]), vec![10, 20, 30]);
    }

    #[test]
    fn writes_png_jpeg_webp_and_bmp() {
        let premul = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 0,
        ];
        let dir = std::env::temp_dir().join(format!("pdf2img-encode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        for format in [
            crate::OutputFormat::Png,
            crate::OutputFormat::Jpeg,
            crate::OutputFormat::Webp,
            crate::OutputFormat::Bmp,
        ] {
            let path = dir.join(format!("sample.{}", format.extension()));
            super::write_image(&path, format, 2, 2, &premul, 80).unwrap();
            assert!(std::fs::metadata(&path).unwrap().len() > 24);
        }

        let _ = std::fs::remove_dir_all(dir);
    }
}
