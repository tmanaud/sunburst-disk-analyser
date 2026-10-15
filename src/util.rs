//! Formatting helpers shared by the UI and the CSV export.

/// Splits a size in a 3-significant-digit number and its decimal unit,
/// e.g. `("6.56", "GB")`, matching how Finder and Explorer present sizes.
pub fn size_parts(bytes: u64) -> (String, &'static str) {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes == 0 {
        return ("0".into(), "KB");
    }
    if bytes < 1000 {
        return (bytes.to_string(), "B");
    }
    let mut v = bytes as f64 / 1000.0;
    let mut u = 0;
    while v >= 999.5 && u < UNITS.len() - 1 {
        v /= 1000.0;
        u += 1;
    }
    let s = if v >= 99.95 {
        format!("{v:.0}")
    } else if v >= 9.995 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    };
    (s, UNITS[u])
}

pub fn fmt_size(bytes: u64) -> String {
    let (n, u) = size_parts(bytes);
    format!("{n} {u}")
}

/// 0 = bytes/KB, 1 = MB, 2 = hundreds of MB, 3 = GB and more.
pub fn size_class(bytes: u64) -> u8 {
    match bytes {
        b if b >= 1_000_000_000 => 3,
        b if b >= 500_000_000 => 2,
        b if b >= 1_000_000 => 1,
        _ => 0,
    }
}

pub fn fmt_count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Formats a Unix timestamp in local time; `long` gives "27 Dec 2024".
pub fn fmt_date(secs: i64, long: bool) -> String {
    use chrono::TimeZone;
    if secs <= 0 {
        return "—".into();
    }
    match chrono::Local.timestamp_opt(secs, 0).single() {
        Some(d) if long && crate::i18n::lang() == crate::i18n::Lang::En => d.format("%-d %b %Y").to_string(),
        Some(d) => d.format(crate::i18n::date_format()).to_string(),
        None => "—".into(),
    }
}

/// Shortens `text` to at most `max_chars` characters by replacing its middle
/// with an ellipsis, keeping the extension visible.
pub fn middle_ellipsis(text: &str, max_chars: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max_chars || max_chars < 5 {
        return text.to_string();
    }
    let keep = max_chars - 1;
    let tail = (keep * 2 / 5).max(2);
    let head = keep - tail;
    let mut s: String = chars[..head].iter().collect();
    s.push('…');
    s.extend(chars[chars.len() - tail..].iter());
    s
}

/// Quotes a CSV field and neutralises spreadsheet formula prefixes.
pub fn csv_field(s: &str) -> String {
    let mut v = s.to_string();
    if v.starts_with(['=', '+', '-', '@', '\t', '\r']) {
        v.insert(0, '\'');
    }
    if v.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", v.replace('"', "\"\""))
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(fmt_size(0), "0 KB");
        assert_eq!(fmt_size(512), "512 B");
        assert_eq!(fmt_size(12_300), "12.3 KB");
        assert_eq!(fmt_size(1_610_000), "1.61 MB");
        assert_eq!(fmt_size(99_000_000_000), "99.0 GB");
        assert_eq!(fmt_size(312_000_000_000), "312 GB");
        assert_eq!(fmt_size(999_900_000), "1.00 GB");
        assert_eq!(fmt_size(1_600_000_000_000), "1.60 TB");
        assert_eq!(size_class(2_000_000_000), 3);
        assert_eq!(size_class(64_400_000), 1);
    }

    #[test]
    fn ellipsis_and_csv() {
        assert_eq!(middle_ellipsis("short.txt", 20), "short.txt");
        let s = middle_ellipsis("archive-backup-0064.bin", 16);
        assert_eq!(s.chars().count(), 16);
        assert!(s.ends_with("64.bin") && s.contains('…'));
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("=cmd()"), "'=cmd()");
        assert_eq!(fmt_count(1234567), "1 234 567");
    }
}
