//! Drawing for the layered windows.
//!
//! Text is drawn with GDI, white on black, into a mask bitmap. The mask's
//! grey level is the text coverage, which is then blended in the wanted colour
//! onto a premultiplied-alpha bitmap. That bitmap goes to the screen with
//! `UpdateLayeredWindow`, so the text looks right on any taskbar colour.

use std::mem::{size_of, zeroed};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HWND, POINT, SIZE};
use windows_sys::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER,
    BLENDFUNCTION, CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, DEFAULT_CHARSET,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, ExtTextOutW, GdiFlush, GetTextExtentPoint32W,
    GetTextMetricsW, HBITMAP, HDC, HFONT, HGDIOBJ, LOGFONTW, SelectObject, SetBkMode,
    SetTextColor, TEXTMETRICW, TRANSPARENT,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{ULW_ALPHA, UpdateLayeredWindow};

// ---------------------------------------------------------------- surface

struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    w: i32,
    h: i32,
}

impl Surface {
    fn new(w: i32, h: i32) -> Option<Surface> {
        unsafe {
            let dc = CreateCompatibleDC(null_mut());
            if dc.is_null() {
                return None;
            }
            let mut bi: BITMAPINFO = zeroed();
            bi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = w;
            bi.bmiHeader.biHeight = -h; // top-down rows
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            bi.bmiHeader.biCompression = BI_RGB;
            let mut bits = null_mut();
            let bmp = CreateDIBSection(dc, &bi, DIB_RGB_COLORS, &mut bits, null_mut(), 0);
            if bmp.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let old = SelectObject(dc, bmp);
            Some(Surface { dc, bmp, old, bits: bits as *mut u32, w, h })
        }
    }

    fn pixels(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bmp);
            DeleteDC(self.dc);
        }
    }
}

// ------------------------------------------------------------------- font

pub struct Font {
    h: HFONT,
    pub line_h: i32,
    key: (String, i32, bool, u32),
}

impl Font {
    pub fn new(face: &str, size10: i32, bold: bool, dpi: u32) -> Font {
        unsafe {
            let mut lf: LOGFONTW = zeroed();
            lf.lfHeight = -((size10 as i64 * dpi as i64 + 360) / 720) as i32;
            lf.lfWeight = if bold { 700 } else { 400 };
            lf.lfCharSet = DEFAULT_CHARSET;
            lf.lfQuality = ANTIALIASED_QUALITY;
            for (dst, src) in lf.lfFaceName.iter_mut().zip(face.encode_utf16().take(31)) {
                *dst = src;
            }
            let h = CreateFontIndirectW(&lf);
            let dc = CreateCompatibleDC(null_mut());
            let old = SelectObject(dc, h);
            let mut tm: TEXTMETRICW = zeroed();
            GetTextMetricsW(dc, &mut tm);
            SelectObject(dc, old);
            DeleteDC(dc);
            Font { h, line_h: tm.tmHeight.max(1), key: (face.to_string(), size10, bold, dpi) }
        }
    }

    pub fn matches(&self, face: &str, size10: i32, bold: bool, dpi: u32) -> bool {
        self.key.0 == face && self.key.1 == size10 && self.key.2 == bold && self.key.3 == dpi
    }
}

impl Drop for Font {
    fn drop(&mut self) {
        unsafe { DeleteObject(self.h) };
    }
}

// ----------------------------------------------------------------- layout

pub struct Cell {
    pub label: &'static str,
    pub value: String,
    /// Widest expected value, so the column does not jump as numbers change.
    pub template: &'static str,
}

pub struct Grid {
    pub rows: usize,
    pub pad_x: i32,
    pub pad_y: i32,
    pub col_gap: i32,
    pub label_gap: i32,
    pub min_w: i32,
    pub min_h: i32,
}

#[derive(Clone, Copy)]
pub struct Ink {
    pub color: u32,
    pub alpha: u8,
}

pub struct Text {
    s: Vec<u16>,
    x: i32,
    y: i32,
    ink: Ink,
}

pub struct Layout {
    pub w: i32,
    pub h: i32,
    texts: Vec<Text>,
}

/// Background fill. `alpha` 1 is nearly invisible but still catches the mouse.
#[derive(Clone, Copy)]
pub struct Background {
    pub color: u32,
    pub alpha: u8,
    pub radius: i32,
}

pub struct Renderer {
    measure_dc: HDC,
    surf: Option<Surface>,
    mask: Option<Surface>,
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer { measure_dc: unsafe { CreateCompatibleDC(null_mut()) }, surf: None, mask: None }
    }

    fn width(&self, font: &Font, s: &[u16]) -> i32 {
        if s.is_empty() {
            return 0;
        }
        let mut sz = SIZE { cx: 0, cy: 0 };
        unsafe {
            let old = SelectObject(self.measure_dc, font.h);
            GetTextExtentPoint32W(self.measure_dc, s.as_ptr(), s.len() as i32, &mut sz);
            SelectObject(self.measure_dc, old);
        }
        sz.cx
    }

    /// Places the cells column by column, `g.rows` cells per column. Labels are
    /// left-aligned and values right-aligned within each column.
    pub fn layout(&self, font: &Font, cells: &[Cell], g: &Grid, label: Ink, value: Ink) -> Layout {
        let rows = g.rows.max(1);
        let used_rows = rows.min(cells.len().max(1));
        let line = font.line_h;
        let content_h = used_rows as i32 * line;
        let mut texts = Vec::new();
        let mut columns = Vec::new();
        let mut content_w = 0;
        for col in cells.chunks(rows) {
            let labels: Vec<Vec<u16>> = col.iter().map(|c| c.label.encode_utf16().collect()).collect();
            let values: Vec<Vec<u16>> = col.iter().map(|c| c.value.encode_utf16().collect()).collect();
            let lw = labels.iter().map(|l| self.width(font, l)).max().unwrap_or(0);
            let vw_actual: Vec<i32> = values.iter().map(|v| self.width(font, v)).collect();
            let tw = col
                .iter()
                .map(|c| self.width(font, &c.template.encode_utf16().collect::<Vec<_>>()))
                .max()
                .unwrap_or(0);
            let vw = vw_actual.iter().copied().max().unwrap_or(0).max(tw);
            let value_x = lw + if lw > 0 { g.label_gap } else { 0 };
            if !columns.is_empty() {
                content_w += g.col_gap;
            }
            columns.push((content_w, value_x, labels, values));
            content_w += value_x + vw;
        }
        let w = (content_w + 2 * g.pad_x).max(g.min_w).max(1);
        let h = (content_h + 2 * g.pad_y).max(g.min_h).max(1);
        let x0 = (w - content_w) / 2;
        let y0 = (h - content_h) / 2;
        // Values start right after the label column; the column keeps the
        // template width, so the window does not resize as numbers change.
        for (cx, value_x, labels, values) in columns {
            for (r, (l, v)) in labels.into_iter().zip(values).enumerate() {
                let y = y0 + r as i32 * line;
                if !l.is_empty() {
                    texts.push(Text { s: l, x: x0 + cx, y, ink: label });
                }
                texts.push(Text { s: v, x: x0 + cx + value_x, y, ink: value });
            }
        }
        Layout { w, h, texts }
    }

    /// Draws `lay` and pushes it to `hwnd`. `pos` moves a top-level window at the
    /// same time; child windows pass `None`.
    pub fn present(
        &mut self,
        hwnd: HWND,
        pos: Option<POINT>,
        font: &Font,
        lay: &Layout,
        bg: Background,
        opacity: u8,
    ) -> bool {
        let (w, h) = (lay.w, lay.h);
        if self.surf.as_ref().is_none_or(|s| s.w != w || s.h != h) {
            self.surf = Surface::new(w, h);
            self.mask = Surface::new(w, h);
        }
        let (Some(surf), Some(mask)) = (self.surf.as_mut(), self.mask.as_mut()) else {
            return false;
        };
        fill_background(surf, bg);

        // One mask pass per ink (usually two: labels and values).
        let mut inks: Vec<(u32, u8)> = Vec::new();
        for t in &lay.texts {
            if !inks.contains(&(t.ink.color, t.ink.alpha)) {
                inks.push((t.ink.color, t.ink.alpha));
            }
        }
        for (color, alpha) in inks {
            mask.pixels().fill(0);
            unsafe {
                let old = SelectObject(mask.dc, font.h);
                SetBkMode(mask.dc, TRANSPARENT as i32);
                SetTextColor(mask.dc, 0x00FF_FFFF);
                for t in lay.texts.iter().filter(|t| t.ink.color == color && t.ink.alpha == alpha) {
                    ExtTextOutW(mask.dc, t.x, t.y, 0, null(), t.s.as_ptr(), t.s.len() as u32, null());
                }
                SelectObject(mask.dc, old);
                GdiFlush();
            }
            blend_mask(surf.pixels(), mask.pixels(), color, alpha);
        }

        let size = SIZE { cx: w, cy: h };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: opacity,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let pos_ptr = pos.as_ref().map_or(null(), |p| p as *const POINT);
        unsafe {
            UpdateLayeredWindow(hwnd, null_mut(), pos_ptr, &size, surf.dc, &src, 0, &blend, ULW_ALPHA)
                != 0
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.surf = None;
        self.mask = None;
        unsafe { DeleteDC(self.measure_dc) };
    }
}

// ----------------------------------------------------------------- pixels

fn mul(a: u32, b: u32) -> u32 {
    (a * b + 127) / 255
}

fn rgb(c: u32) -> (u32, u32, u32) {
    (c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF)
}

/// Premultiplied BGRA pixel from a COLORREF and an alpha.
fn premul(color: u32, a: u32) -> u32 {
    let (r, g, b) = rgb(color);
    (a << 24) | (mul(r, a) << 16) | (mul(g, a) << 8) | mul(b, a)
}

fn fill_background(s: &mut Surface, bg: Background) {
    let (w, h) = (s.w, s.h);
    let full = premul(bg.color, bg.alpha as u32);
    let px = s.pixels();
    px.fill(full);
    let r = bg.radius.min(w / 2).min(h / 2);
    if r <= 0 {
        return;
    }
    // Soften the four corners by how much of each pixel lies inside the arc.
    let rf = r as f32;
    for y in 0..r {
        for x in 0..r {
            let dx = rf - (x as f32 + 0.5);
            let dy = rf - (y as f32 + 0.5);
            let cover = (rf - (dx * dx + dy * dy).sqrt() + 0.5).clamp(0.0, 1.0);
            let a = (bg.alpha as f32 * cover).round() as u32;
            let p = premul(bg.color, a);
            for (cx, cy) in [(x, y), (w - 1 - x, y), (x, h - 1 - y), (w - 1 - x, h - 1 - y)] {
                px[(cy * w + cx) as usize] = p;
            }
        }
    }
}

/// Source-over blend of `color` at mask coverage × `alpha` onto `dst`.
fn blend_mask(dst: &mut [u32], mask: &[u32], color: u32, alpha: u8) {
    let (r, g, b) = rgb(color);
    for (d, &m) in dst.iter_mut().zip(mask) {
        let cover = (m >> 8) & 0xFF;
        if cover == 0 {
            continue;
        }
        let a = mul(cover, alpha as u32);
        let inv = 255 - a;
        let (da, dr, dg, db) = ((*d >> 24) & 0xFF, (*d >> 16) & 0xFF, (*d >> 8) & 0xFF, *d & 0xFF);
        let na = a + mul(da, inv);
        let nr = mul(r, a) + mul(dr, inv);
        let ng = mul(g, a) + mul(dg, inv);
        let nb = mul(b, a) + mul(db, inv);
        *d = (na.min(255) << 24) | (nr.min(255) << 16) | (ng.min(255) << 8) | nb.min(255);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiply() {
        assert_eq!(premul(0x00FF_FFFF, 255), 0xFFFF_FFFF);
        assert_eq!(premul(0x0000_00FF, 128), 0x8080_0000);
        assert_eq!(premul(0x0000_0000, 1), 0x0100_0000);
    }

    #[test]
    fn blend_full_and_none() {
        let mut dst = vec![0x0100_0000u32, 0x0100_0000];
        let mask = vec![0x00FF_FF00u32 | 0xFF, 0];
        blend_mask(&mut dst, &mask, 0x00FF_FFFF, 255);
        assert_eq!(dst[0], 0xFFFF_FFFF);
        assert_eq!(dst[1], 0x0100_0000);
    }
}
