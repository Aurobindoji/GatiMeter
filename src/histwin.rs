//! A read-only text window: the traffic history (daily and monthly totals)
//! and the SSD write report share it.

use std::cell::Cell;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CreateFontIndirectW, DEFAULT_CHARSET, DeleteObject, HFONT, LOGFONTW,
};
use windows_sys::Win32::UI::Controls::EM_SETSEL;
use windows_sys::Win32::UI::HiDpi::{GetDpiForSystem, GetDpiForWindow};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, ES_AUTOVSCROLL, ES_MULTILINE, ES_READONLY,
    GetClientRect, IsWindow, MoveWindow, SW_SHOWNORMAL, SWP_NOACTIVATE, SWP_NOZORDER,
    SendMessageW, SetForegroundWindow, SetWindowPos, SetWindowTextW, ShowWindow, WM_CREATE,
    WM_DESTROY, WM_DPICHANGED, WM_SETFONT, WM_SIZE, WS_CHILD,
    WM_CTLCOLOREDIT, WM_CTLCOLORSTATIC, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};

use crate::sys::{hinstance, wide};

const CLASS: &str = "GatiMeter.History";

thread_local! {
    static WIN: Cell<HWND> = const { Cell::new(null_mut()) };
    static EDIT: Cell<HWND> = const { Cell::new(null_mut()) };
    static FONT: Cell<HFONT> = const { Cell::new(null_mut()) };
}

pub fn register() {
    crate::app::register_class(CLASS, Some(proc), 0);
}

/// Opens the window (or brings it forward) with `title`, showing `text`.
pub fn show(title: &str, text: &str) {
    let mut hwnd = WIN.with(|w| w.get());
    unsafe {
        if hwnd.is_null() || IsWindow(hwnd) == 0 {
            let dpi = GetDpiForSystem().max(96) as i32;
            let (w, h) = (620 * dpi / 96, 640 * dpi / 96);
            hwnd = CreateWindowExW(
                0,
                wide(CLASS).as_ptr(),
                wide(title).as_ptr(),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                w,
                h,
                null_mut(),
                null_mut(),
                hinstance(),
                null(),
            );
            if hwnd.is_null() {
                return;
            }
            WIN.with(|c| c.set(hwnd));
        }
        SetWindowTextW(hwnd, wide(title).as_ptr());
        // Follow a theme change made while the window was closed or hidden.
        crate::theme::title_bar(hwnd);
        crate::theme::control(EDIT.with(|e| e.get()), "DarkMode_Explorer");
        let edit = EDIT.with(|e| e.get());
        let body = wide(&text.replace('\n', "\r\n"));
        SetWindowTextW(edit, body.as_ptr());
        SendMessageW(edit, EM_SETSEL, 0, 0);
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
    }
}

fn set_font(hwnd: HWND) {
    let dpi = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        d => d as i32,
    };
    let mut lf: LOGFONTW = unsafe { std::mem::zeroed() };
    lf.lfHeight = -(10 * dpi / 72);
    lf.lfWeight = 400;
    lf.lfCharSet = DEFAULT_CHARSET;
    lf.lfQuality = CLEARTYPE_QUALITY;
    for (d, s) in lf.lfFaceName.iter_mut().zip("Consolas".encode_utf16()) {
        *d = s;
    }
    let font = unsafe { CreateFontIndirectW(&lf) };
    let edit = EDIT.with(|e| e.get());
    unsafe { SendMessageW(edit, WM_SETFONT, font as usize, 1) };
    let old = FONT.with(|f| f.replace(font));
    if !old.is_null() {
        unsafe { DeleteObject(old) };
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            let style = WS_CHILD
                | WS_VISIBLE
                | WS_VSCROLL
                | (ES_MULTILINE | ES_READONLY | ES_AUTOVSCROLL) as u32;
            let edit = unsafe {
                CreateWindowExW(
                    0, // no sunken edge: it shows as a white frame in dark mode
                    wide("EDIT").as_ptr(),
                    null(),
                    style,
                    0,
                    0,
                    0,
                    0,
                    hwnd,
                    null_mut(),
                    hinstance(),
                    null(),
                )
            };
            EDIT.with(|e| e.set(edit));
            set_font(hwnd);
            crate::theme::title_bar(hwnd);
            crate::theme::control(edit, "DarkMode_Explorer"); // dark scroll bar
            0
        }
        // A read-only edit box asks with WM_CTLCOLORSTATIC.
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => match crate::theme::ctl_color(wp as _) {
            Some(b) => b as LRESULT,
            None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        },
        WM_SIZE => {
            let mut r = RECT::default();
            unsafe {
                GetClientRect(hwnd, &mut r);
                MoveWindow(EDIT.with(|e| e.get()), 0, 0, r.right, r.bottom, 1);
            }
            0
        }
        WM_DPICHANGED => {
            let r = unsafe { &*(lp as *const RECT) };
            unsafe {
                SetWindowPos(
                    hwnd,
                    null_mut(),
                    r.left,
                    r.top,
                    r.right - r.left,
                    r.bottom - r.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                )
            };
            set_font(hwnd);
            0
        }
        WM_DESTROY => {
            WIN.with(|w| w.set(null_mut()));
            EDIT.with(|e| e.set(null_mut()));
            let f = FONT.with(|f| f.replace(null_mut()));
            if !f.is_null() {
                unsafe { DeleteObject(f) };
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
