//! Program state, the hidden main window and the message loop.
//!
//! All state lives in one `App` on the UI thread. Window procedures reach it
//! through `with_app`, which refuses (returns `None`) instead of panicking when
//! a message arrives while the state is already borrowed.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::mem::size_of;
use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::HBRUSH;
use windows_sys::Win32::UI::Controls::{ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, IDC_ARROW,
    LoadCursorW, LoadIconW, MSG, PBT_APMSUSPEND, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetTimer, TranslateMessage, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
    WM_DISPLAYCHANGE, WM_ENDSESSION, WM_POWERBROADCAST, WM_QUERYENDSESSION, WM_SETTINGCHANGE,
    WM_TIMER, WNDCLASSEXW, WNDPROC, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::floating::FloatWin;
use crate::format;
use crate::history::History;
use crate::render::Cell;
use crate::sampler::{SamplerCfg, SamplerHandle, Snapshot, WM_APP_SNAPSHOT};
use crate::settings::{Item, Settings};
use crate::sys::{self, Date, hinstance, wide};
use crate::taskbar::TaskbarWin;
use crate::tray::{
    NIN_BALLOONHIDE, NIN_BALLOONTIMEOUT, NIN_BALLOONUSERCLICK, NIN_KEYSELECT, Tooltip, Tray,
    WM_APP_TRAY,
};
use crate::writewatch::{self, WriteEntry, WriteSummary};

const MAIN_CLASS: &str = "GatiMeter.Main";
const TIMER_PLACE: usize = 1;
const NIN_SELECT: u32 = 0x400;
const HISTORY_SAVE_EVERY: Duration = Duration::from_secs(300);
/// The same program is not warned about again for this long.
const WRITE_ALERT_AGAIN: Duration = Duration::from_secs(6 * 3600);

static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app state, or returns `None` if it is busy or gone.
pub fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut g) => g.as_mut().map(f),
        Err(_) => None,
    })
}

pub fn register_class(name: &str, proc: WNDPROC, style: u32) {
    register_class_bg(name, proc, style, null_mut());
}

/// Like `register_class`, with a background brush for windows that hold controls.
pub fn register_class_bg(name: &str, proc: WNDPROC, style: u32, background: HBRUSH) {
    let n = wide(name);
    unsafe {
        let mut wc: WNDCLASSEXW = std::mem::zeroed();
        wc.cbSize = size_of::<WNDCLASSEXW>() as u32;
        wc.style = style;
        wc.lpfnWndProc = proc;
        wc.hInstance = hinstance();
        wc.hCursor = LoadCursorW(null_mut(), IDC_ARROW);
        wc.hIcon = LoadIconW(hinstance(), 1 as _);
        wc.hbrBackground = background;
        wc.lpszClassName = n.as_ptr();
        RegisterClassExW(&wc);
    }
}

pub struct App {
    pub main: HWND,
    dir: PathBuf,
    pub settings: Settings,
    pub snap: Snapshot,
    history: History,
    today: Date,
    taskbar: Option<TaskbarWin>,
    floating: Option<FloatWin>,
    tray: Tray,
    tip: Tooltip,
    sampler: SamplerHandle,
    light_taskbar: bool,
    last_save: Instant,
    /// Adapter names in the order the last menu listed them.
    pub menu_adapters: Vec<String>,
    pub menu_disks: Vec<String>,
    /// Program names in the order the last "Ignored programs" menu listed them.
    pub menu_ignore: Vec<String>,
    /// Latest SSD write watch summary (while the watch is on).
    write: Option<WriteSummary>,
    /// Lower-case program name -> when it was last warned about.
    write_alerted: HashMap<String, Instant>,
    /// The tray icon is shown only to carry a warning.
    tray_for_alert: bool,
}

fn sampler_cfg(s: &Settings) -> SamplerCfg {
    SamplerCfg {
        interval_ms: s.interval_ms,
        adapter: s.adapter.clone(),
        gpu: s.has(Item::Gpu),
        disk: s.has(Item::Disk),
        cpu_temp: s.has(Item::CpuTemp),
        gpu_temp: s.has(Item::GpuTemp),
        disk_temp: s.has(Item::DiskTemp),
        disk_temp_drive: s.disk_temp_drive.clone(),
        write_watch: s.write_watch,
    }
}

pub fn run() {
    unsafe {
        let icc = INITCOMMONCONTROLSEX {
            dwSize: size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        };
        InitCommonControlsEx(&icc);
    }
    register_class(MAIN_CLASS, Some(main_proc), 0);
    crate::taskbar::register();
    crate::floating::register();
    crate::histwin::register();
    crate::arrange::register();
    TASKBAR_CREATED.store(
        unsafe { RegisterWindowMessageW(wide("TaskbarCreated").as_ptr()) },
        Ordering::Relaxed,
    );

    // A hidden top-level window: owns the menu and tray icon and receives the
    // "TaskbarCreated" broadcast after Explorer restarts.
    let main = unsafe {
        CreateWindowExW(
            WS_EX_TOOLWINDOW,
            wide(MAIN_CLASS).as_ptr(),
            wide("GatiMeter").as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            null_mut(),
            null_mut(),
            hinstance(),
            null(),
        )
    };
    if main.is_null() {
        return;
    }

    let dir = sys::data_dir();
    let settings = Settings::load(&dir.join("settings.ini"));
    let history = History::load(dir.join("history.csv"));
    let sampler = SamplerHandle::start(sampler_cfg(&settings), main);
    let app = App {
        main,
        tray: Tray::new(main),
        tip: Tooltip::new(main),
        dir,
        settings,
        snap: Snapshot::default(),
        history,
        today: Date::today(),
        taskbar: None,
        floating: None,
        sampler,
        light_taskbar: sys::system_uses_light_theme(),
        last_save: Instant::now(),
        menu_adapters: Vec::new(),
        menu_disks: Vec::new(),
        menu_ignore: Vec::new(),
        write: None,
        write_alerted: HashMap::new(),
        tray_for_alert: false,
    };
    APP.with(|c| *c.borrow_mut() = Some(app));
    with_app(|a| a.apply());
    unsafe { SetTimer(main, TIMER_PLACE, 500, None) };

    let mut msg: MSG = unsafe { std::mem::zeroed() };
    while unsafe { GetMessageW(&mut msg, null_mut(), 0, 0) } > 0 {
        if crate::arrange::pre_translate(&msg) {
            continue;
        }
        unsafe {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

impl App {
    /// Rows per column of the main readout: the taskbar's, or one long list
    /// when only the floating window is shown.
    pub fn layout_rows(&self) -> usize {
        match &self.taskbar {
            Some(t) if self.settings.show_taskbar && t.rows > 0 => t.rows,
            _ if self.settings.show_floating => usize::MAX,
            _ => 2,
        }
    }

    /// True when no readout window can be right-clicked, so the tray icon must stay.
    pub fn tray_forced(&self) -> bool {
        let s = &self.settings;
        let taskbar_ok = s.show_taskbar && self.taskbar.is_some();
        let float_ok = s.show_floating && !s.float_clickthrough;
        !(taskbar_ok || float_ok)
    }

    /// Saves the settings and brings every window in line with them.
    pub fn changed(&mut self) {
        let _ = self.settings.save(&self.dir.join("settings.ini"));
        self.apply();
    }

    pub fn apply(&mut self) {
        self.sampler.set_cfg(sampler_cfg(&self.settings));
        if !self.settings.write_watch {
            self.write = None;
        }
        self.sync_taskbar();
        if self.settings.show_floating {
            if self.floating.is_none() {
                self.floating = FloatWin::create(&self.settings);
                if let Some(f) = &self.floating {
                    self.tip.add(f.hwnd);
                }
            }
            if let Some(f) = &self.floating {
                f.apply_style(&self.settings);
            }
        } else if let Some(f) = self.floating.take() {
            self.tip.remove(f.hwnd);
        }
        self.sync_tray();
        self.render();
    }

    fn sync_tray(&mut self) {
        let need = self.settings.show_tray_icon || self.tray_forced() || self.tray_for_alert;
        self.tray.set_visible(need);
    }

    /// The warning was clicked, closed or timed out.
    pub fn balloon_done(&mut self) {
        if self.tray_for_alert {
            self.tray_for_alert = false;
            self.sync_tray();
        }
    }

    /// Warns about programs that keep writing heavily to the SSDs.
    fn check_writes(&mut self) {
        let Some(s) = &self.write else { return };
        let limit = self.settings.write_limit_gb as u64 * writewatch::GB;
        let fresh: Vec<WriteEntry> = writewatch::suspects(s, limit)
            .into_iter()
            .filter(|e| {
                let key = e.name.to_lowercase();
                !self.settings.write_ignore.contains(&key)
                    && self.write_alerted.get(&key).is_none_or(|t| t.elapsed() >= WRITE_ALERT_AGAIN)
            })
            .cloned()
            .collect();
        if fresh.is_empty() {
            return;
        }
        let when = sys::now_text();
        for e in &fresh {
            self.write_alerted.insert(e.name.to_lowercase(), Instant::now());
            let _ = writewatch::log_alert(&self.dir, &when, e, s);
        }
        let refs: Vec<&WriteEntry> = fresh.iter().collect();
        let (title, text) = writewatch::alert_text(&refs, s);
        if !self.tray.is_shown() {
            self.tray_for_alert = true;
            self.sync_tray();
        }
        self.tray.balloon(&title, &text);
    }

    /// Programs for the "Ignored programs" menu (lower case).
    pub fn ignore_choices(&self) -> Vec<String> {
        writewatch::ignore_choices(
            self.write.as_ref(),
            &self.settings.write_ignore,
            &writewatch::recent_alert_names(&self.dir, 5),
            20,
        )
    }

    pub fn write_report(&self) -> String {
        let s = &self.settings;
        writewatch::report(
            self.write.as_ref(),
            s.write_watch,
            s.write_limit_gb,
            &s.write_ignore,
            &writewatch::recent_alerts(&self.dir, 10),
        )
    }

    fn sync_taskbar(&mut self) {
        if (self.taskbar.as_ref().is_some_and(|t| !t.alive()) || !self.settings.show_taskbar)
            && let Some(t) = self.taskbar.take() {
                self.tip.remove(t.hwnd);
            }
        if self.settings.show_taskbar && self.taskbar.is_none() {
            self.taskbar = TaskbarWin::create();
            if let Some(t) = &self.taskbar {
                self.tip.add(t.hwnd);
            }
        }
    }

    pub fn render(&mut self) {
        let cells = self.cells();
        if let Some(t) = &mut self.taskbar {
            t.update(&cells, &self.settings, self.light_taskbar);
        }
        if let Some(f) = &mut self.floating {
            f.update(&cells, &self.settings);
        }
        let tip = self.tooltip_text();
        self.tip.set_text(&tip);
        let tray_tip = self.tray_tip();
        self.tray.set_tip(&tray_tip);
    }

    fn on_snapshots(&mut self) {
        let snaps = self.sampler.drain();
        let Some(last) = snaps.last().cloned() else { return };
        let today = Date::today();
        if today != self.today {
            let _ = self.history.save();
            self.today = today;
        }
        for s in &snaps {
            self.history.add(today, s.delta_up, s.delta_down);
        }
        if self.settings.write_watch
            && let Some(w) = snaps.iter().rev().find_map(|s| s.write.clone())
        {
            self.write = Some(w);
            self.check_writes();
        }
        self.snap = last;
        if self.last_save.elapsed() >= HISTORY_SAVE_EVERY {
            let _ = self.history.save();
            self.last_save = Instant::now();
        }
        // Explorer restarted or the taskbar was not ready at start-up.
        if self.settings.show_taskbar && self.taskbar.as_ref().is_none_or(|t| !t.alive()) {
            self.sync_taskbar();
            self.sync_tray();
        }
        self.render();
    }

    fn on_timer(&mut self) {
        if let Some(t) = &mut self.taskbar {
            t.place(&self.settings);
        }
    }

    pub fn floating_moved(&mut self) {
        if let Some(f) = &self.floating {
            let (x, y) = f.position();
            self.settings.float_x = Some(x);
            self.settings.float_y = Some(y);
            let _ = self.settings.save(&self.dir.join("settings.ini"));
        }
    }

    fn refresh_theme(&mut self) {
        let light = sys::system_uses_light_theme();
        if light != self.light_taskbar {
            self.light_taskbar = light;
            self.render();
        }
    }

    fn explorer_restarted(&mut self) {
        if let Some(t) = self.taskbar.take() {
            self.tip.remove(t.hwnd);
        }
        self.tray.readd();
        self.apply();
    }

    fn save_all(&mut self) {
        let _ = self.history.save();
        let _ = self.settings.save(&self.dir.join("settings.ini"));
    }

    fn shutdown(&mut self) {
        self.sampler.stop();
        for s in self.sampler.drain() {
            self.history.add(self.today, s.delta_up, s.delta_down);
        }
        self.save_all();
        if let Some(t) = self.taskbar.take() {
            self.tip.remove(t.hwnd);
        }
        if let Some(f) = self.floating.take() {
            self.tip.remove(f.hwnd);
        }
        self.tray.set_visible(false);
    }

    fn network_desc(&self) -> String {
        if !self.settings.adapter.is_empty() {
            return self.settings.adapter.clone();
        }
        if self.snap.measured.is_empty() {
            "Automatic (no connected adapter)".into()
        } else {
            format!("Automatic ({})", self.snap.measured.join(", "))
        }
    }

    pub fn history_report(&self) -> String {
        self.history.report(Date::today(), &self.network_desc())
    }

    fn cells(&self) -> Vec<Cell> {
        let s = &self.settings;
        let n = &self.snap;
        s.items
            .iter()
            .map(|&it| {
                let (value, template) = match it {
                    Item::Up => (format::speed(n.up_bps, s.bits), format::speed_template(s.bits)),
                    Item::Down => (format::speed(n.down_bps, s.bits), format::speed_template(s.bits)),
                    Item::Cpu => (format::percent(n.cpu), "100%"),
                    Item::Mem => (format::percent(n.mem), "100%"),
                    Item::Gpu => (format::percent(n.gpu), "100%"),
                    Item::Disk => (format::percent(n.disk), "100%"),
                    Item::CpuTemp => (format::temp(n.cpu_temp), "100°C"),
                    Item::GpuTemp => (format::temp(n.gpu_temp), "100°C"),
                    Item::DiskTemp => (format::temp(n.disk_temp), "100°C"),
                };
                let label = if s.labels || it.always_labelled() { it.label() } else { "" };
                Cell { label, value, template }
            })
            .collect()
    }

    fn tooltip_text(&self) -> String {
        let n = &self.snap;
        let bits = self.settings.bits;
        let mut t = String::new();
        let _ = writeln!(t, "Network: {}", self.network_desc());
        let _ = writeln!(
            t,
            "Upload {}    Download {}",
            format::speed(n.up_bps, bits),
            format::speed(n.down_bps, bits)
        );
        let (du, dd) = self.history.day(self.today);
        let _ = writeln!(t, "Today: ↑ {}  ↓ {}", format::bytes(du), format::bytes(dd));
        let (mu, md) = self.history.month(self.today.y, self.today.m);
        let _ = writeln!(t, "This month: ↑ {}  ↓ {}", format::bytes(mu), format::bytes(md));
        let temp = |c: Option<f32>, note: &str| {
            c.map(|c| format!("    {}{note}", format::temp(Some(c)))).unwrap_or_default()
        };
        let _ = writeln!(t, "CPU: {}{}", format::percent(n.cpu), temp(n.cpu_temp, " (ACPI, approximate)"));
        let _ = writeln!(
            t,
            "Memory: {} of {} ({})",
            format::bytes(n.mem_used),
            format::bytes(n.mem_total),
            format::percent(n.mem)
        );
        if n.gpu.is_some() || n.gpu_temp.is_some() {
            let _ = writeln!(t, "GPU: {}{}", format::percent(n.gpu), temp(n.gpu_temp, ""));
        }
        if n.disk.is_some() || n.disk_temp.is_some() {
            let _ = writeln!(t, "Disk: {}{}", format::percent(n.disk), temp(n.disk_temp, ""));
        }
        if n.disk_temps.len() > 1 {
            for (name, c) in &n.disk_temps {
                let mark = if *name == self.settings.disk_temp_drive { "  (shown)" } else { "" };
                let _ = writeln!(t, "    {name}: {}{mark}", format::temp(Some(*c)));
            }
        }
        t.trim_end().replace('\n', "\r\n")
    }

    fn tray_tip(&self) -> String {
        let n = &self.snap;
        let bits = self.settings.bits;
        format!(
            "GatiMeter\n↑ {}   ↓ {}\nCPU {}   RAM {}",
            format::speed(n.up_bps, bits),
            format::speed(n.down_bps, bits),
            format::percent(n.cpu),
            format::percent(n.mem)
        )
    }
}

fn shutdown() {
    let app = APP.with(|c| c.try_borrow_mut().ok().and_then(|mut g| g.take()));
    if let Some(mut a) = app {
        a.shutdown();
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let taskbar_created = TASKBAR_CREATED.load(Ordering::Relaxed);
    match msg {
        WM_APP_SNAPSHOT => {
            with_app(|a| a.on_snapshots());
            0
        }
        WM_APP_TRAY => {
            match (lp as u32) & 0xFFFF {
                WM_CONTEXTMENU | NIN_SELECT | NIN_KEYSELECT => crate::menu::show_at_cursor(),
                NIN_BALLOONUSERCLICK => {
                    with_app(|a| a.balloon_done());
                    crate::menu::open_write_report();
                }
                NIN_BALLOONTIMEOUT | NIN_BALLOONHIDE => {
                    with_app(|a| a.balloon_done());
                }
                _ => {}
            }
            0
        }
        WM_TIMER if wp == TIMER_PLACE => {
            with_app(|a| a.on_timer());
            0
        }
        WM_SETTINGCHANGE => {
            with_app(|a| a.refresh_theme());
            0
        }
        WM_DISPLAYCHANGE => {
            with_app(|a| a.render());
            0
        }
        WM_QUERYENDSESSION => 1,
        WM_ENDSESSION => {
            if wp != 0 {
                with_app(|a| a.save_all());
            }
            0
        }
        WM_POWERBROADCAST => {
            if wp as u32 == PBT_APMSUSPEND {
                with_app(|a| a.save_all());
            }
            1
        }
        WM_CLOSE => {
            unsafe { DestroyWindow(hwnd) };
            0
        }
        WM_DESTROY => {
            shutdown();
            unsafe { PostQuitMessage(0) };
            0
        }
        m if taskbar_created != 0 && m == taskbar_created => {
            with_app(|a| a.explorer_restarted());
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
