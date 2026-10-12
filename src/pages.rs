/// 解析从 1 开始的页码表达式，例如 `1`、`1-3`、`1,4,6-8`。
/// 结果按页码升序去重。
pub fn parse_pages(spec: &str) -> Result<Vec<usize>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("页码不能为空".to_string());
    }

    let mut pages = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err(format!("页码格式不正确: {spec}"));
        }

        if let Some((start, end)) = part.split_once('-') {
            let start = parse_page_number(start)?;
            let end = parse_page_number(end)?;
            if end < start {
                return Err(format!("页码范围 {start}-{end} 的结束页小于起始页"));
            }
            if end - start > 100_000 {
                return Err("页码范围过大".to_string());
            }
            pages.extend(start..=end);
        } else {
            pages.push(parse_page_number(part)?);
        }
    }

    pages.sort_unstable();
    pages.dedup();
    Ok(pages)
}

fn parse_page_number(text: &str) -> Result<usize, String> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("无法识别的页码: {text}"));
    }

    let page = text
        .parse::<usize>()
        .map_err(|_| format!("无法识别的页码: {text}"))?;
    if page == 0 {
        return Err("页码从 1 开始".to_string());
    }
    Ok(page)
}

#[cfg(test)]
mod tests {
    use super::parse_pages;

    #[test]
    fn parses_single_ranges_and_lists() {
        assert_eq!(parse_pages("1").unwrap(), vec![1]);
        assert_eq!(parse_pages("1-3").unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_pages("1,4,6-8").unwrap(), vec![1, 4, 6, 7, 8]);
        assert_eq!(parse_pages(" 5, 1-2, 2 ").unwrap(), vec![1, 2, 5]);
        assert_eq!(parse_pages("01").unwrap(), vec![1]);
    }

    #[test]
    fn rejects_invalid_specs() {
        assert!(parse_pages("").is_err());
        assert!(parse_pages("0").is_err());
        assert!(parse_pages("2-1").is_err());
        assert!(parse_pages("1-").is_err());
        assert!(parse_pages("a").is_err());
        assert!(parse_pages("1,,2").is_err());
    }
}
