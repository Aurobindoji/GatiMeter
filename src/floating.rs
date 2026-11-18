//! The floating readout: a borderless layered window that can be dragged anywhere.

use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, Ordering};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, GWL_EXSTYLE, GetCursorPos,
    GetWindowRect, HTCAPTION, HTCLIENT, HWND_NOTOPMOST, HWND_TOPMOST, IsWindow, MA_NOACTIVATE,
    SPI_GETWORKAREA, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    SetWindowLongPtrW, SetWindowPos, ShowWindow, SystemParametersInfoW, WM_CONTEXTMENU,
    WM_DPICHANGED, WM_EXITSIZEMOVE, WM_LBUTTONDBLCLK, WM_MOUSEACTIVATE, WM_NCHITTEST,
    WM_NCLBUTTONDBLCLK, WM_NCRBUTTONUP, WM_RBUTTONUP, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
};

use crate::render::{Background, Cell, Font, Grid, Ink, Renderer};
use crate::settings::Settings;
use crate::sys::{hinstance, wide};

const CLASS: &str = "GatiMeter.Floating";

/// Read by WM_NCHITTEST, which may arrive while the app state is borrowed.
static LOCKED: AtomicBool = AtomicBool::new(false);

pub fn register() {
    crate::app::register_class(CLASS, Some(proc), CS_DBLCLKS);
}

pub struct FloatWin {
    pub hwnd: HWND,
    renderer: Renderer,
    font: Option<Font>,
}

fn on_a_monitor(x: i32, y: i32) -> bool {
    unsafe { !MonitorFromPoint(POINT { x: x + 8, y: y + 8 }, MONITOR_DEFAULTTONULL).is_null() }
}

impl FloatWin {
    pub fn create(s: &Settings) -> Option<FloatWin> {
        let (x, y) = match (s.float_x, s.float_y) {
            (Some(x), Some(y)) if on_a_monitor(x, y) => (x, y),
            _ => {
                let mut work = RECT::default();
                unsafe {
                    SystemParametersInfoW(SPI_GETWORKAREA, 0, &mut work as *mut RECT as *mut _, 0)
                };
                (work.right - 220, work.bottom - 160)
            }
        };
        let class = wide(CLASS);
        let title = wide("GatiMeter");
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                class.as_ptr(),
                title.as_ptr(),
                WS_POPUP,
                x,
                y,
                1,
                1,
                null_mut(),
                null_mut(),
                hinstance(),
                null(),
            )
        };
        if hwnd.is_null() {
            return None;
        }
        let w = FloatWin { hwnd, renderer: Renderer::new(), font: None };
        w.apply_style(s);
        unsafe { ShowWindow(hwnd, SW_SHOWNOACTIVATE) };
        Some(w)
    }

    /// Topmost, click-through and lock follow the settings.
    pub fn apply_style(&self, s: &Settings) {
        LOCKED.store(s.float_locked, Ordering::Relaxed);
        let mut ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        if s.float_topmost {
            ex |= windows_sys::Win32::UI::WindowsAndMessaging::WS_EX_TOPMOST;
        }
        if s.float_clickthrough {
            ex |= WS_EX_TRANSPARENT;
        }
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex as isize);
            let after = if s.float_topmost { HWND_TOPMOST } else { HWND_NOTOPMOST };
            SetWindowPos(self.hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    pub fn update(&mut self, cells: &[Cell], s: &Settings) {
        let dpi = match unsafe { GetDpiForWindow(self.hwnd) } {
            0 => 96,
            d => d,
        };
        if self.font.as_ref().is_none_or(|f| !f.matches(&s.font_face, s.font_size10, s.font_bold, dpi)) {
            self.font = Some(Font::new(&s.font_face, s.font_size10, s.font_bold, dpi));
        }
        let font = self.font.as_ref().unwrap();
        let px = |v: i32| v * dpi as i32 / 96;
        let text = s.text_color.unwrap_or(if s.float_transparent {
            0x00FF_FFFF
        } else {
            contrast(s.float_bg)
        });
        let grid = Grid {
            rows: cells.len().max(1),
            pad_x: px(8),
            pad_y: px(5),
            col_gap: px(10),
            label_gap: px(10),
            min_w: 0,
            min_h: 0,
        };
        let lay = self.renderer.layout(
            font,
            cells,
            &grid,
            Ink { color: text, alpha: 200 },
            Ink { color: text, alpha: 255 },
        );
        let bg = if s.float_transparent {
            Background { color: 0, alpha: 1, radius: 0 }
        } else {
            Background { color: s.float_bg, alpha: 255, radius: px(6) }
        };
        let opacity = (s.float_opacity.clamp(20, 100) as u32 * 255 / 100) as u8;
        self.renderer.present(self.hwnd, None, font, &lay, bg, opacity);
    }

    pub fn position(&self) -> (i32, i32) {
        let mut r = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut r) };
        (r.left, r.top)
    }
}

impl Drop for FloatWin {
    fn drop(&mut self) {
        unsafe {
            if IsWindow(self.hwnd) != 0 {
                DestroyWindow(self.hwnd);
            }
        }
    }
}

/// Black or white, whichever reads better on `bg`.
fn contrast(bg: u32) -> u32 {
    let (r, g, b) = (bg & 0xFF, (bg >> 8) & 0xFF, (bg >> 16) & 0xFF);
    let luma = 299 * r + 587 * g + 114 * b;
    if luma > 150_000 { 0x0000_0000 } else { 0x00FF_FFFF }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        // Dragging anywhere moves the window, unless it is locked.
        WM_NCHITTEST => {
            if LOCKED.load(Ordering::Relaxed) { HTCLIENT as LRESULT } else { HTCAPTION as LRESULT }
        }
        WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
        WM_NCRBUTTONUP | WM_RBUTTONUP => {
            let mut pt = POINT::default();
            unsafe { GetCursorPos(&mut pt) };
            crate::menu::show(pt.x, pt.y);
            0
        }
        WM_CONTEXTMENU => 0,
        WM_NCLBUTTONDBLCLK | WM_LBUTTONDBLCLK => {
            crate::sys::open_task_manager();
            0
        }
        WM_EXITSIZEMOVE => {
            crate::app::with_app(|a| a.floating_moved());
            0
        }
        WM_DPICHANGED => {
            crate::app::with_app(|a| a.render());
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn contrast_picks_readable_text() {
        assert_eq!(super::contrast(0x0020_2020), 0x00FF_FFFF);
        assert_eq!(super::contrast(0x00F0_F0F0), 0x0000_0000);
    }
}
