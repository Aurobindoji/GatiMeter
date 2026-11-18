//! Text formatting for speeds, byte counts, percentages and temperatures.

/// `bytes_per_sec` as "12.3 KB/s" (bytes, base 1024) or "98.4 Kb/s" (bits, base 1000).
pub fn speed(bytes_per_sec: f64, bits: bool) -> String {
    let (mut v, base, units) = if bits {
        (bytes_per_sec * 8.0, 1000.0, ["b/s", "Kb/s", "Mb/s", "Gb/s", "Tb/s"])
    } else {
        (bytes_per_sec, 1024.0, ["B/s", "KB/s", "MB/s", "GB/s", "TB/s"])
    };
    if !v.is_finite() || v < 0.0 {
        v = 0.0;
    }
    let mut i = 0;
    // Switch unit before the text would grow to four digits ("1000.0").
    while i < units.len() - 1 && (if i == 0 { v >= 999.5 } else { v >= 999.95 }) {
        v /= base;
        i += 1;
    }
    if i == 0 { format!("{v:.0} {}", units[0]) } else { format!("{v:.1} {}", units[i]) }
}

/// The widest text `speed` produces, used to keep the column width steady.
pub fn speed_template(bits: bool) -> &'static str {
    if bits { "888.8 Mb/s" } else { "888.8 MB/s" }
}

/// Totals such as "1.23 GB" (base 1024).
pub fn bytes(n: u64) -> String {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    match i {
        0 => format!("{n} B"),
        _ if v >= 100.0 => format!("{v:.0} {}", UNITS[i]),
        _ if v >= 10.0 => format!("{v:.1} {}", UNITS[i]),
        _ => format!("{v:.2} {}", UNITS[i]),
    }
}

pub fn percent(p: Option<f32>) -> String {
    match p {
        Some(p) => format!("{:.0}%", p.clamp(0.0, 100.0)),
        None => "--".into(),
    }
}

pub fn temp(c: Option<f32>) -> String {
    match c {
        Some(c) => format!("{c:.0}°C"),
        None => "--".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_bytes() {
        assert_eq!(speed(0.0, false), "0 B/s");
        assert_eq!(speed(512.0, false), "512 B/s");
        assert_eq!(speed(999.4, false), "999 B/s");
        assert_eq!(speed(999.6, false), "1.0 KB/s");
        assert_eq!(speed(12.3 * 1024.0, false), "12.3 KB/s");
        assert_eq!(speed(999.96 * 1024.0, false), "1.0 MB/s");
        assert_eq!(speed(5.5 * 1024.0 * 1024.0, false), "5.5 MB/s");
        assert_eq!(speed(-3.0, false), "0 B/s");
        assert_eq!(speed(f64::NAN, false), "0 B/s");
    }

    #[test]
    fn speed_bits() {
        assert_eq!(speed(125.0, true), "1.0 Kb/s");
        assert_eq!(speed(12_500_000.0, true), "100.0 Mb/s");
    }

    #[test]
    fn totals() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(1023), "1023 B");
        assert_eq!(bytes(1536), "1.50 KB");
        assert_eq!(bytes(15 * 1024 * 1024), "15.0 MB");
        assert_eq!(bytes(300 * 1024 * 1024 * 1024), "300 GB");
    }

    #[test]
    fn pct_and_temp() {
        assert_eq!(percent(Some(12.4)), "12%");
        assert_eq!(percent(Some(130.0)), "100%");
        assert_eq!(percent(None), "--");
        assert_eq!(temp(Some(45.6)), "46°C");
        assert_eq!(temp(None), "--");
    }
}
