use anyhow::{bail, Result};

/// 单边像素上限。再大的位图会让内存占用过高。
pub const MAX_EDGE: u32 = 16_384;

/// 单页像素总量上限，大约对应 400MB 的未压缩 RGBA 缓冲。
pub const MAX_PIXELS: u64 = 100_000_000;

/// 生成图片的像素宽高范围。未填写的边界不参与约束。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SizeLimits {
    pub min_width: Option<u32>,
    pub max_width: Option<u32>,
    pub min_height: Option<u32>,
    pub max_height: Option<u32>,
}

/// 按页面宽高比算出的缩放结果。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FittedSize {
    /// 相对 PDF 用户单位（1 单位 = 1/72 英寸）的缩放倍数。
    pub scale: f32,
    pub width: u32,
    pub height: u32,
    /// 为了不超过上限，结果仍小于指定的下限。
    pub below_minimum: bool,
}

/// 在保持宽高比的前提下，把页面放进给定的像素范围。
///
/// `page_width` 与 `page_height` 使用 PDF 的渲染尺寸（点）。`dpi / 72` 是首选缩放；
/// 上限会把结果缩小，下限会在不超过上限时把结果放大。
pub fn fit_page(page_width: f32, page_height: f32, dpi: f32, limits: SizeLimits) -> Result<FittedSize> {
    if !page_width.is_finite() || !page_height.is_finite() || page_width <= 0.0 || page_height <= 0.0 {
        bail!("页面尺寸无效");
    }
    if !dpi.is_finite() || dpi <= 0.0 {
        bail!("dpi 必须是大于 0 的数字");
    }
    if dpi > 9_600.0 {
        bail!("dpi 不能超过 9600");
    }
    validate_edge("最小宽度", limits.min_width)?;
    validate_edge("最大宽度", limits.max_width)?;
    validate_edge("最小高度", limits.min_height)?;
    validate_edge("最大高度", limits.max_height)?;
    if let (Some(min), Some(max)) = (limits.min_width, limits.max_width) {
        if min > max {
            bail!("最小宽度 {min} 大于最大宽度 {max}");
        }
    }
    if let (Some(min), Some(max)) = (limits.min_height, limits.max_height) {
        if min > max {
            bail!("最小高度 {min} 大于最大高度 {max}");
        }
    }

    let preferred = dpi / 72.0;
    let mut scale = preferred
        .min(scale_for_max(page_width, limits.max_width.unwrap_or(MAX_EDGE)))
        .min(scale_for_max(page_height, limits.max_height.unwrap_or(MAX_EDGE)));
    scale = scale
        .min(scale_for_max(page_width, MAX_EDGE))
        .min(scale_for_max(page_height, MAX_EDGE));
    scale = shrink_until_within(page_width, page_height, scale, limits);

    let mut below_minimum = false;
    if let Some(needed) = required_min_scale(page_width, page_height, limits) {
        if needed > scale {
            let raised = shrink_until_within(page_width, page_height, needed, limits);
            if meets_minimum(page_width, page_height, raised, limits) {
                scale = raised;
            } else {
                below_minimum = true;
                scale = largest_scale(page_width, page_height, limits);
            }
        }
    }

    scale = ensure_at_least_one_pixel(page_width, page_height, scale);
    let width = pixels(page_width, scale);
    let height = pixels(page_height, scale);
    if width == 0 || height == 0 {
        bail!("计算出的图片尺寸为 0");
    }
    if width > MAX_EDGE || height > MAX_EDGE || u64::from(width) * u64::from(height) > MAX_PIXELS {
        bail!(
            "页面按当前设置会生成 {width}x{height} 的图片，超过单边 {MAX_EDGE} 像素或单页 {MAX_PIXELS} 像素的限制。请降低 --dpi，或设置更小的 --max-width / --max-height"
        );
    }
    if !meets_minimum(page_width, page_height, scale, limits) {
        below_minimum = true;
    }

    Ok(FittedSize {
        scale,
        width,
        height,
        below_minimum,
    })
}

fn validate_edge(name: &str, value: Option<u32>) -> Result<()> {
    if let Some(value) = value {
        if value == 0 {
            bail!("{name}必须大于 0");
        }
        if value > MAX_EDGE {
            bail!("{name}不能超过 {MAX_EDGE} 像素");
        }
    }
    Ok(())
}

/// 与渲染器一致：正数向 0 截断。调用方负责把结果限制在允许范围内。
fn pixels(dimension: f32, scale: f32) -> u32 {
    let value = dimension * scale;
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    if value >= u32::MAX as f32 {
        u32::MAX
    } else {
        value as u32
    }
}

fn scale_for_max(dimension: f32, max_pixels: u32) -> f32 {
    let mut scale = max_pixels as f32 / dimension;
    for _ in 0..8 {
        if pixels(dimension, scale) <= max_pixels {
            return scale.max(0.0);
        }
        scale -= 1.0 / dimension;
    }
    scale.max(0.0)
}

fn scale_for_min(dimension: f32, min_pixels: u32) -> f32 {
    let mut scale = min_pixels as f32 / dimension;
    for _ in 0..8 {
        if pixels(dimension, scale) >= min_pixels {
            return scale;
        }
        scale += 1.0 / dimension;
    }
    scale
}

fn within_maximum(page_width: f32, page_height: f32, scale: f32, limits: SizeLimits) -> bool {
    let width = pixels(page_width, scale);
    let height = pixels(page_height, scale);
    width >= 1
        && height >= 1
        && limits.max_width.is_none_or(|max| width <= max)
        && limits.max_height.is_none_or(|max| height <= max)
        && width <= MAX_EDGE
        && height <= MAX_EDGE
        && u64::from(width) * u64::from(height) <= MAX_PIXELS
}

fn meets_minimum(page_width: f32, page_height: f32, scale: f32, limits: SizeLimits) -> bool {
    let width = pixels(page_width, scale);
    let height = pixels(page_height, scale);
    limits.min_width.is_none_or(|min| width >= min) && limits.min_height.is_none_or(|min| height >= min)
}

fn shrink_until_within(page_width: f32, page_height: f32, scale: f32, limits: SizeLimits) -> f32 {
    if within_maximum(page_width, page_height, scale, limits) {
        return scale;
    }

    let mut low = 0.0;
    let mut high = scale;
    for _ in 0..48 {
        let mid = (low + high) * 0.5;
        if within_maximum(page_width, page_height, mid, limits) {
            low = mid;
        } else {
            high = mid;
        }
    }
    low
}

fn largest_scale(page_width: f32, page_height: f32, limits: SizeLimits) -> f32 {
    let ceiling = scale_for_max(page_width, limits.max_width.unwrap_or(MAX_EDGE))
        .min(scale_for_max(page_height, limits.max_height.unwrap_or(MAX_EDGE)))
        .min(scale_for_max(page_width, MAX_EDGE))
        .min(scale_for_max(page_height, MAX_EDGE));
    shrink_until_within(page_width, page_height, ceiling, limits)
}

fn required_min_scale(page_width: f32, page_height: f32, limits: SizeLimits) -> Option<f32> {
    let width = limits.min_width.map(|min| scale_for_min(page_width, min));
    let height = limits.min_height.map(|min| scale_for_min(page_height, min));
    match (width, height) {
        (Some(width), Some(height)) => Some(width.max(height)),
        (Some(width), None) => Some(width),
        (None, Some(height)) => Some(height),
        (None, None) => None,
    }
}

fn ensure_at_least_one_pixel(page_width: f32, page_height: f32, scale: f32) -> f32 {
    if pixels(page_width, scale) >= 1 && pixels(page_height, scale) >= 1 {
        return scale;
    }
    scale.max(1.0 / page_width).max(1.0 / page_height)
}

#[cfg(test)]
mod tests {
    use super::{fit_page, SizeLimits};

    fn limits(
        min_width: Option<u32>,
        max_width: Option<u32>,
        min_height: Option<u32>,
        max_height: Option<u32>,
    ) -> SizeLimits {
        SizeLimits {
            min_width,
            max_width,
            min_height,
            max_height,
        }
    }

    #[test]
    fn dpi_sets_the_size_when_no_limits_are_given() {
        let fitted = fit_page(300.0, 200.0, 72.0, SizeLimits::default()).unwrap();
        assert_eq!((fitted.width, fitted.height), (300, 200));
        assert!(!fitted.below_minimum);

        let fitted = fit_page(300.0, 200.0, 144.0, SizeLimits::default()).unwrap();
        assert_eq!((fitted.width, fitted.height), (600, 400));
    }

    #[test]
    fn maximum_shrinks_the_page_without_stretching() {
        let fitted = fit_page(300.0, 200.0, 144.0, limits(None, Some(150), None, None)).unwrap();
        assert_eq!((fitted.width, fitted.height), (150, 100));

        let fitted = fit_page(300.0, 200.0, 72.0, limits(None, None, None, Some(50))).unwrap();
        assert_eq!((fitted.width, fitted.height), (75, 50));
    }

    #[test]
    fn minimum_grows_the_page_until_it_hits_the_maximum() {
        let fitted = fit_page(300.0, 200.0, 72.0, limits(Some(450), None, None, None)).unwrap();
        assert_eq!((fitted.width, fitted.height), (450, 300));
        assert!(!fitted.below_minimum);

        let fitted = fit_page(300.0, 200.0, 72.0, limits(Some(1000), None, None, Some(50))).unwrap();
        assert_eq!((fitted.width, fitted.height), (75, 50));
        assert!(fitted.below_minimum);
    }

    #[test]
    fn rejects_inverted_ranges_and_invalid_dpi() {
        assert!(fit_page(300.0, 200.0, 72.0, limits(Some(400), Some(100), None, None)).is_err());
        assert!(fit_page(300.0, 200.0, 0.0, SizeLimits::default()).is_err());
        assert!(fit_page(0.0, 200.0, 72.0, SizeLimits::default()).is_err());
    }
}
