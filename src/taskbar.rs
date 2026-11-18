//! The readout inside the Windows taskbar: a layered child window of
//! `Shell_TrayWnd`, placed just left of the notification area (or at the left end).

use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, FindWindowExW, FindWindowW,
    GW_CHILD, GetClientRect, GetCursorPos, GetWindow, GetWindowRect, HWND_TOP, IsWindow,
    MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos,
    WM_LBUTTONDBLCLK, WM_MOUSEACTIVATE, WM_RBUTTONUP, WS_CHILD, WS_CLIPSIBLINGS, WS_EX_LAYERED,
};
use windows_sys::Win32::Graphics::Gdi::MapWindowPoints;

use crate::render::{Background, Cell, Font, Grid, Ink, Renderer};
use crate::settings::Settings;
use crate::sys::{hinstance, wide};

const CLASS: &str = "GatiMeter.Taskbar";

pub fn register() {
    crate::app::register_class(CLASS, Some(proc), CS_DBLCLKS);
}

pub struct TaskbarWin {
    pub hwnd: HWND,
    tray: HWND,
    renderer: Renderer,
    font: Option<Font>,
    /// Last size drawn, used by `place` between redraws.
    size: (i32, i32),
    last_rect: (i32, i32, i32, i32),
}

impl TaskbarWin {
    pub fn create() -> Option<TaskbarWin> {
        unsafe {
            let tray = FindWindowW(wide("Shell_TrayWnd").as_ptr(), null());
            if tray.is_null() {
                return None;
            }
            let class = wide(CLASS);
            let title = wide("GatiMeter");
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED,
                class.as_ptr(),
                title.as_ptr(),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                1,
                1,
                tray,
                null_mut(),
                hinstance(),
                null(),
            );
            if hwnd.is_null() {
                return None;
            }
            Some(TaskbarWin {
                hwnd,
                tray,
                renderer: Renderer::new(),
                font: None,
                size: (0, 0),
                last_rect: (i32::MIN, 0, 0, 0),
            })
        }
    }

    pub fn alive(&self) -> bool {
        unsafe { IsWindow(self.hwnd) != 0 && IsWindow(self.tray) != 0 }
    }

    fn dpi(&self) -> u32 {
        match unsafe { GetDpiForWindow(self.tray) } {
            0 => 96,
            d => d,
        }
    }

    fn horizontal(&self) -> bool {
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.tray, &mut r) };
        (r.right - r.left) >= (r.bottom - r.top)
    }

    /// Lays out, places and draws. Returns false if the taskbar went away.
    pub fn update(&mut self, cells: &[Cell], s: &Settings, light_taskbar: bool) -> bool {
        if !self.alive() {
            return false;
        }
        let dpi = self.dpi();
        if self.font.as_ref().is_none_or(|f| !f.matches(&s.font_face, s.font_size10, s.font_bold, dpi)) {
            self.font = Some(Font::new(&s.font_face, s.font_size10, s.font_bold, dpi));
        }
        let px = |v: i32| v * dpi as i32 / 96;

        let mut client = RECT::default();
        unsafe { GetClientRect(self.tray, &mut client) };
        let text = s.text_color.unwrap_or(if light_taskbar { 0x0000_0000 } else { 0x00FF_FFFF });
        let value = Ink { color: text, alpha: 255 };
        let label = Ink { color: text, alpha: 200 };
        let horizontal = self.horizontal();
        let font = self.font.as_ref().unwrap();
        let grid = if horizontal {
            let h = client.bottom - client.top;
            let rows = if h >= 2 * font.line_h + px(4) { 2 } else { 1 };
            Grid { rows, pad_x: px(4), pad_y: 0, col_gap: px(10), label_gap: px(4), min_w: 0, min_h: h }
        } else {
            let w = client.right - client.left;
            Grid {
                rows: cells.len().max(1),
                pad_x: px(2),
                pad_y: px(4),
                col_gap: 0,
                label_gap: px(4),
                min_w: w,
                min_h: 0,
            }
        };
        let lay = self.renderer.layout(font, cells, &grid, label, value);
        self.size = (lay.w, lay.h);
        self.place(s);
        // Alpha 1 is invisible but keeps the mouse on this window between the letters.
        let bg = Background { color: 0, alpha: 1, radius: 0 };
        let font = self.font.as_ref().unwrap();
        self.renderer.present(self.hwnd, None, font, &lay, bg, 255);
        true
    }

    /// Moves the window to its spot and keeps it above the taskbar's own content.
    pub fn place(&mut self, s: &Settings) {
        if !self.alive() || self.size.0 == 0 {
            return;
        }
        let (w, h) = self.size;
        let margin = 2 * self.dpi() as i32 / 96;
        let mut client = RECT::default();
        let mut notify = RECT::default();
        let have_notify = unsafe {
            GetClientRect(self.tray, &mut client);
            let n = FindWindowExW(self.tray, null_mut(), wide("TrayNotifyWnd").as_ptr(), null());
            if n.is_null() {
                false
            } else {
                GetWindowRect(n, &mut notify);
                MapWindowPoints(null_mut(), self.tray, &mut notify as *mut RECT as *mut POINT, 2);
                true
            }
        };
        let (x, y) = if self.horizontal() {
            let x = if s.taskbar_left {
                margin
            } else if have_notify {
                notify.left - w - margin
            } else {
                client.right - w - margin
            };
            (x.max(0), 0)
        } else {
            let y = if s.taskbar_left {
                margin
            } else if have_notify {
                notify.top - h - margin
            } else {
                client.bottom - h - margin
            };
            (0, y.max(0))
        };
        unsafe {
            if self.last_rect != (x, y, w, h) {
                self.last_rect = (x, y, w, h);
                SetWindowPos(self.hwnd, HWND_TOP, x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
            } else if GetWindow(self.tray, GW_CHILD) != self.hwnd {
                // Explorer reordered its children; come back on top.
                SetWindowPos(
                    self.hwnd,
                    HWND_TOP,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
                );
            }
        }
    }
}

impl Drop for TaskbarWin {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(self.hwnd) != 0 {
                DestroyWindow(self.hwnd);
            }
        }
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_RBUTTONUP => {
            let mut pt = POINT::default();
            unsafe { GetCursorPos(&mut pt) };
            crate::menu::show(pt.x, pt.y);
            0
        }
        WM_LBUTTONDBLCLK => {
            crate::sys::open_task_manager();
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
