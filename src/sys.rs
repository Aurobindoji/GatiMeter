//! Small Win32 helpers shared by the rest of the program.

use std::path::PathBuf;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, SYSTEMTIME,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_SZ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey,
    RegDeleteValueW, RegGetValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows_sys::Win32::System::SystemInformation::GetLocalTime;
use windows_sys::Win32::System::Threading::CreateMutexW;
use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND, MessageBoxW, SW_SHOWNORMAL,
};

pub const APP_NAME: &str = "GatiMeter";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// UTF-16, NUL-terminated.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads a UTF-16 buffer up to the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Reads a NUL-terminated UTF-16 string from a raw pointer.
///
/// # Safety
/// `p` must be null or point to a NUL-terminated UTF-16 string.
pub unsafe fn from_wide_ptr(p: *const u16) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut len = 0usize;
    unsafe {
        while *p.add(len) != 0 {
            len += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
    }
}

pub fn hinstance() -> windows_sys::Win32::Foundation::HINSTANCE {
    unsafe { GetModuleHandleW(null()) }
}

/// `%APPDATA%\GatiMeter`, created if missing.
pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(APP_NAME);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Holds the single-instance mutex for the life of the process.
pub struct InstanceLock(HANDLE);

impl Drop for InstanceLock {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

/// Returns `None` when another GatiMeter is already running for this user.
pub fn single_instance() -> Option<InstanceLock> {
    let name = wide("Local\\GatiMeter.SingleInstance");
    unsafe {
        let h = CreateMutexW(null(), 0, name.as_ptr());
        if h.is_null() {
            return None;
        }
        if GetLastError() == ERROR_ALREADY_EXISTS {
            CloseHandle(h);
            return None;
        }
        Some(InstanceLock(h))
    }
}

pub fn message_box(owner: HWND, text: &str, title: &str) {
    let t = wide(text);
    let c = wide(title);
    unsafe {
        MessageBoxW(owner, t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND);
    }
}

pub fn shell_open(target: &str) {
    let op = wide("open");
    let file = wide(target);
    unsafe {
        ShellExecuteW(null_mut(), op.as_ptr(), file.as_ptr(), null(), null(), SW_SHOWNORMAL);
    }
}

pub fn open_task_manager() {
    shell_open("taskmgr.exe");
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub y: i32,
    pub m: u32,
    pub d: u32,
}

impl Date {
    pub fn today() -> Date {
        let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut st) };
        Date { y: st.wYear as i32, m: st.wMonth as u32, d: st.wDay as u32 }
    }

    pub fn parse(s: &str) -> Option<Date> {
        let mut it = s.trim().split('-');
        let y = it.next()?.parse().ok()?;
        let m = it.next()?.parse().ok()?;
        let d = it.next()?.parse().ok()?;
        if it.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        Some(Date { y, m, d })
    }
}

impl std::fmt::Display for Date {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.y, self.m, self.d)
    }
}

// ---------------------------------------------------------------- registry

fn reg_get_dword(root: HKEY, sub: &str, name: &str) -> Option<u32> {
    let sub = wide(sub);
    let name = wide(name);
    let mut val = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            root,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            null_mut(),
            &mut val as *mut u32 as *mut _,
            &mut size,
        )
    };
    (rc == 0).then_some(val)
}

fn reg_get_string(root: HKEY, sub: &str, name: &str) -> Option<String> {
    let sub = wide(sub);
    let name = wide(name);
    let mut buf = vec![0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let rc = unsafe {
        RegGetValueW(
            root,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            null_mut(),
            buf.as_mut_ptr() as *mut _,
            &mut size,
        )
    };
    (rc == 0).then(|| from_wide(&buf))
}

/// True when Windows uses the light theme for the taskbar and Start.
pub fn system_uses_light_theme() -> bool {
    reg_get_dword(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "SystemUsesLightTheme",
    ) == Some(1)
}

const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

fn exe_command() -> String {
    let exe = std::env::current_exe().unwrap_or_default();
    format!("\"{}\"", exe.display())
}

/// True when the Run entry exists and points at this exe.
pub fn autostart_enabled() -> bool {
    reg_get_string(HKEY_CURRENT_USER, RUN_KEY, APP_NAME)
        .is_some_and(|v| v.eq_ignore_ascii_case(&exe_command()))
}

pub fn set_autostart(on: bool) -> bool {
    let sub = wide(RUN_KEY);
    let name = wide(APP_NAME);
    unsafe {
        let mut key: HKEY = null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, sub.as_ptr(), 0, KEY_SET_VALUE, &mut key) != 0 {
            return false;
        }
        let rc = if on {
            let data = wide(&exe_command());
            RegSetValueExW(
                key,
                name.as_ptr(),
                0,
                REG_SZ,
                data.as_ptr() as *const u8,
                (data.len() * 2) as u32,
            )
        } else {
            RegDeleteValueW(key, name.as_ptr())
        };
        RegCloseKey(key);
        rc == 0 || (!on && rc == 2) // 2 = value was not there
    }
}
