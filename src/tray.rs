//! Notification-area icon and the hover tooltip of the readout windows.

use std::mem::size_of;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::Controls::{
    TOOLTIPS_CLASSW, TTDT_AUTOPOP, TTF_IDISHWND, TTF_SUBCLASS, TTM_ADDTOOLW, TTM_DELTOOLW,
    TTM_SETDELAYTIME, TTM_SETMAXTIPWIDTH, TTM_UPDATETIPTEXTW, TTS_ALWAYSTIP, TTS_NOPREFIX,
    TTTOOLINFOW,
};
use windows_sys::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CW_USEDEFAULT, CreateWindowExW, DestroyIcon, DestroyWindow, GetSystemMetrics, HICON,
    IMAGE_ICON, LR_DEFAULTCOLOR, LoadImageW, SM_CXSMICON, SM_CYSMICON, SendMessageW, WM_APP,
    WS_EX_TOPMOST, WS_POPUP,
};

use crate::sys::{hinstance, wide};

pub const WM_APP_TRAY: u32 = WM_APP + 2;
pub const NIN_KEYSELECT: u32 = 1025;

pub struct Tray {
    hwnd: HWND,
    icon: HICON,
    shown: bool,
    tip: String,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        let icon = unsafe {
            LoadImageW(
                hinstance(),
                1 as _, // the application icon, resource id 1
                IMAGE_ICON,
                GetSystemMetrics(SM_CXSMICON),
                GetSystemMetrics(SM_CYSMICON),
                LR_DEFAULTCOLOR,
            )
        };
        Tray { hwnd, icon, shown: false, tip: "GatiMeter".into() }
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut d: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
        d.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        d.hWnd = self.hwnd;
        d.uID = 1;
        d.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
        d.uCallbackMessage = WM_APP_TRAY;
        d.hIcon = self.icon;
        for (dst, src) in d.szTip.iter_mut().take(127).zip(self.tip.encode_utf16()) {
            *dst = src;
        }
        d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
        d
    }

    pub fn set_visible(&mut self, on: bool) {
        unsafe {
            if on && !self.shown {
                let d = self.data();
                if Shell_NotifyIconW(NIM_ADD, &d) != 0 {
                    Shell_NotifyIconW(NIM_SETVERSION, &d);
                    self.shown = true;
                }
            } else if !on && self.shown {
                Shell_NotifyIconW(NIM_DELETE, &self.data());
                self.shown = false;
            }
        }
    }

    /// After Explorer restarts, every icon has to be added again.
    pub fn readd(&mut self) {
        if self.shown {
            self.shown = false;
            self.set_visible(true);
        }
    }

    pub fn set_tip(&mut self, tip: &str) {
        if self.tip == tip {
            return;
        }
        self.tip = tip.to_string();
        if self.shown {
            unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.data()) };
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.set_visible(false);
        if !self.icon.is_null() {
            unsafe { DestroyIcon(self.icon) };
        }
    }
}

/// One tooltip control shared by the taskbar and floating windows.
pub struct Tooltip {
    tip: HWND,
    text: Vec<u16>,
    tools: Vec<HWND>,
}

impl Tooltip {
    pub fn new(owner: HWND) -> Tooltip {
        let tip = unsafe {
            CreateWindowExW(
                WS_EX_TOPMOST,
                TOOLTIPS_CLASSW,
                null(),
                WS_POPUP | TTS_NOPREFIX | TTS_ALWAYSTIP,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                owner,
                null_mut(),
                hinstance(),
                null(),
            )
        };
        if !tip.is_null() {
            unsafe {
                SendMessageW(tip, TTM_SETMAXTIPWIDTH, 0, 600);
                SendMessageW(tip, TTM_SETDELAYTIME, TTDT_AUTOPOP as usize, 30_000);
            }
        }
        Tooltip { tip, text: wide("GatiMeter"), tools: Vec::new() }
    }

    fn info(&self, hwnd: HWND) -> TTTOOLINFOW {
        let mut ti: TTTOOLINFOW = unsafe { std::mem::zeroed() };
        ti.cbSize = size_of::<TTTOOLINFOW>() as u32;
        ti.uFlags = TTF_IDISHWND | TTF_SUBCLASS;
        ti.hwnd = hwnd;
        ti.uId = hwnd as usize;
        ti.lpszText = self.text.as_ptr() as *mut u16;
        ti
    }

    pub fn add(&mut self, hwnd: HWND) {
        if self.tip.is_null() || self.tools.contains(&hwnd) {
            return;
        }
        let ti = self.info(hwnd);
        unsafe { SendMessageW(self.tip, TTM_ADDTOOLW, 0, &ti as *const _ as LPARAM) };
        self.tools.push(hwnd);
    }

    pub fn remove(&mut self, hwnd: HWND) {
        if let Some(i) = self.tools.iter().position(|&h| h == hwnd) {
            let ti = self.info(hwnd);
            unsafe { SendMessageW(self.tip, TTM_DELTOOLW, 0, &ti as *const _ as LPARAM) };
            self.tools.remove(i);
        }
    }

    pub fn set_text(&mut self, text: &str) {
        let w = wide(text);
        if w == self.text {
            return;
        }
        self.text = w;
        for &h in &self.tools {
            let ti = self.info(h);
            unsafe { SendMessageW(self.tip, TTM_UPDATETIPTEXTW, 0, &ti as *const _ as LPARAM) };
        }
    }
}

impl Drop for Tooltip {
    fn drop(&mut self) {
        if !self.tip.is_null() {
            unsafe { DestroyWindow(self.tip) };
        }
    }
}
