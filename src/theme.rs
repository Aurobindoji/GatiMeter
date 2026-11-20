//! Dark or light menus and windows, following the Windows app theme
//! (Settings > Personalisation > Colours > "Choose your app mode").
//!
//! * Menus: uxtheme's `SetPreferredAppMode` and `FlushMenuThemes` (exported
//!   by ordinal 135 and 136 since Windows 10 1903; the same calls other
//!   Win32 programs such as Notepad++ use). If they are missing, menus stay
//!   light.
//! * Title bars: `DwmSetWindowAttribute(DWMWA_USE_IMMERSIVE_DARK_MODE)`.
//! * Controls: the "DarkMode_Explorer" / "DarkMode_ItemsView" visual styles,
//!   plus our own colours for text boxes, labels and lists.

use std::cell::Cell;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{COLORREF, HWND};
use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, HBRUSH, HDC, SetBkColor, SetTextColor};
use windows_sys::Win32::System::LibraryLoader::{
    GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
};
use windows_sys::Win32::UI::Controls::SetWindowTheme;

use crate::sys::{self, wide};

/// Background and text of dark windows (COLORREF, 0x00BBGGRR).
pub const DARK_BG: COLORREF = 0x0020_2020;
pub const DARK_TEXT: COLORREF = 0x00E8_E8E8;

const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
/// PreferredAppMode values.
const FORCE_DARK: i32 = 2;
const FORCE_LIGHT: i32 = 3;

type SetModeFn = unsafe extern "system" fn(i32) -> i32;
type FlushFn = unsafe extern "system" fn();

/// True when Windows apps use the dark theme.
pub fn dark() -> bool {
    !sys::apps_use_light_theme()
}

fn menu_api() -> Option<(SetModeFn, FlushFn)> {
    static API: OnceLock<Option<(SetModeFn, FlushFn)>> = OnceLock::new();
    *API.get_or_init(|| unsafe {
        let lib = LoadLibraryExW(wide("uxtheme.dll").as_ptr(), std::ptr::null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32);
        if lib.is_null() {
            return None;
        }
        // By ordinal: the low word of the "name" pointer.
        let set = GetProcAddress(lib, 135 as *const u8)?;
        let flush = GetProcAddress(lib, 136 as *const u8)?;
        Some((
            std::mem::transmute::<unsafe extern "system" fn() -> isize, SetModeFn>(set),
            std::mem::transmute::<unsafe extern "system" fn() -> isize, FlushFn>(flush),
        ))
    })
}

/// Makes the next popup menu match the current Windows app theme.
pub fn sync_menus() {
    if let Some((set, flush)) = menu_api() {
        unsafe {
            set(if dark() { FORCE_DARK } else { FORCE_LIGHT });
            flush();
        }
    }
}

/// Dark or light title bar for a top-level window.
pub fn title_bar(hwnd: HWND) {
    let on: i32 = dark() as i32;
    unsafe {
        DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &on as *const i32 as _, 4);
    }
}

/// Gives a control the dark visual style `class` (or its normal style when light).
pub fn control(hwnd: HWND, class: &str) {
    let name = wide(if dark() { class } else { "Explorer" });
    unsafe { SetWindowTheme(hwnd, name.as_ptr(), std::ptr::null()) };
}

thread_local! {
    static BRUSH: Cell<HBRUSH> = const { Cell::new(std::ptr::null_mut()) };
}

/// The dark background brush (kept for the life of the program).
pub fn dark_brush() -> HBRUSH {
    BRUSH.with(|b| {
        if b.get().is_null() {
            b.set(unsafe { CreateSolidBrush(DARK_BG) });
        }
        b.get()
    })
}

/// For WM_CTLCOLORSTATIC / WM_CTLCOLOREDIT: the brush to return, or `None`
/// to let Windows draw its normal colours.
pub fn ctl_color(hdc: HDC) -> Option<HBRUSH> {
    if !dark() {
        return None;
    }
    unsafe {
        SetTextColor(hdc, DARK_TEXT);
        SetBkColor(hdc, DARK_BG);
    }
    Some(dark_brush())
}
