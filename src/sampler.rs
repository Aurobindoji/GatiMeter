//! Background thread that reads all values once per interval and hands a
//! `Snapshot` to the UI thread.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{FILETIME, HWND};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::GetSystemTimes;
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use crate::net::NetSampler;
use crate::pdh::{Counter, Query};
use crate::temps::{DiskTemp, GpuTemp, pick_disk_temp};
use crate::writewatch::{WriteSummary, WriteWatch};

pub const WM_APP_SNAPSHOT: u32 = WM_APP + 1;

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub up_bps: f64,
    pub down_bps: f64,
    pub delta_up: u64,
    pub delta_down: u64,
    pub cpu: Option<f32>,
    pub mem: Option<f32>,
    pub mem_used: u64,
    pub mem_total: u64,
    pub gpu: Option<f32>,
    pub disk: Option<f32>,
    pub cpu_temp: Option<f32>,
    pub gpu_temp: Option<f32>,
    pub disk_temp: Option<f32>,
    /// Every disk that reports a temperature: (name, °C).
    pub disk_temps: Vec<(String, f32)>,
    pub adapters: Vec<String>,
    pub measured: Vec<String>,
    /// Set at the end of each 10-second step of the SSD write watch.
    pub write: Option<WriteSummary>,
}

/// What to read. Costly readings are only taken when they are shown.
#[derive(Clone, Debug, PartialEq)]
pub struct SamplerCfg {
    pub interval_ms: u32,
    pub adapter: String,
    pub gpu: bool,
    pub disk: bool,
    pub cpu_temp: bool,
    pub gpu_temp: bool,
    pub disk_temp: bool,
    /// Disk whose temperature is shown; empty for the hottest.
    pub disk_temp_drive: String,
    pub write_watch: bool,
}

enum Ctl {
    Wake,
    Stop,
}

pub struct SamplerHandle {
    cfg: Arc<Mutex<SamplerCfg>>,
    ctl: Sender<Ctl>,
    rx: Receiver<Snapshot>,
    thread: Option<JoinHandle<()>>,
}

impl SamplerHandle {
    pub fn start(cfg: SamplerCfg, notify: HWND) -> SamplerHandle {
        let shared = Arc::new(Mutex::new(cfg));
        let (ctl_tx, ctl_rx) = channel();
        let (tx, rx) = channel();
        let hwnd = notify as usize;
        let cfg2 = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("sampler".into())
            .spawn(move || run(cfg2, ctl_rx, tx, hwnd))
            .ok();
        SamplerHandle { cfg: shared, ctl: ctl_tx, rx, thread }
    }

    pub fn set_cfg(&self, cfg: SamplerCfg) {
        let mut g = self.cfg.lock().unwrap_or_else(|e| e.into_inner());
        if *g != cfg {
            *g = cfg;
            drop(g);
            let _ = self.ctl.send(Ctl::Wake);
        }
    }

    pub fn drain(&self) -> Vec<Snapshot> {
        self.rx.try_iter().collect()
    }

    pub fn stop(&mut self) {
        let _ = self.ctl.send(Ctl::Stop);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(cfg: Arc<Mutex<SamplerCfg>>, ctl: Receiver<Ctl>, tx: Sender<Snapshot>, hwnd: usize) {
    let mut s = Sampler::new();
    loop {
        let c = cfg.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let snap = s.sample(&c);
        if tx.send(snap).is_err() {
            break;
        }
        unsafe { PostMessageW(hwnd as HWND, WM_APP_SNAPSHOT, 0, 0) };
        let wait = Duration::from_millis(c.interval_ms.clamp(250, 10_000) as u64);
        match ctl.recv_timeout(wait) {
            Ok(Ctl::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Ctl::Wake) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

struct Sampler {
    net: NetSampler,
    query: Option<Query>,
    cpu: Option<Counter>,
    gpu: Option<Counter>,
    disk: Option<Counter>,
    /// Thermal zone counter; `true` when it is the tenths-of-kelvin variant.
    thermal: Option<(Counter, bool)>,
    thermal_tried: bool,
    sys_times: Option<(u64, u64)>,
    gpu_temp: GpuTemp,
    disk_temp: DiskTemp,
    disk_temp_last: Vec<(String, f32)>,
    disk_temp_at: Option<Instant>,
    disk_write: Option<Counter>,
    watch: Option<WriteWatch>,
}

fn ft(f: &FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

impl Sampler {
    fn new() -> Sampler {
        let query = Query::new();
        let cpu = query.as_ref().and_then(|q| q.add("\\Processor Information(_Total)\\% Processor Utility"));
        Sampler {
            net: NetSampler::default(),
            query,
            cpu,
            gpu: None,
            disk: None,
            thermal: None,
            thermal_tried: false,
            sys_times: None,
            gpu_temp: GpuTemp::new(),
            disk_temp: DiskTemp::new(),
            disk_temp_last: Vec::new(),
            disk_temp_at: None,
            disk_write: None,
            watch: None,
        }
    }

    /// Adds counters that became wanted and drops the ones no longer shown.
    fn sync_counters(&mut self, cfg: &SamplerCfg) {
        let Some(q) = &self.query else { return };
        toggle(q, &mut self.gpu, cfg.gpu, "\\GPU Engine(*)\\Utilization Percentage");
        toggle(q, &mut self.disk, cfg.disk, "\\PhysicalDisk(*)\\% Idle Time");
        toggle(q, &mut self.disk_write, cfg.write_watch, "\\PhysicalDisk(*)\\Disk Write Bytes/sec");
        if cfg.cpu_temp && self.thermal.is_none() && !self.thermal_tried {
            self.thermal_tried = true;
            self.thermal = q
                .add("\\Thermal Zone Information(*)\\High Precision Temperature")
                .map(|c| (c, true))
                .or_else(|| q.add("\\Thermal Zone Information(*)\\Temperature").map(|c| (c, false)));
        } else if !cfg.cpu_temp {
            if let Some((c, _)) = self.thermal.take() {
                q.remove(c);
            }
            self.thermal_tried = false;
        }
    }

    fn cpu_from_system_times(&mut self) -> Option<f32> {
        let (mut idle, mut kernel, mut user) =
            (FILETIME::default(), FILETIME::default(), FILETIME::default());
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return None;
        }
        let idle = ft(&idle);
        let total = ft(&kernel) + ft(&user); // kernel time includes idle time
        let prev = self.sys_times.replace((idle, total));
        let (pi, pt) = prev?;
        let dt = total.checked_sub(pt)?;
        let di = idle.checked_sub(pi)?;
        if dt == 0 {
            return None;
        }
        Some((100.0 * (dt.saturating_sub(di)) as f64 / dt as f64) as f32)
    }

    fn sample(&mut self, cfg: &SamplerCfg) -> Snapshot {
        self.sync_counters(cfg);
        if let Some(q) = &self.query {
            q.collect();
        }
        let net = self.net.sample(&cfg.adapter);
        let mut s = Snapshot {
            up_bps: net.up_bps,
            down_bps: net.down_bps,
            delta_up: net.delta_up,
            delta_down: net.delta_down,
            adapters: net.adapters,
            measured: net.measured,
            ..Snapshot::default()
        };

        // CPU: Task Manager's "% Processor Utility", else the classic idle-time ratio.
        let classic = self.cpu_from_system_times();
        s.cpu = self
            .cpu
            .as_ref()
            .and_then(|c| c.value())
            .map(|v| v.clamp(0.0, 100.0) as f32)
            .or(classic);

        let mut m: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        m.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 && m.ullTotalPhys > 0 {
            s.mem_total = m.ullTotalPhys;
            s.mem_used = m.ullTotalPhys.saturating_sub(m.ullAvailPhys);
            s.mem = Some((s.mem_used as f64 * 100.0 / s.mem_total as f64) as f32);
        }

        if let Some(c) = &self.gpu {
            s.gpu = Some(busiest_gpu_engine(&c.array()));
        }
        if let Some(c) = &self.disk {
            s.disk = c
                .array()
                .iter()
                .filter(|(name, _)| name != "_Total")
                .map(|(_, idle)| (100.0 - idle).clamp(0.0, 100.0) as f32)
                .fold(None, |m: Option<f32>, v| Some(m.map_or(v, |m| m.max(v))));
        }
        if let Some((c, precise)) = &self.thermal {
            s.cpu_temp = c
                .array()
                .iter()
                .map(|(_, v)| if *precise { v / 10.0 } else { *v } - 273.15)
                .filter(|c| (1.0..=125.0).contains(c))
                .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
                .map(|c| c as f32);
        }
        if cfg.gpu_temp {
            s.gpu_temp = self.gpu_temp.read();
        }
        if cfg.disk_temp {
            // Disk temperature changes slowly; ask the drives every 5 s.
            if self.disk_temp_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(5)) {
                self.disk_temp_last = self.disk_temp.read();
                self.disk_temp_at = Some(Instant::now());
            }
            s.disk_temp = pick_disk_temp(&self.disk_temp_last, &cfg.disk_temp_drive);
            s.disk_temps = self.disk_temp_last.clone();
        } else {
            self.disk_temp_at = None;
        }
        if cfg.write_watch {
            let rates = self.disk_write.as_ref().map(|c| c.array()).unwrap_or_default();
            s.write = self.watch.get_or_insert_with(WriteWatch::new).sample(&rates);
        } else {
            self.watch = None;
        }
        s
    }
}

/// Reads everything `rounds` times on the calling thread (for `--probe`).
pub fn probe(rounds: u32, gap: Duration) -> Vec<Snapshot> {
    let cfg = SamplerCfg {
        interval_ms: 1000,
        adapter: String::new(),
        gpu: true,
        disk: true,
        cpu_temp: true,
        gpu_temp: true,
        disk_temp: true,
        disk_temp_drive: String::new(),
        write_watch: false, // needs 10 s steps; `--probe` prints its own lines
    };
    let mut s = Sampler::new();
    let mut out = Vec::new();
    for i in 0..rounds {
        if i > 0 {
            std::thread::sleep(gap);
        }
        out.push(s.sample(&cfg));
    }
    out
}

fn toggle(q: &Query, slot: &mut Option<Counter>, want: bool, path: &str) {
    if want && slot.is_none() {
        *slot = q.add(path);
    } else if !want
        && let Some(c) = slot.take() {
            q.remove(c);
        }
}

/// GPU load as Task Manager shows it: add up every process per engine, then
/// take the busiest engine. Instance names look like
/// `pid_1234_luid_0x0_0xD1A3_phys_0_eng_0_engtype_3D`.
pub fn busiest_gpu_engine(items: &[(String, f64)]) -> f32 {
    let mut per_engine: HashMap<&str, f64> = HashMap::new();
    for (name, v) in items {
        let key = name.find("luid_").map_or(name.as_str(), |i| &name[i..]);
        *per_engine.entry(key).or_default() += v;
    }
    per_engine.values().fold(0.0f64, |m, &v| m.max(v)).clamp(0.0, 100.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_engine_sum() {
        let items = vec![
            ("pid_1_luid_0x0_0x1_phys_0_eng_0_engtype_3D".to_string(), 10.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_0_engtype_3D".to_string(), 15.0),
            ("pid_2_luid_0x0_0x1_phys_0_eng_3_engtype_VideoDecode".to_string(), 20.0),
        ];
        assert_eq!(busiest_gpu_engine(&items), 25.0);
        assert_eq!(busiest_gpu_engine(&[]), 0.0);
    }
}
