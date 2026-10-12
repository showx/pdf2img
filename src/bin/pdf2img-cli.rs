use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;
use pdf2img::{convert_pdf, ConvertOptions, OutputFormat, SizeLimits};

#[derive(Debug, Parser)]
#[command(
    name = "pdf2img-cli",
    version,
    about = "把 PDF 每一页转换成图片，并保存到该 PDF 所在目录",
    after_long_help = "\
大小范围按像素指定，页面会按原始宽高比缩放，不会被拉变形。
未设置上限时，按 --dpi 渲染（默认 144，即每点 2 像素）；设置上限后不会超过该上限。
设置下限后，会在不超过上限的前提下尽量放大到下限。若宽高比使两者无法同时满足，则保持在上限内，并提示实际尺寸。

图形界面请直接运行 pdf2img。

示例:
  pdf2img-cli notes.pdf
  pdf2img-cli notes.pdf -f jpeg --max-width 1920 --max-height 1080
  pdf2img-cli notes.pdf -f png --min-width 800 --max-width 1600 --pages 1-3
  pdf2img-cli notes.pdf -f webp --dpi 200 --pages 1,4,6-8"
)]
struct Cli {
    /// PDF 文件路径
    pdf: PathBuf,

    /// 图片格式：png、jpeg、jpg、webp、bmp
    #[arg(short, long, value_enum, default_value_t = OutputFormat::Png)]
    format: OutputFormat,

    /// 生成图片的最小宽度，单位像素
    #[arg(long, value_parser = parse_edge)]
    min_width: Option<u32>,

    /// 生成图片的最大宽度，单位像素
    #[arg(long, value_parser = parse_edge)]
    max_width: Option<u32>,

    /// 生成图片的最小高度，单位像素
    #[arg(long, value_parser = parse_edge)]
    min_height: Option<u32>,

    /// 生成图片的最大高度，单位像素
    #[arg(long, value_parser = parse_edge)]
    max_height: Option<u32>,

    /// 未达到尺寸上限时使用的分辨率。默认 144
    #[arg(long, default_value_t = 144.0, value_parser = parse_dpi)]
    dpi: f32,

    /// JPEG 质量，1 到 100。webp 使用无损编码，不使用该参数
    #[arg(short, long, default_value_t = 85, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,

    /// 要转换的页码，从 1 开始。例如 1、1-3、1,4,6-8。默认全部页面
    #[arg(long)]
    pages: Option<String>,

    /// 加密 PDF 的打开密码
    #[arg(long)]
    password: Option<String>,
}

fn parse_edge(text: &str) -> Result<u32, String> {
    let value: u32 = text.parse().map_err(|_| format!("无法识别的像素值: {text}"))?;
    if value == 0 {
        return Err("像素值必须大于 0".to_string());
    }
    Ok(value)
}

fn parse_dpi(text: &str) -> Result<f32, String> {
    let value: f32 = text.parse().map_err(|_| format!("无法识别的 dpi: {text}"))?;
    if !value.is_finite() || value <= 0.0 {
        return Err("dpi 必须是大于 0 的数字".to_string());
    }
    Ok(value)
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("错误: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let saved = convert_pdf(
        &cli.pdf,
        &ConvertOptions {
            format: cli.format,
            limits: SizeLimits {
                min_width: cli.min_width,
                max_width: cli.max_width,
                min_height: cli.min_height,
                max_height: cli.max_height,
            },
            dpi: cli.dpi,
            quality: cli.quality,
            pages: cli.pages,
            password: cli.password,
        },
    )
    .with_context(|| format!("转换失败: {}", cli.pdf.display()))?;

    for image in &saved {
        println!(
            "已保存 {} ({}x{})",
            image.path.display(),
            image.width,
            image.height
        );
        if image.below_minimum {
            eprintln!(
                "提示: 第 {} 页在不超过上限的情况下无法同时达到最小宽高，实际尺寸为 {}x{}",
                image.page, image.width, image.height
            );
        }
    }
    println!("完成，共 {} 页", saved.len());
    Ok(())
}
