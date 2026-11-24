//! Text formatting of sizes, dates and size arguments.

/// Size in decimal units, as the desktop app shows them.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut v = bytes as f64 / 1000.0;
    let mut i = 0;
    while v >= 1000.0 && i < UNITS.len() - 1 {
        v /= 1000.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{v:.0} {}", UNITS[i])
    } else if v >= 10.0 {
        format!("{v:.1} {}", UNITS[i])
    } else {
        format!("{v:.2} {}", UNITS[i])
    }
}

/// `YYYY-MM-DD` in UTC, or `unknown` for a missing date.
pub fn date(ts: i64) -> String {
    if ts <= 0 {
        return "unknown".into();
    }
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = ts.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Count with thin grouping, `2 691 346`.
pub fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// Bytes from a number or a string such as `500MB`, `1.5 GB`, `2G`, `10k`.
/// Units are decimal; `GiB`-style units are binary.
pub fn parse_size(text: &str) -> Option<u64> {
    let t = text.trim();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let (num, unit) = t.split_at(split);
    let value: f64 = num.parse().ok()?;
    let unit = unit.trim().to_ascii_lowercase();
    let factor: f64 = match unit.as_str() {
        "" | "b" => 1.0,
        "k" | "kb" => 1e3,
        "m" | "mb" => 1e6,
        "g" | "gb" => 1e9,
        "t" | "tb" => 1e12,
        "kib" => 1024.0,
        "mib" => 1024.0 * 1024.0,
        "gib" => 1024.0 * 1024.0 * 1024.0,
        "tib" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    (value >= 0.0).then(|| (value * factor) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1_500), "1.50 KB");
        assert_eq!(size(598_000_000_000), "598 GB");
        assert_eq!(size(1_450_000_000_000), "1.45 TB");
    }

    #[test]
    fn dates() {
        assert_eq!(date(0), "unknown");
        assert_eq!(date(86_400), "1970-01-02");
        assert_eq!(date(1_690_329_600), "2023-07-26");
        assert_eq!(date(1_709_251_200), "2024-03-01");
    }

    #[test]
    fn counts() {
        assert_eq!(count(7), "7");
        assert_eq!(count(1000), "1 000");
        assert_eq!(count(2_691_346), "2 691 346");
    }

    #[test]
    fn parsed_sizes() {
        assert_eq!(parse_size("500MB"), Some(500_000_000));
        assert_eq!(parse_size("1.5 GB"), Some(1_500_000_000));
        assert_eq!(parse_size("2g"), Some(2_000_000_000));
        assert_eq!(parse_size("1GiB"), Some(1_073_741_824));
        assert_eq!(parse_size("42"), Some(42));
        assert_eq!(parse_size("lots"), None);
        assert_eq!(parse_size("3 parsecs"), None);
    }
}
