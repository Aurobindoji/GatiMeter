//! The right-click menu (taskbar readout, floating window and tray icon) and
//! the commands behind it.

use std::mem::size_of;
use std::ptr::null;

use windows_sys::Win32::Foundation::{HWND, POINT};
use windows_sys::Win32::Graphics::Gdi::{GetDC, GetDeviceCaps, LOGFONTW, LOGPIXELSY, ReleaseDC};
use windows_sys::Win32::UI::Controls::Dialogs::{
    CC_FULLOPEN, CC_RGBINIT, CF_INITTOLOGFONTSTRUCT, CF_LIMITSIZE, CF_NOVERTFONTS,
    CF_SCREENFONTS, CHOOSECOLORW, CHOOSEFONTW, ChooseColorW, ChooseFontW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, GetCursorPos, HMENU, MF_CHECKED, MF_GRAYED,
    MF_POPUP, MF_SEPARATOR, MF_STRING, PostMessageW, SetForegroundWindow, TPM_NONOTIFY,
    TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx, WM_CLOSE, WM_NULL,
};

use crate::app::{App, with_app};
use crate::settings::{ITEMS, Item};
use crate::sys::{self, wide};

const SHOW_TASKBAR: u32 = 100;
const SHOW_FLOAT: u32 = 101;
const SHOW_TRAY: u32 = 102;
const ITEM_BASE: u32 = 200;
const ADAPTER_AUTO: u32 = 300;
const ADAPTER_BASE: u32 = 301;
const UNIT_BYTES: u32 = 400;
const UNIT_BITS: u32 = 401;
const LABELS: u32 = 402;
const TB_RIGHT: u32 = 403;
const TB_LEFT: u32 = 404;
const FL_TOPMOST: u32 = 500;
const FL_CLICK: u32 = 501;
const FL_LOCK: u32 = 502;
const FL_TRANSPARENT: u32 = 503;
const FL_BG: u32 = 504;
const FL_OPACITY_BASE: u32 = 510; // + percent / 10
const FONT: u32 = 600;
const COLOR_AUTO: u32 = 601;
const COLOR_PICK: u32 = 602;
const INTERVAL_BASE: u32 = 700;
const HISTORY: u32 = 800;
const TASKMGR: u32 = 801;
const AUTOSTART: u32 = 802;
const DATA_DIR: u32 = 803;
const ABOUT: u32 = 804;
const EXIT: u32 = 805;

const INTERVALS: [(u32, &str); 4] = [(500, "0.5 s"), (1000, "1 s"), (2000, "2 s"), (5000, "5 s")];

fn add(m: HMENU, id: u32, text: &str, checked: bool, enabled: bool) {
    let mut flags = MF_STRING;
    if checked {
        flags |= MF_CHECKED;
    }
    if !enabled {
        flags |= MF_GRAYED;
    }
    let t = wide(text);
    unsafe { AppendMenuW(m, flags, id as usize, t.as_ptr()) };
}

fn sep(m: HMENU) {
    unsafe { AppendMenuW(m, MF_SEPARATOR, 0, null()) };
}

fn sub(m: HMENU, text: &str) -> HMENU {
    let s = unsafe { CreatePopupMenu() };
    let t = wide(text);
    unsafe { AppendMenuW(m, MF_POPUP, s as usize, t.as_ptr()) };
    s
}

fn build(a: &mut App) -> HMENU {
    let s = &a.settings;
    let m = unsafe { CreatePopupMenu() };
    add(m, SHOW_TASKBAR, "Show in taskbar", s.show_taskbar, true);
    add(m, SHOW_FLOAT, "Show floating window", s.show_floating, true);
    let forced = a.tray_forced();
    add(m, SHOW_TRAY, "Show notification-area icon", s.show_tray_icon || forced, !forced);
    sep(m);

    let items = sub(m, "Items to show");
    for (i, it) in ITEMS.iter().enumerate() {
        add(items, ITEM_BASE + i as u32, it.menu_text(), s.has(*it), true);
    }

    let net = sub(m, "Network adapter");
    add(net, ADAPTER_AUTO, "Automatic (all connected adapters)", s.adapter.is_empty(), true);
    sep(net);
    let mut names = a.snap.adapters.clone();
    if !s.adapter.is_empty() && !names.contains(&s.adapter) {
        names.push(s.adapter.clone()); // chosen earlier, not connected now
    }
    for (i, n) in names.iter().enumerate().take(50) {
        add(net, ADAPTER_BASE + i as u32, n, *n == s.adapter, true);
    }
    a.menu_adapters = names;

    let units = sub(m, "Speed units");
    add(units, UNIT_BYTES, "Bytes (KB/s, MB/s)", !s.bits, true);
    add(units, UNIT_BITS, "Bits (Kb/s, Mb/s)", s.bits, true);
    add(m, LABELS, "Show labels", s.labels, true);

    let pos = sub(m, "Taskbar position");
    add(pos, TB_RIGHT, "Next to the tray icons", !s.taskbar_left, true);
    add(pos, TB_LEFT, "Left end of the taskbar", s.taskbar_left, true);

    let fl = sub(m, "Floating window");
    add(fl, FL_TOPMOST, "Always on top", s.float_topmost, true);
    add(fl, FL_CLICK, "Click-through (undo from the tray icon)", s.float_clickthrough, true);
    add(fl, FL_LOCK, "Lock position", s.float_locked, true);
    add(fl, FL_TRANSPARENT, "Transparent background", s.float_transparent, true);
    sep(fl);
    let op = sub(fl, "Opacity");
    for p in (30..=100).rev().step_by(10) {
        add(op, FL_OPACITY_BASE + p / 10, &format!("{p}%"), s.float_opacity as u32 == p, true);
    }
    add(fl, FL_BG, "Background colour...", false, !s.float_transparent);

    add(m, FONT, &format!("Font... ({} {} pt)", s.font_face, s.font_size10 as f32 / 10.0), false, true);
    let col = sub(m, "Text colour");
    add(col, COLOR_AUTO, "Automatic", s.text_color.is_none(), true);
    add(col, COLOR_PICK, "Choose...", s.text_color.is_some(), true);

    let iv = sub(m, "Update interval");
    for (i, (ms, text)) in INTERVALS.iter().enumerate() {
        add(iv, INTERVAL_BASE + i as u32, text, s.interval_ms == *ms, true);
    }
    sep(m);
    add(m, HISTORY, "Traffic history...", false, true);
    add(m, TASKMGR, "Open Task Manager", false, true);
    sep(m);
    add(m, AUTOSTART, "Start with Windows", sys::autostart_enabled(), true);
    add(m, DATA_DIR, "Open data folder", false, true);
    add(m, ABOUT, "About GatiMeter", false, true);
    sep(m);
    add(m, EXIT, "Exit", false, true);
    m
}

/// Shows the menu at screen point (x, y) and runs the chosen command.
pub fn show(x: i32, y: i32) {
    let Some((menu, main)) = with_app(|a| (build(a), a.main)) else { return };
    let cmd = unsafe {
        SetForegroundWindow(main);
        let cmd = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY,
            x,
            y,
            main,
            null(),
        );
        PostMessageW(main, WM_NULL, 0, 0);
        DestroyMenu(menu); // also destroys the submenus
        cmd
    };
    if cmd > 0 {
        run(cmd as u32, main);
    }
}

pub fn show_at_cursor() {
    let mut pt = POINT::default();
    unsafe { GetCursorPos(&mut pt) };
    show(pt.x, pt.y);
}

/// Commands that open a dialog run outside the app borrow, because dialogs
/// pump messages that need the app state.
fn run(cmd: u32, main: HWND) {
    match cmd {
        FONT => {
            let cur = with_app(|a| {
                (a.settings.font_face.clone(), a.settings.font_size10, a.settings.font_bold)
            });
            if let Some((face, size10, bold)) = cur.and_then(|c| choose_font(main, c)) {
                with_app(|a| {
                    a.settings.font_face = face;
                    a.settings.font_size10 = size10;
                    a.settings.font_bold = bold;
                    a.changed();
                });
            }
        }
        COLOR_PICK => {
            let cur = with_app(|a| a.settings.text_color.unwrap_or(0x00FF_FFFF));
            if let Some(c) = cur.and_then(|c| choose_color(main, c)) {
                with_app(|a| {
                    a.settings.text_color = Some(c);
                    a.changed();
                });
            }
        }
        FL_BG => {
            let cur = with_app(|a| a.settings.float_bg);
            if let Some(c) = cur.and_then(|c| choose_color(main, c)) {
                with_app(|a| {
                    a.settings.float_bg = c;
                    a.changed();
                });
            }
        }
        HISTORY => {
            if let Some(text) = with_app(|a| a.history_report()) {
                crate::histwin::show(&text);
            }
        }
        TASKMGR => sys::open_task_manager(),
        AUTOSTART => {
            let on = !sys::autostart_enabled();
            if !sys::set_autostart(on) {
                sys::message_box(main, "Could not change the Windows start-up entry.", "GatiMeter");
            }
        }
        DATA_DIR => sys::shell_open(&sys::data_dir().to_string_lossy()),
        ABOUT => sys::message_box(main, &about_text(), "About GatiMeter"),
        EXIT => unsafe {
            PostMessageW(main, WM_CLOSE, 0, 0);
        },
        _ => {
            with_app(|a| {
                if apply(a, cmd) {
                    a.changed();
                }
            });
        }
    }
}

/// Settings toggles. Returns true when something changed.
fn apply(a: &mut App, cmd: u32) -> bool {
    let s = &mut a.settings;
    match cmd {
        SHOW_TASKBAR => s.show_taskbar = !s.show_taskbar,
        SHOW_FLOAT => s.show_floating = !s.show_floating,
        SHOW_TRAY => s.show_tray_icon = !s.show_tray_icon,
        c if (ITEM_BASE..ITEM_BASE + ITEMS.len() as u32).contains(&c) => {
            let it: Item = ITEMS[(c - ITEM_BASE) as usize];
            let on = !s.has(it);
            s.set_item(it, on);
        }
        ADAPTER_AUTO => s.adapter.clear(),
        c if (ADAPTER_BASE..ADAPTER_BASE + 50).contains(&c) => {
            match a.menu_adapters.get((c - ADAPTER_BASE) as usize) {
                Some(n) => a.settings.adapter = n.clone(),
                None => return false,
            }
        }
        UNIT_BYTES => s.bits = false,
        UNIT_BITS => s.bits = true,
        LABELS => s.labels = !s.labels,
        TB_RIGHT => s.taskbar_left = false,
        TB_LEFT => s.taskbar_left = true,
        FL_TOPMOST => s.float_topmost = !s.float_topmost,
        FL_CLICK => s.float_clickthrough = !s.float_clickthrough,
        FL_LOCK => s.float_locked = !s.float_locked,
        FL_TRANSPARENT => s.float_transparent = !s.float_transparent,
        c if (FL_OPACITY_BASE + 3..=FL_OPACITY_BASE + 10).contains(&c) => {
            s.float_opacity = ((c - FL_OPACITY_BASE) * 10) as u8
        }
        COLOR_AUTO => s.text_color = None,
        c if (INTERVAL_BASE..INTERVAL_BASE + INTERVALS.len() as u32).contains(&c) => {
            s.interval_ms = INTERVALS[(c - INTERVAL_BASE) as usize].0
        }
        _ => return false,
    }
    true
}

fn about_text() -> String {
    format!(
        "GatiMeter {}\n\nNetwork speed and system load on the Windows taskbar.\n\n\
         No kernel driver: every value comes from documented user-mode Windows \
         interfaces (network counters, performance counters, the display driver's \
         own GPU data and the storage temperature query).\n\n\
         CPU temperature comes from the ACPI thermal zone. Many PCs report a fixed \
         value there, so treat it as approximate.\n\n\
         Licence: GPL-3.0-or-later.\nData folder: {}",
        sys::VERSION,
        sys::data_dir().display()
    )
}

fn choose_font(owner: HWND, (face, size10, bold): (String, i32, bool)) -> Option<(String, i32, bool)> {
    unsafe {
        let dc = GetDC(std::ptr::null_mut());
        let dpi = GetDeviceCaps(dc, LOGPIXELSY as _).max(96);
        ReleaseDC(std::ptr::null_mut(), dc);
        let mut lf: LOGFONTW = std::mem::zeroed();
        lf.lfHeight = -((size10 * dpi + 360) / 720);
        lf.lfWeight = if bold { 700 } else { 400 };
        for (d, s) in lf.lfFaceName.iter_mut().zip(face.encode_utf16().take(31)) {
            *d = s;
        }
        let mut cf: CHOOSEFONTW = std::mem::zeroed();
        cf.lStructSize = size_of::<CHOOSEFONTW>() as u32;
        cf.hwndOwner = owner;
        cf.lpLogFont = &mut lf;
        cf.Flags = CF_SCREENFONTS | CF_INITTOLOGFONTSTRUCT | CF_NOVERTFONTS | CF_LIMITSIZE;
        cf.nSizeMin = 6;
        cf.nSizeMax = 36;
        if ChooseFontW(&mut cf) == 0 {
            return None;
        }
        let face = sys::from_wide(&lf.lfFaceName);
        Some((face, cf.iPointSize.clamp(60, 360), lf.lfWeight >= 600))
    }
}

fn choose_color(owner: HWND, initial: u32) -> Option<u32> {
    let mut custom = [0x00FF_FFFFu32; 16];
    unsafe {
        let mut cc: CHOOSECOLORW = std::mem::zeroed();
        cc.lStructSize = size_of::<CHOOSECOLORW>() as u32;
        cc.hwndOwner = owner;
        cc.rgbResult = initial;
        cc.lpCustColors = custom.as_mut_ptr();
        cc.Flags = CC_RGBINIT | CC_FULLOPEN;
        (ChooseColorW(&mut cc) != 0).then_some(cc.rgbResult)
    }
}
