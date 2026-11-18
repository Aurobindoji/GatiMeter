//! Daily traffic totals, stored as `date,upload_bytes,download_bytes` lines.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::format;
use crate::sys::Date;

pub struct History {
    path: PathBuf,
    days: BTreeMap<Date, (u64, u64)>,
    dirty: bool,
}

impl History {
    pub fn load(path: PathBuf) -> History {
        let days = std::fs::read_to_string(&path).map(|t| parse(&t)).unwrap_or_default();
        History { path, days, dirty: false }
    }

    pub fn add(&mut self, day: Date, up: u64, down: u64) {
        if up == 0 && down == 0 {
            return;
        }
        let e = self.days.entry(day).or_default();
        e.0 = e.0.saturating_add(up);
        e.1 = e.1.saturating_add(down);
        self.dirty = true;
    }

    pub fn day(&self, d: Date) -> (u64, u64) {
        self.days.get(&d).copied().unwrap_or_default()
    }

    pub fn month(&self, y: i32, m: u32) -> (u64, u64) {
        self.days
            .iter()
            .filter(|(d, _)| d.y == y && d.m == m)
            .fold((0, 0), |a, (_, v)| (a.0 + v.0, a.1 + v.1))
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        if !self.dirty {
            return Ok(());
        }
        write_atomic(&self.path, &to_text(&self.days))?;
        self.dirty = false;
        Ok(())
    }

    /// The text shown in the history window.
    pub fn report(&self, today: Date, adapter: &str) -> String {
        let mut s = String::new();
        let line = |s: &mut String, name: &str, (up, down): (u64, u64)| {
            let _ = writeln!(
                s,
                "{name:<12}{:>12}{:>12}{:>12}",
                format::bytes(up),
                format::bytes(down),
                format::bytes(up + down)
            );
        };
        let _ = writeln!(s, "Network: {adapter}");
        let _ = writeln!(s);
        let _ = writeln!(s, "{:<12}{:>12}{:>12}{:>12}", "", "Upload", "Download", "Total");
        line(&mut s, "Today", self.day(today));
        line(&mut s, "This month", self.month(today.y, today.m));
        let _ = writeln!(s);

        let mut months: BTreeMap<(i32, u32), (u64, u64)> = BTreeMap::new();
        for (d, v) in &self.days {
            let e = months.entry((d.y, d.m)).or_default();
            e.0 += v.0;
            e.1 += v.1;
        }
        let _ = writeln!(s, "By month");
        for ((y, m), v) in months.iter().rev().take(24) {
            line(&mut s, &format!("{y:04}-{m:02}"), *v);
        }
        let _ = writeln!(s);
        let _ = writeln!(s, "By day");
        for (d, v) in self.days.iter().rev().take(92) {
            line(&mut s, &d.to_string(), *v);
        }
        if self.days.is_empty() {
            let _ = writeln!(s, "(nothing recorded yet)");
        }
        s
    }
}

fn parse(text: &str) -> BTreeMap<Date, (u64, u64)> {
    let mut days = BTreeMap::new();
    for line in text.lines() {
        let mut f = line.split(',');
        let (Some(d), Some(u), Some(w)) = (f.next(), f.next(), f.next()) else { continue };
        let (Some(d), Ok(u), Ok(w)) = (Date::parse(d), u.trim().parse::<u64>(), w.trim().parse::<u64>())
        else {
            continue;
        };
        let e: &mut (u64, u64) = days.entry(d).or_default();
        e.0 = e.0.saturating_add(u);
        e.1 = e.1.saturating_add(w);
    }
    days
}

fn to_text(days: &BTreeMap<Date, (u64, u64)>) -> String {
    let mut s = String::from("date,upload_bytes,download_bytes\r\n");
    for (d, (u, w)) in days {
        let _ = write!(s, "{d},{u},{w}\r\n");
    }
    s
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("csv.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_sum() {
        let t = "date,upload_bytes,download_bytes\n2026-11-01,10,20\n2026-11-02,1,2\nbad line\n2026-10-31,5,5\n";
        let days = parse(t);
        assert_eq!(days.len(), 3);
        let h = History { path: PathBuf::new(), days, dirty: false };
        assert_eq!(h.month(2026, 11), (11, 22));
        assert_eq!(h.day(Date { y: 2026, m: 10, d: 31 }), (5, 5));
        assert_eq!(parse(&to_text(&h.days)), h.days);
    }

    #[test]
    fn add_marks_dirty() {
        let mut h = History { path: PathBuf::new(), days: BTreeMap::new(), dirty: false };
        h.add(Date { y: 2026, m: 1, d: 1 }, 0, 0);
        assert!(!h.dirty);
        h.add(Date { y: 2026, m: 1, d: 1 }, 3, 4);
        h.add(Date { y: 2026, m: 1, d: 1 }, 1, 1);
        assert!(h.dirty);
        assert_eq!(h.day(Date { y: 2026, m: 1, d: 1 }), (4, 5));
    }
}
