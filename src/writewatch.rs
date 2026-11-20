//! SSD write watch (optional, off by default): warns when one program keeps
//! writing a lot to the SSDs, the way some display drivers have done with a
//! log file that they rewrite over and over.
//!
//! Without admin rights Windows tells neither which file a program writes nor
//! how much of its writing reaches a disk. So two numbers are compared:
//!
//! * Every process's write I/O counter, read for all processes in one
//!   `NtQuerySystemInformation(SystemProcessInformation)` call: the I/O
//!   counters of the documented structure, the same values that
//!   `GetProcessIoCounters` and Task Manager's "I/O write bytes" give. No
//!   process is opened, so services that run as SYSTEM are included. These
//!   counters also count pipes and some network traffic, so on their own they
//!   can be far above what reached the disk.
//! * What the SSDs really wrote: PDH `\PhysicalDisk(*)\Disk Write Bytes/sec`
//!   for the disks without seek penalty (`StorageDeviceSeekPenaltyProperty`).
//!
//! The SSD total of each 10-second step is then shared out among the steady
//! writers, the most
//! believable first: a counter that stays within the SSD total goes before one
//! that runs above it (pipes), and a bigger writer before a smaller one. A
//! writer takes `min(its bytes, what is left)` from each step, but only if its
//! bytes fit what is left (at most twice that plus 1 MB) in three of four
//! of its steps. What it took is its estimated share of the SSD writes.
//!
//! A steady writer wrote something in at least two of every three minutes of
//! the window, and no single minute holds more than half of its bytes. A
//! service that rewrites a whole log file every half minute counts (it may be
//! idle in most 10-second steps); one big file copy does not.
//!
//! A process is a suspect when, over the last 15 minutes, its share reaches
//! the chosen limit.

use std::collections::{HashMap, VecDeque};
use std::fmt::Write as _;
use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

use windows_sys::Wdk::System::SystemInformation::{
    NtQuerySystemInformation, SystemProcessInformation,
};
use windows_sys::Win32::System::Ioctl::StorageDeviceSeekPenaltyProperty;

use crate::format;
use crate::temps::{MAX_DRIVES, query_drive_property};

/// One step of the window.
pub const STEP: Duration = Duration::from_secs(10);
/// 90 steps of 10 s: the last 15 minutes.
pub const WINDOW_STEPS: usize = 90;
/// Steps per minute (10-second steps).
const STEPS_PER_MINUTE: usize = 6;
/// A longer step means the PC slept or the sampler stalled: start over.
const MAX_STEP: Duration = Duration::from_secs(60);
/// Look for SSDs again now and then (a USB SSD may be plugged in).
const RESCAN: Duration = Duration::from_secs(600);
/// Processes kept in a summary, busiest first.
const TOP: usize = 12;
pub const GB: u64 = 1024 * 1024 * 1024;
pub const LOG_FILE: &str = "write_alerts.csv";
/// Idle (0) and System (4) are listed but never warned about.
const LAST_KERNEL_PID: u32 = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct ProcCounter {
    pub pid: u32,
    pub name: String,
    /// Bytes written since the process started (all write I/O).
    pub written: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WriteEntry {
    pub pid: u32,
    pub name: String,
    /// Write I/O counted in the window.
    pub bytes: u64,
    /// Steps of the window in which it wrote anything.
    pub active: usize,
    /// Minutes of the window in which it wrote anything.
    pub minutes: usize,
    /// Estimated part of the SSD writes that came from it (0: not a steady
    /// writer, or its counter does not match the SSD writes).
    pub disk_share: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WriteSummary {
    /// Length of the window in seconds.
    pub secs: f64,
    pub steps: usize,
    /// True once the window covers all 15 minutes.
    pub full: bool,
    /// Bytes the watched disks wrote in the window.
    pub disk: u64,
    /// The watched disks, as performance-counter names ("0 C:").
    pub disks: Vec<String>,
    pub top: Vec<WriteEntry>,
}

// ------------------------------------------------------------ the window

struct Step {
    secs: f64,
    disk: u64,
    /// (pid, name, bytes) for every process that wrote in this step.
    procs: Vec<(u32, String, u64)>,
}

/// The last `cap` steps, and every process's counter at the end of the newest.
pub struct Window {
    cap: usize,
    steps: VecDeque<Step>,
    prev: HashMap<u32, (String, u64)>,
}

impl Window {
    pub fn new(cap: usize) -> Window {
        Window { cap, steps: VecDeque::new(), prev: HashMap::new() }
    }

    /// Remembers the counters without adding a step.
    pub fn prime(&mut self, now: &[ProcCounter]) {
        self.prev = now.iter().map(|p| (p.pid, (p.name.clone(), p.written))).collect();
    }

    /// Adds a step `secs` long in which the watched disks wrote `disk` bytes;
    /// `now` holds every process's counter at its end. A process seen for the
    /// first time (or a process id reused by another program) adds nothing.
    pub fn push(&mut self, secs: f64, disk: u64, now: &[ProcCounter]) {
        let mut procs = Vec::new();
        for p in now {
            if let Some((name, before)) = self.prev.get(&p.pid)
                && *name == p.name
                && p.written > *before
            {
                procs.push((p.pid, p.name.clone(), p.written - before));
            }
        }
        self.prime(now);
        self.steps.push_back(Step { secs, disk, procs });
        while self.steps.len() > self.cap {
            self.steps.pop_front();
        }
    }

    pub fn summary(&self) -> WriteSummary {
        // Per process: (step index, bytes) for every step it wrote in.
        let mut per: HashMap<(u32, &str), Vec<(usize, u64)>> = HashMap::new();
        for (i, s) in self.steps.iter().enumerate() {
            for (pid, name, bytes) in &s.procs {
                per.entry((*pid, name.as_str())).or_default().push((i, *bytes));
            }
        }
        let disk: Vec<u64> = self.steps.iter().map(|s| s.disk).collect();
        let mut all: Vec<(WriteEntry, Vec<(usize, u64)>)> = per
            .into_iter()
            .map(|((pid, name), v)| {
                let e = WriteEntry {
                    pid,
                    name: name.to_string(),
                    bytes: v.iter().map(|x| x.1).sum(),
                    active: v.len(),
                    minutes: per_minute(&v, self.steps.len()).iter().filter(|&&b| b > 0).count(),
                    disk_share: 0,
                };
                (e, v)
            })
            .collect();
        share_out(&mut all, &disk);
        let mut top: Vec<WriteEntry> = all.into_iter().map(|(e, _)| e).collect();
        top.sort_by(|a, b| {
            b.disk_share.cmp(&a.disk_share).then(b.bytes.cmp(&a.bytes)).then_with(|| a.name.cmp(&b.name))
        });
        top.truncate(TOP);
        WriteSummary {
            secs: self.steps.iter().map(|s| s.secs).sum(),
            steps: self.steps.len(),
            full: self.steps.len() >= self.cap,
            disk: self.steps.iter().map(|s| s.disk).sum(),
            disks: Vec::new(),
            top,
        }
    }
}

/// Bytes a process wrote in one step can have reached the disk when they are
/// at most twice what the disk wrote (not yet claimed) plus 1 MB. The slack
/// covers writes the cache flushed a moment later and rewrites the cache
/// absorbed; pipe traffic far above the disk total does not fit.
fn fits(bytes: u64, left: u64) -> bool {
    bytes <= left.saturating_mul(2).saturating_add(1024 * 1024)
}

/// Minutes in a window of `steps` steps (the last one may be short).
pub fn window_minutes(steps: usize) -> usize {
    steps.div_ceil(STEPS_PER_MINUTE)
}

/// A process's bytes added up per minute of the window.
fn per_minute(v: &[(usize, u64)], steps: usize) -> Vec<u64> {
    let mut m = vec![0u64; window_minutes(steps)];
    for &(i, b) in v {
        if let Some(slot) = m.get_mut(i / STEPS_PER_MINUTE) {
            *slot += b;
        }
    }
    m
}

/// Wrote in at least two of every three minutes, with no minute holding more
/// than half of its bytes (see the module notes).
fn steady(v: &[(usize, u64)], steps: usize) -> bool {
    let m = per_minute(v, steps);
    let active = m.iter().filter(|&&b| b > 0).count();
    let total: u64 = m.iter().sum();
    let biggest = m.iter().copied().max().unwrap_or(0);
    total > 0 && active * 3 >= m.len() * 2 && biggest.saturating_mul(2) <= total
}

/// Fills `disk_share` of the steady writers; see the module notes.
fn share_out(all: &mut [(WriteEntry, Vec<(usize, u64)>)], disk: &[u64]) {
    let n = disk.len();
    let overshoot = |v: &[(usize, u64)]| v.iter().map(|&(i, b)| b.saturating_sub(disk[i])).sum::<u64>();
    let mut order: Vec<usize> = (0..all.len())
        .filter(|&k| all[k].0.pid > LAST_KERNEL_PID && steady(&all[k].1, n))
        .collect();
    order.sort_by_key(|&k| (overshoot(&all[k].1), std::cmp::Reverse(all[k].0.bytes), all[k].0.pid));
    let mut left = disk.to_vec();
    for k in order {
        let (e, v) = &mut all[k];
        let fit = v.iter().filter(|&&(i, b)| fits(b, left[i])).count();
        if fit * 4 < e.active * 3 {
            continue;
        }
        for &(i, b) in v.iter() {
            let take = b.min(left[i]);
            left[i] -= take;
            e.disk_share += take;
        }
    }
}

/// `bytes` written in `secs` seconds, scaled to one hour.
pub fn per_hour(bytes: u64, secs: f64) -> u64 {
    if secs <= 0.0 { 0 } else { (bytes as f64 * 3600.0 / secs) as u64 }
}

/// Processes whose estimated share of the SSD writes over the full window
/// reaches `limit_per_hour`.
pub fn suspects(s: &WriteSummary, limit_per_hour: u64) -> Vec<&WriteEntry> {
    if !s.full || s.secs <= 0.0 {
        return Vec::new();
    }
    let need = ((limit_per_hour as f64 * s.secs / 3600.0) as u64).max(1);
    s.top.iter().filter(|e| e.disk_share >= need).collect()
}

// ------------------------------------------------------ reading Windows

/// The leading disk number of a `\PhysicalDisk` instance ("1 D: E:" -> 1).
pub fn disk_number(instance: &str) -> Option<u32> {
    instance.split_whitespace().next()?.parse().ok()
}

/// The per-disk write rates to add up: the SSDs, or every disk when no SSD
/// was found. `_Total` is never included.
pub fn watched<'a>(rates: &'a [(String, f64)], ssd: &[u32]) -> Vec<&'a (String, f64)> {
    rates
        .iter()
        .filter(|(name, _)| disk_number(name).is_some_and(|n| ssd.is_empty() || ssd.contains(&n)))
        .collect()
}

/// `\\.\PhysicalDriveN` numbers of the drives without seek penalty (SSDs).
pub fn ssd_drives() -> Vec<u32> {
    (0..MAX_DRIVES).filter(|&i| is_ssd(i) == Some(true)).collect()
}

fn is_ssd(index: u32) -> Option<bool> {
    // DEVICE_SEEK_PENALTY_DESCRIPTOR: Version (u32), Size (u32), IncursSeekPenalty (u8).
    let mut buf = [0u64; 4];
    let got = query_drive_property(index, StorageDeviceSeekPenaltyProperty, &mut buf)?;
    if got < 9 {
        return None;
    }
    Some(buf[1] & 0xFF == 0)
}

#[repr(C)]
struct UnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *const u16,
}

/// `SYSTEM_PROCESS_INFORMATION` as winternl.h documents it. `io` is its
/// `Reserved7`: read, write and other operation counts, then read, write and
/// other byte counts (`IO_COUNTERS`).
#[repr(C)]
#[allow(dead_code)]
struct ProcInfo {
    next_entry_offset: u32,
    number_of_threads: u32,
    reserved1: [u8; 48],
    image_name: UnicodeString,
    base_priority: i32,
    unique_process_id: usize,
    reserved2: usize,
    handle_count: u32,
    session_id: u32,
    reserved3: usize,
    peak_virtual_size: usize,
    virtual_size: usize,
    reserved4: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    reserved5: usize,
    quota_paged_pool_usage: usize,
    reserved6: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
    private_page_count: usize,
    io: [i64; 6],
}

#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(size_of::<ProcInfo>() == 0x100);
    assert!(std::mem::offset_of!(ProcInfo, unique_process_id) == 0x50);
    assert!(std::mem::offset_of!(ProcInfo, io) == 0xD0);
};

const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;
const IO_WRITE_BYTES: usize = 4;

/// Every process's write I/O counter. `buf` is reused between calls.
pub fn read_processes(buf: &mut Vec<u64>) -> Option<Vec<ProcCounter>> {
    if buf.len() < 64 * 1024 {
        buf.resize(64 * 1024, 0); // 512 KB, enough for a few hundred processes
    }
    for _ in 0..4 {
        let size = (buf.len() * 8).min(u32::MAX as usize);
        let mut need = 0u32;
        let st = unsafe {
            NtQuerySystemInformation(SystemProcessInformation, buf.as_mut_ptr().cast(), size as u32, &mut need)
        };
        if st == STATUS_INFO_LENGTH_MISMATCH {
            let bytes = (need as usize).max(size) + 128 * 1024;
            buf.resize(bytes.div_ceil(8), 0);
            continue;
        }
        if st < 0 {
            return None;
        }
        return Some(unsafe { parse(buf, size) });
    }
    None
}

/// # Safety
/// `buf` must hold what `NtQuerySystemInformation(SystemProcessInformation)`
/// wrote into its first `len` bytes.
unsafe fn parse(buf: &[u64], len: usize) -> Vec<ProcCounter> {
    let base = buf.as_ptr() as *const u8;
    let (start, end) = (base as usize, base as usize + len);
    let mut out = Vec::new();
    let mut off = 0usize;
    while off + size_of::<ProcInfo>() <= len {
        let p = unsafe { &*(base.add(off) as *const ProcInfo) };
        let s = &p.image_name;
        let (ptr, bytes) = (s.buffer as usize, s.length as usize);
        let name = if s.buffer.is_null() || ptr < start || ptr + bytes > end {
            if p.unique_process_id == 0 { "Idle".to_string() } else { String::new() }
        } else {
            String::from_utf16_lossy(unsafe { std::slice::from_raw_parts(s.buffer, bytes / 2) })
        };
        out.push(ProcCounter {
            pid: p.unique_process_id as u32,
            name,
            written: p.io[IO_WRITE_BYTES].max(0) as u64,
        });
        if p.next_entry_offset == 0 {
            break;
        }
        off += p.next_entry_offset as usize;
    }
    out
}

/// Lives on the sampler thread while the watch is on.
pub struct WriteWatch {
    window: Window,
    ssd: Vec<u32>,
    scanned: Option<Instant>,
    disk_bytes: f64,
    last: Option<Instant>,
    step_start: Instant,
    primed: bool,
    buf: Vec<u64>,
}

impl WriteWatch {
    pub fn new() -> WriteWatch {
        WriteWatch {
            window: Window::new(WINDOW_STEPS),
            ssd: Vec::new(),
            scanned: None,
            disk_bytes: 0.0,
            last: None,
            step_start: Instant::now(),
            primed: false,
            buf: Vec::new(),
        }
    }

    /// Call after every performance-counter collect with the per-disk
    /// `Disk Write Bytes/sec` values. Returns a summary at the end of each step.
    pub fn sample(&mut self, rates: &[(String, f64)]) -> Option<WriteSummary> {
        let now = Instant::now();
        if self.scanned.is_none_or(|t| now.duration_since(t) >= RESCAN) {
            self.ssd = ssd_drives();
            self.scanned = Some(now);
        }
        let watched = watched(rates, &self.ssd);
        if let Some(last) = self.last {
            let dt = now.duration_since(last).as_secs_f64();
            self.disk_bytes += watched.iter().map(|(_, r)| r.max(0.0)).sum::<f64>() * dt;
        }
        self.last = Some(now);

        let step = now.duration_since(self.step_start);
        if !self.primed || step > MAX_STEP {
            let procs = read_processes(&mut self.buf)?;
            self.window = Window::new(WINDOW_STEPS);
            self.window.prime(&procs);
            self.primed = true;
            self.step_start = now;
            self.disk_bytes = 0.0;
            return None;
        }
        if step < STEP {
            return None;
        }
        let procs = read_processes(&mut self.buf)?;
        self.window.push(step.as_secs_f64(), self.disk_bytes as u64, &procs);
        self.disk_bytes = 0.0;
        self.step_start = now;
        let mut s = self.window.summary();
        s.disks = watched.iter().map(|(n, _)| n.clone()).collect();
        Some(s)
    }
}

// ------------------------------------------------------ text and the log

fn name_pid(e: &WriteEntry) -> String {
    let mut n: String = e.name.chars().take(32).collect();
    let _ = write!(n, " ({})", e.pid);
    n
}

/// Title and text of the warning balloon.
pub fn alert_text(list: &[&WriteEntry], s: &WriteSummary) -> (String, String) {
    let mins = (s.secs / 60.0).round();
    let mut body = String::new();
    for e in list.iter().take(3) {
        let _ = writeln!(body, "{}: about {} per hour", e.name, format::bytes(per_hour(e.disk_share, s.secs)));
    }
    let _ = write!(
        body,
        "for {mins:.0} min. The SSDs wrote {} per hour. Click for details.",
        format::bytes(per_hour(s.disk, s.secs))
    );
    ("Heavy SSD writes".to_string(), body)
}

/// Adds one line per warning to `write_alerts.csv` in `dir`.
pub fn log_alert(dir: &Path, when: &str, e: &WriteEntry, s: &WriteSummary) -> std::io::Result<()> {
    let path = dir.join(LOG_FILE);
    let new = !path.exists();
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    if new {
        f.write_all(b"time,process,pid,process_bytes,ssd_share_bytes,ssd_bytes,seconds\r\n")?;
    }
    write!(
        f,
        "{when},{},{},{},{},{},{:.0}\r\n",
        e.name.replace(',', " "),
        e.pid,
        e.bytes,
        e.disk_share,
        s.disk,
        s.secs
    )
}

/// One line of the log as report text, or `None` for the header or a bad line.
fn alert_line(line: &str) -> Option<String> {
    let f: Vec<&str> = line.trim().split(',').collect();
    if f.len() < 7 {
        return None;
    }
    let share: u64 = f[4].parse().ok()?;
    let disk: u64 = f[5].parse().ok()?;
    let secs: f64 = f[6].parse().ok()?;
    Some(format!(
        "{}  {}  about {} per hour (all SSD writes {} per hour)",
        f[0],
        f[1],
        format::bytes(per_hour(share, secs)),
        format::bytes(per_hour(disk, secs))
    ))
}

/// The newest `n` warnings from the log, newest first.
pub fn recent_alerts(dir: &Path, n: usize) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join(LOG_FILE)) else { return Vec::new() };
    let mut lines: Vec<String> = text.lines().filter_map(alert_line).collect();
    lines.reverse();
    lines.truncate(n);
    lines
}

/// Text of the "SSD write report" window.
pub fn report(s: Option<&WriteSummary>, on: bool, limit_gb: u32, ignore: &[String], recent: &[String]) -> String {
    let mut t = String::new();
    let _ = writeln!(t, "SSD write watch");
    let _ = writeln!(t, "===============");
    let _ = writeln!(t);
    if !on {
        let _ = writeln!(t, "Status: off. Turn it on in the menu: SSD write warning >");
        let _ = writeln!(t, "Warn about heavy SSD writes.");
    } else {
        let _ = writeln!(t, "Status: on. Warns when one program's estimated share of the SSD writes");
        let _ = writeln!(t, "stays at {limit_gb} GB per hour or more for 15 minutes.");
        let _ = writeln!(t);
        match s {
            None => {
                let _ = writeln!(t, "Collecting the first readings (10 seconds)...");
            }
            Some(s) => {
                let disks = if s.disks.is_empty() { "none found".to_string() } else { s.disks.join(", ") };
                let _ = writeln!(t, "Watched disks: {disks}");
                let mins = s.secs / 60.0;
                if s.full {
                    let _ = writeln!(t, "Last {mins:.0} minutes:");
                } else {
                    let _ = writeln!(t, "Last {mins:.1} minutes (warnings start after 15 minutes):");
                }
                let _ = writeln!(
                    t,
                    "  The SSDs wrote {} = {} per hour",
                    format::bytes(s.disk),
                    format::bytes(per_hour(s.disk, s.secs))
                );
                let _ = writeln!(t);
                let _ = writeln!(t, "Busiest writers. \"Write I/O\" is the program's own counter; it also");
                let _ = writeln!(t, "counts pipes and network traffic. \"To SSD\" is the estimated part of");
                let _ = writeln!(t, "the SSD writes that came from it (only for steady writers: active in");
                let _ = writeln!(t, "at least 2 of every 3 minutes, no minute holding half of it).");
                let _ = writeln!(t);
                let _ = writeln!(
                    t,
                    "  {:<40} {:>10} {:>13} {:>11}",
                    "Program (process id)", "Write I/O", "To SSD /hour", "Active min"
                );
                let total_min = window_minutes(s.steps);
                for e in &s.top {
                    let active = format!("{}/{}", e.minutes, total_min);
                    let share = if e.disk_share == 0 {
                        "-".to_string()
                    } else {
                        format::bytes(per_hour(e.disk_share, s.secs))
                    };
                    let _ = writeln!(
                        t,
                        "  {:<40} {:>10} {:>13} {:>11}",
                        name_pid(e),
                        format::bytes(e.bytes),
                        share,
                        active
                    );
                }
                if s.top.is_empty() {
                    let _ = writeln!(t, "  (no writes yet)");
                }
            }
        }
    }
    let _ = writeln!(t);
    if ignore.is_empty() {
        let _ = writeln!(t, "Ignored programs: none.");
    } else {
        let _ = writeln!(t, "Ignored programs: {}", ignore.join(", "));
    }
    let _ = writeln!(t, "To ignore one, add it to write_ignore= in settings.ini (Open data folder),");
    let _ = writeln!(t, "for example: write_ignore=steam.exe,qbittorrent.exe");
    let _ = writeln!(t);
    let _ = writeln!(t, "Recent warnings, newest first ({LOG_FILE}):");
    if recent.is_empty() {
        let _ = writeln!(t, "  none");
    }
    for r in recent {
        let _ = writeln!(t, "  {r}");
    }
    t
}

/// Lines for `--probe`: the SSDs found and the busiest writers since they started.
pub fn probe_text() -> String {
    let mut t = String::new();
    let _ = writeln!(t, "SSD drives (no seek penalty): {:?}", ssd_drives());
    let mut buf = Vec::new();
    match read_processes(&mut buf) {
        None => {
            let _ = writeln!(t, "Process write counters: not available");
        }
        Some(mut p) => {
            p.sort_by_key(|c| std::cmp::Reverse(c.written));
            let _ = writeln!(t, "Process write counters ({} processes), busiest since start:", p.len());
            for c in p.iter().take(10) {
                let _ = writeln!(t, "  {:<36} {:>7} {:>10}", c.name, c.pid, format::bytes(c.written));
            }
        }
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    fn pc(pid: u32, name: &str, written: u64) -> ProcCounter {
        ProcCounter { pid, name: name.into(), written }
    }

    /// Runs `steps` steps of 10 s; `f(step)` gives (disk bytes, counters).
    fn run(steps: usize, f: impl Fn(u64) -> (u64, Vec<ProcCounter>)) -> WriteSummary {
        let mut w = Window::new(WINDOW_STEPS);
        w.prime(&f(0).1);
        for i in 1..=steps as u64 {
            let (disk, now) = f(i);
            w.push(10.0, disk, &now);
        }
        w.summary()
    }

    #[test]
    fn steady_writer_is_a_suspect() {
        // A writes 10 MB every step; B writes 50 MB once; the SSDs 12 MB a step.
        let s = run(90, |i| {
            let b = if i >= 40 { 50 * MB } else { 0 };
            (12 * MB, vec![pc(100, "a.exe", i * 10 * MB), pc(200, "b.exe", b)])
        });
        assert!(s.full);
        assert_eq!(s.steps, 90);
        assert_eq!(s.disk, 90 * 12 * MB);
        assert_eq!(
            s.top[0],
            WriteEntry {
                pid: 100,
                name: "a.exe".into(),
                bytes: 900 * MB,
                active: 90,
                minutes: 15,
                disk_share: 900 * MB
            }
        );
        assert_eq!((s.top[1].active, s.top[1].disk_share), (1, 0));
        // 900 MB in 15 min = 3.5 GB per hour.
        let found = suspects(&s, 2 * GB);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "a.exe");
        assert!(suspects(&s, 5 * GB).is_empty());
    }

    #[test]
    fn log_rewritten_every_half_minute_is_caught() {
        // Like the reported NVIDIA case: a 19 MB file rewritten in one go every
        // 30 s (every third step) = about 2.2 GB per hour; idle in 2 of 3 steps.
        let s = run(90, |i| {
            let nv = (i / 3) * 19 * MB;
            let disk = if i % 3 == 0 { 20 * MB } else { MB };
            (disk, vec![pc(10, "NVDisplay.Container.exe", nv)])
        });
        let e = &s.top[0];
        assert_eq!((e.active, e.minutes), (30, 15));
        assert_eq!(e.disk_share, 30 * 19 * MB);
        let found = suspects(&s, 2 * GB);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "NVDisplay.Container.exe");
    }

    #[test]
    fn writer_busy_for_only_a_few_minutes_is_not_steady() {
        // 4 GB spread over the first 4 minutes, then nothing.
        let s = run(90, |i| {
            let w = i.min(24) * 170 * MB;
            (if i <= 24 { 170 * MB } else { 0 }, vec![pc(10, "copy.exe", w)])
        });
        assert_eq!(s.top[0].minutes, 4);
        assert!(suspects(&s, GB).is_empty());
    }

    #[test]
    fn one_minute_holding_most_bytes_is_not_steady() {
        // A trickle every minute plus one 2 GB burst.
        let s = run(90, |i| {
            let w = i * MB + if i >= 50 { 2 * GB } else { 0 };
            (MB + if i == 50 { 2 * GB } else { 0 }, vec![pc(10, "mixed.exe", w)])
        });
        assert_eq!(s.top[0].minutes, 15);
        assert_eq!(s.top[0].disk_share, 0);
    }

    #[test]
    fn browser_pipes_are_not_blamed_for_another_writer() {
        // Seen in the live test: a real writer (6 MB a step) and a browser whose
        // counter (8 or 14 MB, mostly pipes) runs above everything the SSDs wrote.
        let s = run(90, |i| {
            let odd = i % 2 == 1;
            let brave: u64 = (1..=i).map(|k| if k % 2 == 1 { 14 * MB } else { 8 * MB }).sum();
            let background = if odd { 5 * MB } else { 3 * MB };
            (6 * MB + background, vec![pc(10, "writer.exe", i * 6 * MB), pc(20, "brave.exe", brave)])
        });
        let found = suspects(&s, GB);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].name.as_str(), found[0].disk_share), ("writer.exe", 540 * MB));
        let brave = s.top.iter().find(|e| e.name == "brave.exe").unwrap();
        assert_eq!(brave.disk_share, 0);
    }

    #[test]
    fn small_pipe_writer_gets_only_the_rest() {
        // A real 14 MB writer and a 3 MB pipe user; the SSDs wrote 15 MB a step.
        let s = run(90, |i| (15 * MB, vec![pc(10, "nv.exe", i * 14 * MB), pc(20, "app.exe", i * 3 * MB)]));
        let found = suspects(&s, GB);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "nv.exe");
        let app = s.top.iter().find(|e| e.name == "app.exe").unwrap();
        assert_eq!(app.disk_share, 90 * MB); // 0.35 GB per hour: below any limit
    }

    #[test]
    fn rewrites_absorbed_by_the_cache_still_count() {
        // The counter (20 MB a step) is above the disk (15 MB): the cache merged
        // some rewrites. The disk part is still blamed on it.
        let s = run(90, |i| (15 * MB, vec![pc(10, "nv.exe", i * 20 * MB)]));
        let found = suspects(&s, 5 * GB);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].disk_share, 90 * 15 * MB);
    }

    #[test]
    fn pipe_traffic_alone_is_not_a_suspect() {
        // The counter is high but the SSDs are quiet: pipes or network.
        let s = run(90, |i| (MB, vec![pc(100, "claude.exe", i * 100 * MB)]));
        assert!(suspects(&s, GB).is_empty());
    }

    #[test]
    fn one_big_copy_is_not_a_suspect() {
        let s = run(90, |i| {
            let w = if i >= 45 { 4 * GB } else { 0 };
            (if i == 45 { 4 * GB } else { 0 }, vec![pc(100, "copy.exe", w)])
        });
        assert_eq!(s.top[0].active, 1);
        assert!(suspects(&s, GB).is_empty());
    }

    #[test]
    fn no_warning_before_the_window_is_full() {
        let s = run(30, |i| (100 * MB, vec![pc(100, "a.exe", i * 100 * MB)]));
        assert!(!s.full);
        assert!(suspects(&s, GB).is_empty());
    }

    #[test]
    fn kernel_processes_are_never_suspects() {
        let s = run(90, |i| (100 * MB, vec![pc(4, "System", i * 100 * MB)]));
        assert_eq!(s.top[0].pid, 4);
        assert!(suspects(&s, GB).is_empty());
    }

    #[test]
    fn new_and_reused_process_ids_start_from_zero() {
        let mut w = Window::new(5);
        w.prime(&[pc(1, "old.exe", 10 * MB)]);
        // pid 1 is now another program; pid 2 is new with a big lifetime total.
        w.push(10.0, 0, &[pc(1, "new.exe", 50 * MB), pc(2, "late.exe", 70 * MB)]);
        assert!(w.summary().top.is_empty());
        w.push(10.0, 0, &[pc(1, "new.exe", 51 * MB), pc(2, "late.exe", 70 * MB)]);
        let s = w.summary();
        assert_eq!(s.top.len(), 1);
        assert_eq!((s.top[0].name.as_str(), s.top[0].bytes), ("new.exe", MB));
    }

    #[test]
    fn old_steps_drop_out() {
        let mut w = Window::new(3);
        w.prime(&[pc(1, "a.exe", 0)]);
        for i in 1..=5u64 {
            w.push(10.0, i, &[pc(1, "a.exe", i * MB)]);
        }
        let s = w.summary();
        assert_eq!((s.steps, s.disk, s.top[0].bytes), (3, 3 + 4 + 5, 3 * MB));
        assert!(s.full);
    }

    #[test]
    fn disks_to_watch() {
        let rates = vec![
            ("0 C:".to_string(), 1.0),
            ("1 D: E:".to_string(), 2.0),
            ("_Total".to_string(), 3.0),
        ];
        let names = |ssd: &[u32]| watched(&rates, ssd).iter().map(|(n, _)| n.clone()).collect::<Vec<_>>();
        assert_eq!(names(&[1]), vec!["1 D: E:"]);
        assert_eq!(names(&[]), vec!["0 C:", "1 D: E:"]);
        assert_eq!(disk_number("12 F:"), Some(12));
        assert_eq!(disk_number("_Total"), None);
    }

    #[test]
    fn rate_per_hour() {
        assert_eq!(per_hour(GB, 900.0), 4 * GB);
        assert_eq!(per_hour(GB, 0.0), 0);
    }

    #[test]
    fn log_lines_and_report() {
        assert_eq!(alert_line("time,process,pid,process_bytes,ssd_share_bytes,ssd_bytes,seconds"), None);
        let line = alert_line("2026-11-19 21:30,nv.exe,7,3221225472,1073741824,2147483648,900").unwrap();
        assert_eq!(line, "2026-11-19 21:30  nv.exe  about 4.00 GB per hour (all SSD writes 8.00 GB per hour)");

        let s = run(90, |i| (12 * MB, vec![pc(100, "a.exe", i * 10 * MB)]));
        let text = report(Some(&s), true, 2, &["steam.exe".into()], std::slice::from_ref(&line));
        assert!(text.contains("a.exe (100)"));
        assert!(text.contains("Last 15 minutes"));
        assert!(text.contains("Ignored programs: steam.exe"));
        assert!(text.contains(&line));
        assert!(report(None, false, 2, &[], &[]).contains("Status: off"));

        let found = suspects(&s, 2 * GB);
        let (title, body) = alert_text(&found, &s);
        assert_eq!(title, "Heavy SSD writes");
        assert!(body.starts_with("a.exe: about 3.52 GB per hour\nfor 15 min."));
        assert!(text.contains("To SSD /hour"));
        assert!(body.encode_utf16().count() < 256);
    }

    #[test]
    fn reads_this_pc() {
        let mut buf = Vec::new();
        let p = read_processes(&mut buf).expect("process list");
        assert!(p.len() > 10);
        let me = p.iter().find(|c| c.pid == std::process::id()).expect("own process");
        assert!(me.name.to_lowercase().ends_with(".exe"));
        assert!(p.iter().any(|c| c.pid == 4 && c.name == "System"));
    }
}
