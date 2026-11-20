//! "Arrange items" window: tick which items to show and change their order.
//! Every change is saved and drawn at once.

use std::cell::Cell;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontIndirectW, DEFAULT_CHARSET, DeleteObject, FillRect, HBRUSH, HDC,
    HFONT, LOGFONTW,
};
use windows_sys::Win32::UI::Controls::{
    LVCF_TEXT, LVCF_WIDTH, LVCOLUMNW, LVIF_PARAM, LVIF_TEXT, LVIS_FOCUSED, LVIS_SELECTED,
    LVIS_STATEIMAGEMASK, LVITEMW, LVM_DELETEALLITEMS, LVM_ENSUREVISIBLE, LVM_GETHEADER,
    LVM_GETNEXTITEM, LVM_SETBKCOLOR, LVM_SETTEXTBKCOLOR, LVM_SETTEXTCOLOR,
    LVM_INSERTCOLUMNW, LVM_INSERTITEMW, LVM_SETCOLUMNWIDTH, LVM_SETEXTENDEDLISTVIEWSTYLE,
    LVM_SETITEMSTATE, LVM_SETITEMTEXTW, LVN_ITEMCHANGED, LVNI_SELECTED, LVS_EX_CHECKBOXES,
    LVS_EX_DOUBLEBUFFER, LVS_EX_FULLROWSELECT, LVS_REPORT, LVS_SHOWSELALWAYS, LVS_SINGLESEL,
    NMHDR, NMLISTVIEW, WC_LISTVIEWW,
};
use windows_sys::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BS_PUSHBUTTON, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, GetClientRect,
    GetDlgItem, IsDialogMessageW, IsWindow, MSG, MoveWindow, SW_SHOWNORMAL, SWP_NOACTIVATE,
    SWP_NOZORDER, SendMessageW, SetForegroundWindow, SetWindowPos, ShowWindow, WM_APP,
    WM_COMMAND, WM_CREATE, WM_CTLCOLORSTATIC, WM_DESTROY, WM_DPICHANGED, WM_ERASEBKGND,
    WM_NOTIFY, WM_SETFONT, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_EX_CONTROLPARENT, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use crate::app::with_app;
use crate::settings::{ITEMS, Item, Settings};
use crate::sys::{hinstance, wide};

const CLASS: &str = "GatiMeter.Arrange";
const ID_HINT: i32 = 10;
const ID_LIST: i32 = 11;
const ID_UP: i32 = 12;
const ID_DOWN: i32 = 13;
const ID_RESET: i32 = 14;
const ID_CLOSE: i32 = 15;
const IDCANCEL: i32 = 2;
/// Posted to refill the list after a tick-box change (not safe inside the notification).
const WM_APP_REFILL: u32 = WM_APP + 10;

/// Client area at 96 DPI.
const CLIENT_W: i32 = 500;
const CLIENT_H: i32 = 340;

thread_local! {
    static WIN: Cell<HWND> = const { Cell::new(null_mut()) };
    static FONT: Cell<HFONT> = const { Cell::new(null_mut()) };
    /// True while the list is being refilled, so its notifications are ignored.
    static FILLING: Cell<bool> = const { Cell::new(false) };
}

pub fn register() {
    crate::app::register_class_bg(CLASS, Some(proc), 0, (COLOR_BTNFACE + 1) as usize as HBRUSH);
}

/// Opens the window, or brings it forward.
pub fn show() {
    let hwnd = WIN.with(|w| w.get());
    unsafe {
        if !hwnd.is_null() && IsWindow(hwnd) != 0 {
            apply_theme(hwnd);
            fill(None);
            ShowWindow(hwnd, SW_SHOWNORMAL);
            SetForegroundWindow(hwnd);
            return;
        }
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let dpi = GetDpiForSystem().max(96);
        let mut r = RECT { left: 0, top: 0, right: px(CLIENT_W, dpi), bottom: px(CLIENT_H, dpi) };
        AdjustWindowRectExForDpi(&mut r, style, 0, WS_EX_CONTROLPARENT, dpi);
        let hwnd = CreateWindowExW(
            WS_EX_CONTROLPARENT,
            wide(CLASS).as_ptr(),
            wide("GatiMeter - Arrange items").as_ptr(),
            style,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            r.right - r.left,
            r.bottom - r.top,
            null_mut(),
            null_mut(),
            hinstance(),
            null(),
        );
        if hwnd.is_null() {
            return;
        }
        ShowWindow(hwnd, SW_SHOWNORMAL);
        SetForegroundWindow(hwnd);
    }
}

/// Refreshes the list if the window is open (after the menu changed the items).
pub fn refresh() {
    let hwnd = WIN.with(|w| w.get());
    if !hwnd.is_null() && unsafe { IsWindow(hwnd) } != 0 {
        fill(None);
    }
}

/// Gives the window keyboard navigation (Tab, Esc, Space on the tick boxes).
pub fn pre_translate(msg: &MSG) -> bool {
    let hwnd = WIN.with(|w| w.get());
    !hwnd.is_null() && unsafe { IsDialogMessageW(hwnd, msg) } != 0
}

fn px(v: i32, dpi: u32) -> i32 {
    v * dpi as i32 / 96
}

fn item(id: i32) -> HWND {
    unsafe { GetDlgItem(WIN.with(|w| w.get()), id) }
}

fn child(parent: HWND, class: *const u16, text: &str, style: u32, id: i32) -> HWND {
    let t = wide(text);
    unsafe {
        CreateWindowExW(
            0,
            class,
            t.as_ptr(),
            WS_CHILD | WS_VISIBLE | style,
            0,
            0,
            0,
            0,
            parent,
            id as usize as _,
            hinstance(),
            null(),
        )
    }
}

fn create_controls(hwnd: HWND) {
    let stat = wide("STATIC");
    let button = wide("BUTTON");
    child(
        hwnd,
        stat.as_ptr(),
        "Tick an item to show it. Select an item and move it to change its place.\n\
         The taskbar fills column by column (top, then bottom); the floating window \
         lists the items from top to bottom.",
        0,
        ID_HINT,
    );
    let list = child(
        hwnd,
        WC_LISTVIEWW,
        "",
        WS_TABSTOP | WS_BORDER | LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS,
        ID_LIST,
    );
    unsafe {
        let ex = LVS_EX_CHECKBOXES | LVS_EX_FULLROWSELECT | LVS_EX_DOUBLEBUFFER;
        SendMessageW(list, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        for (i, title) in ["Item", "Place"].iter().enumerate() {
            let t = wide(title);
            let mut col: LVCOLUMNW = std::mem::zeroed();
            col.mask = LVCF_TEXT | LVCF_WIDTH;
            col.cx = 100;
            col.pszText = t.as_ptr() as *mut u16;
            SendMessageW(list, LVM_INSERTCOLUMNW, i, &col as *const _ as isize);
        }
    }
    let push = WS_TABSTOP | BS_PUSHBUTTON as u32;
    child(hwnd, button.as_ptr(), "Move &up", push, ID_UP);
    child(hwnd, button.as_ptr(), "Move &down", push, ID_DOWN);
    child(hwnd, button.as_ptr(), "&Reset order", push, ID_RESET);
    child(hwnd, button.as_ptr(), "Close", push, ID_CLOSE);
    apply_theme(hwnd);
}

/// Dark or light window, list and buttons, as Windows apps are now.
fn apply_theme(hwnd: HWND) {
    use crate::theme;
    theme::title_bar(hwnd);
    unsafe {
        let list = GetDlgItem(hwnd, ID_LIST);
        theme::control(list, "DarkMode_Explorer");
        let header = SendMessageW(list, LVM_GETHEADER, 0, 0) as HWND;
        theme::control(header, "DarkMode_ItemsView");
        let (bg, fg) = if theme::dark() {
            (theme::DARK_BG, theme::DARK_TEXT)
        } else {
            (0x00FF_FFFF, 0x0000_0000)
        };
        SendMessageW(list, LVM_SETBKCOLOR, 0, bg as isize);
        SendMessageW(list, LVM_SETTEXTBKCOLOR, 0, bg as isize);
        SendMessageW(list, LVM_SETTEXTCOLOR, 0, fg as isize);
        for id in [ID_UP, ID_DOWN, ID_RESET, ID_CLOSE] {
            theme::control(GetDlgItem(hwnd, id), "DarkMode_Explorer");
        }
    }
}

/// Positions the controls and sets their font for the window's current DPI.
fn layout(hwnd: HWND) {
    let dpi = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 96,
        d => d,
    };
    let p = |v: i32| px(v, dpi);
    let place = |id: i32, x: i32, y: i32, w: i32, h: i32| unsafe {
        MoveWindow(GetDlgItem(hwnd, id), p(x), p(y), p(w), p(h), 1);
    };
    let list_w = 360;
    let item_col = 200;
    place(ID_HINT, 12, 10, CLIENT_W - 24, 50);
    place(ID_LIST, 12, 66, list_w, CLIENT_H - 78);
    let bx = 12 + list_w + 12;
    let bw = CLIENT_W - bx - 12;
    place(ID_UP, bx, 66, bw, 28);
    place(ID_DOWN, bx, 100, bw, 28);
    place(ID_RESET, bx, 146, bw, 28);
    place(ID_CLOSE, bx, CLIENT_H - 12 - 28, bw, 28);
    unsafe {
        let list = GetDlgItem(hwnd, ID_LIST);
        SendMessageW(list, LVM_SETCOLUMNWIDTH, 0, p(item_col) as isize);
        SendMessageW(list, LVM_SETCOLUMNWIDTH, 1, p(list_w - item_col - 24) as isize);
    }

    let mut lf: LOGFONTW = unsafe { std::mem::zeroed() };
    lf.lfHeight = -(9 * dpi as i32 / 72);
    lf.lfWeight = 400;
    lf.lfCharSet = DEFAULT_CHARSET;
    for (d, s) in lf.lfFaceName.iter_mut().zip("Segoe UI".encode_utf16()) {
        *d = s;
    }
    let font = unsafe { CreateFontIndirectW(&lf) };
    for id in [ID_HINT, ID_LIST, ID_UP, ID_DOWN, ID_RESET, ID_CLOSE] {
        unsafe { SendMessageW(GetDlgItem(hwnd, id), WM_SETFONT, font as usize, 1) };
    }
    let old = FONT.with(|f| f.replace(font));
    if !old.is_null() {
        unsafe { DeleteObject(old) };
    }
}

fn selected_row() -> Option<usize> {
    let r = unsafe { SendMessageW(item(ID_LIST), LVM_GETNEXTITEM, usize::MAX, LVNI_SELECTED as isize) };
    (r >= 0).then_some(r as usize)
}

fn set_state(list: HWND, row: usize, state: u32, mask: u32) {
    let mut it: LVITEMW = unsafe { std::mem::zeroed() };
    it.state = state;
    it.stateMask = mask;
    unsafe { SendMessageW(list, LVM_SETITEMSTATE, row, &it as *const _ as isize) };
}

/// Rebuilds the list from the settings and selects `select` (or keeps the
/// current selection).
fn fill(select: Option<Item>) {
    let Some((order, items, rows)) =
        with_app(|a| (a.settings.order.clone(), a.settings.items.clone(), a.layout_rows()))
    else {
        return;
    };
    let list = item(ID_LIST);
    let select = select.or_else(|| selected_row().and_then(|r| order.get(r).copied()));
    FILLING.with(|f| f.set(true));
    unsafe {
        SendMessageW(list, LVM_DELETEALLITEMS, 0, 0);
        for (row, it) in order.iter().enumerate() {
            let name = wide(it.menu_text());
            let mut lv: LVITEMW = std::mem::zeroed();
            lv.mask = LVIF_TEXT | LVIF_PARAM;
            lv.iItem = row as i32;
            lv.pszText = name.as_ptr() as *mut u16;
            lv.lParam = ITEMS.iter().position(|x| x == it).unwrap_or(0) as isize;
            SendMessageW(list, LVM_INSERTITEMW, 0, &lv as *const _ as isize);

            let place = match items.iter().position(|x| x == it) {
                Some(i) => Settings::place_text(i, rows),
                None => "hidden".to_string(),
            };
            let place = wide(&place);
            let mut sub: LVITEMW = std::mem::zeroed();
            sub.iSubItem = 1;
            sub.pszText = place.as_ptr() as *mut u16;
            SendMessageW(list, LVM_SETITEMTEXTW, row, &sub as *const _ as isize);

            // State image 2 = ticked, 1 = empty box.
            let shown = items.contains(it);
            set_state(list, row, if shown { 2 << 12 } else { 1 << 12 }, LVIS_STATEIMAGEMASK);
        }
        if let Some(row) = select.and_then(|s| order.iter().position(|x| *x == s)) {
            set_state(list, row, LVIS_SELECTED | LVIS_FOCUSED, LVIS_SELECTED | LVIS_FOCUSED);
            SendMessageW(list, LVM_ENSUREVISIBLE, row, 0);
        }
    }
    FILLING.with(|f| f.set(false));
    update_buttons();
}

fn update_buttons() {
    let row = selected_row();
    let n = ITEMS.len();
    unsafe {
        EnableWindow(item(ID_UP), row.is_some_and(|r| r > 0) as i32);
        EnableWindow(item(ID_DOWN), row.is_some_and(|r| r + 1 < n) as i32);
    }
}

fn move_selected(up: bool) {
    let Some(row) = selected_row() else { return };
    let moved = with_app(|a| {
        let it = *a.settings.order.get(row)?;
        if a.settings.move_item(it, up) {
            a.changed();
            Some(it)
        } else {
            None
        }
    })
    .flatten();
    if let Some(it) = moved {
        fill(Some(it));
    }
    unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus(item(ID_LIST)) };
}

fn on_list_changed(nm: &NMLISTVIEW) {
    if FILLING.with(|f| f.get()) || nm.iItem < 0 {
        return;
    }
    let changed = nm.uNewState ^ nm.uOldState;
    if changed & LVIS_STATEIMAGEMASK != 0 && nm.uOldState & LVIS_STATEIMAGEMASK != 0 {
        let on = (nm.uNewState & LVIS_STATEIMAGEMASK) >> 12 == 2;
        let row = nm.iItem as usize;
        with_app(|a| {
            if let Some(&it) = a.settings.order.get(row) {
                a.settings.set_item(it, on);
                a.changed();
            }
        });
        // Refill later: the list must not be rebuilt inside its own notification.
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW(
                WIN.with(|w| w.get()),
                WM_APP_REFILL,
                0,
                0,
            )
        };
    }
    if changed & LVIS_SELECTED != 0 {
        update_buttons();
    }
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_CREATE => {
            WIN.with(|w| w.set(hwnd));
            create_controls(hwnd);
            layout(hwnd);
            fill(Some(ITEMS[0]));
            0
        }
        WM_COMMAND => {
            match (wp & 0xFFFF) as i32 {
                ID_UP => move_selected(true),
                ID_DOWN => move_selected(false),
                ID_RESET => {
                    let sel = selected_row();
                    let keep = with_app(|a| {
                        let keep = sel.and_then(|r| a.settings.order.get(r).copied());
                        a.settings.reset_order();
                        a.changed();
                        keep
                    })
                    .flatten();
                    fill(keep);
                }
                ID_CLOSE | IDCANCEL => unsafe {
                    DestroyWindow(hwnd);
                },
                _ => {}
            }
            0
        }
        WM_NOTIFY => {
            let hdr = unsafe { &*(lp as *const NMHDR) };
            if hdr.idFrom == ID_LIST as usize && hdr.code == LVN_ITEMCHANGED {
                on_list_changed(unsafe { &*(lp as *const NMLISTVIEW) });
            }
            0
        }
        WM_APP_REFILL => {
            fill(None);
            0
        }
        WM_ERASEBKGND if crate::theme::dark() => {
            let mut r = RECT::default();
            unsafe {
                GetClientRect(hwnd, &mut r);
                FillRect(wp as HDC, &r, crate::theme::dark_brush());
            }
            1
        }
        WM_CTLCOLORSTATIC => match crate::theme::ctl_color(wp as HDC) {
            Some(b) => b as LRESULT,
            None => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
        },
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
            layout(hwnd);
            0
        }
        WM_DESTROY => {
            WIN.with(|w| w.set(null_mut()));
            let f = FONT.with(|f| f.replace(null_mut()));
            if !f.is_null() {
                unsafe { DeleteObject(f) };
            }
            0
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
    }
}
