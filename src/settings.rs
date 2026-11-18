//! User settings, kept as plain `key=value` lines in
//! `%APPDATA%\GatiMeter\settings.ini`.

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Up,
    Down,
    Cpu,
    Mem,
    Gpu,
    Disk,
    CpuTemp,
    GpuTemp,
    DiskTemp,
}

pub const ITEMS: [Item; 9] = [
    Item::Up,
    Item::Down,
    Item::Cpu,
    Item::Mem,
    Item::Gpu,
    Item::Disk,
    Item::CpuTemp,
    Item::GpuTemp,
    Item::DiskTemp,
];

impl Item {
    pub fn key(self) -> &'static str {
        match self {
            Item::Up => "up",
            Item::Down => "down",
            Item::Cpu => "cpu",
            Item::Mem => "mem",
            Item::Gpu => "gpu",
            Item::Disk => "disk",
            Item::CpuTemp => "cpu_temp",
            Item::GpuTemp => "gpu_temp",
            Item::DiskTemp => "disk_temp",
        }
    }

    /// Short label drawn before the value.
    pub fn label(self) -> &'static str {
        match self {
            Item::Up => "↑",
            Item::Down => "↓",
            Item::Cpu => "CPU",
            Item::Mem => "RAM",
            Item::Gpu => "GPU",
            Item::Disk => "Disk",
            Item::CpuTemp => "CPU°",
            Item::GpuTemp => "GPU°",
            Item::DiskTemp => "Disk°",
        }
    }

    pub fn menu_text(self) -> &'static str {
        match self {
            Item::Up => "Upload speed",
            Item::Down => "Download speed",
            Item::Cpu => "CPU usage",
            Item::Mem => "Memory usage",
            Item::Gpu => "GPU usage",
            Item::Disk => "Disk activity",
            Item::CpuTemp => "CPU temperature (approximate)",
            Item::GpuTemp => "GPU temperature",
            Item::DiskTemp => "Disk temperature",
        }
    }

    /// The arrows stay even with labels off, or up and down could not be told apart.
    pub fn always_labelled(self) -> bool {
        matches!(self, Item::Up | Item::Down)
    }

    fn from_key(k: &str) -> Option<Item> {
        ITEMS.iter().copied().find(|i| i.key() == k)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub show_taskbar: bool,
    pub show_floating: bool,
    pub show_tray_icon: bool,
    /// Shown items, always in `ITEMS` order.
    pub items: Vec<Item>,
    /// Adapter name, or empty for automatic.
    pub adapter: String,
    pub bits: bool,
    pub labels: bool,
    pub taskbar_left: bool,
    pub float_x: Option<i32>,
    pub float_y: Option<i32>,
    pub float_topmost: bool,
    pub float_clickthrough: bool,
    pub float_locked: bool,
    pub float_transparent: bool,
    /// Whole-window opacity in percent (20..=100).
    pub float_opacity: u8,
    /// COLORREF (0x00BBGGRR).
    pub float_bg: u32,
    pub font_face: String,
    /// Font size in tenths of a point.
    pub font_size10: i32,
    pub font_bold: bool,
    /// COLORREF, or `None` to follow the Windows theme / background.
    pub text_color: Option<u32>,
    pub interval_ms: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            show_taskbar: true,
            show_floating: false,
            show_tray_icon: true,
            items: vec![Item::Up, Item::Down, Item::Cpu, Item::Mem],
            adapter: String::new(),
            bits: false,
            labels: true,
            taskbar_left: false,
            float_x: None,
            float_y: None,
            float_topmost: true,
            float_clickthrough: false,
            float_locked: false,
            float_transparent: false,
            float_opacity: 90,
            float_bg: 0x0020_2020,
            font_face: "Segoe UI".into(),
            font_size10: 90,
            font_bold: false,
            text_color: None,
            interval_ms: 1000,
        }
    }
}

fn color_to_text(c: u32) -> String {
    let (r, g, b) = (c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF);
    format!("#{r:02X}{g:02X}{b:02X}")
}

fn color_from_text(s: &str) -> Option<u32> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    let (r, g, b) = ((v >> 16) & 0xFF, (v >> 8) & 0xFF, v & 0xFF);
    Some(r | (g << 8) | (b << 16))
}

fn flag(v: bool) -> &'static str {
    if v { "1" } else { "0" }
}

impl Settings {
    pub fn has(&self, it: Item) -> bool {
        self.items.contains(&it)
    }

    pub fn set_item(&mut self, it: Item, on: bool) {
        self.items.retain(|&x| x != it);
        if on {
            self.items.push(it);
        }
        self.items.sort_by_key(|x| ITEMS.iter().position(|y| y == x));
    }

    pub fn load(path: &Path) -> Settings {
        std::fs::read_to_string(path).map(|t| Settings::from_text(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("ini.tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
    }

    pub fn to_text(&self) -> String {
        let items: Vec<&str> = self.items.iter().map(|i| i.key()).collect();
        let opt = |v: Option<i32>| v.map(|v| v.to_string()).unwrap_or_default();
        let lines = [
            "# GatiMeter settings".to_string(),
            format!("show_taskbar={}", flag(self.show_taskbar)),
            format!("show_floating={}", flag(self.show_floating)),
            format!("show_tray_icon={}", flag(self.show_tray_icon)),
            format!("items={}", items.join(",")),
            format!("adapter={}", self.adapter),
            format!("bits={}", flag(self.bits)),
            format!("labels={}", flag(self.labels)),
            format!("taskbar_left={}", flag(self.taskbar_left)),
            format!("float_x={}", opt(self.float_x)),
            format!("float_y={}", opt(self.float_y)),
            format!("float_topmost={}", flag(self.float_topmost)),
            format!("float_clickthrough={}", flag(self.float_clickthrough)),
            format!("float_locked={}", flag(self.float_locked)),
            format!("float_transparent={}", flag(self.float_transparent)),
            format!("float_opacity={}", self.float_opacity),
            format!("float_bg={}", color_to_text(self.float_bg)),
            format!("font_face={}", self.font_face),
            format!("font_size10={}", self.font_size10),
            format!("font_bold={}", flag(self.font_bold)),
            format!("text_color={}", self.text_color.map(color_to_text).unwrap_or_else(|| "auto".into())),
            format!("interval_ms={}", self.interval_ms),
        ];
        lines.join("\r\n") + "\r\n"
    }

    pub fn from_text(text: &str) -> Settings {
        let mut s = Settings::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.is_empty() {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let b = v == "1" || v.eq_ignore_ascii_case("true");
            match k {
                "show_taskbar" => s.show_taskbar = b,
                "show_floating" => s.show_floating = b,
                "show_tray_icon" => s.show_tray_icon = b,
                "items" => {
                    s.items.clear();
                    for key in v.split(',') {
                        if let Some(it) = Item::from_key(key.trim()) {
                            s.set_item(it, true);
                        }
                    }
                }
                "adapter" => s.adapter = v.to_string(),
                "bits" => s.bits = b,
                "labels" => s.labels = b,
                "taskbar_left" => s.taskbar_left = b,
                "float_x" => s.float_x = v.parse().ok(),
                "float_y" => s.float_y = v.parse().ok(),
                "float_topmost" => s.float_topmost = b,
                "float_clickthrough" => s.float_clickthrough = b,
                "float_locked" => s.float_locked = b,
                "float_transparent" => s.float_transparent = b,
                "float_opacity" => {
                    if let Ok(p) = v.parse::<u8>() {
                        s.float_opacity = p.clamp(20, 100);
                    }
                }
                "float_bg" => {
                    if let Some(c) = color_from_text(v) {
                        s.float_bg = c;
                    }
                }
                "font_face" if !v.is_empty() => s.font_face = v.chars().take(31).collect(),
                "font_size10" => {
                    if let Ok(p) = v.parse::<i32>() {
                        s.font_size10 = p.clamp(60, 360);
                    }
                }
                "font_bold" => s.font_bold = b,
                "text_color" => s.text_color = color_from_text(v),
                "interval_ms" => {
                    if let Ok(p) = v.parse::<u32>() {
                        s.interval_ms = p.clamp(250, 10_000);
                    }
                }
                _ => {}
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = Settings::default();
        s.set_item(Item::GpuTemp, true);
        s.set_item(Item::Mem, false);
        s.adapter = "Wi-Fi".into();
        s.float_x = Some(-120);
        s.text_color = Some(0x0012_3456);
        s.float_bg = 0x00AB_CDEF;
        let back = Settings::from_text(&s.to_text());
        assert_eq!(back, s);
        assert_eq!(back.items, vec![Item::Up, Item::Down, Item::Cpu, Item::GpuTemp]);
    }

    #[test]
    fn bad_values_fall_back() {
        let s = Settings::from_text("float_opacity=5\nfont_size10=9999\nitems=cpu,nonsense,up\ntext_color=auto\n");
        assert_eq!(s.float_opacity, 20);
        assert_eq!(s.font_size10, 360);
        assert_eq!(s.items, vec![Item::Up, Item::Cpu]);
        assert_eq!(s.text_color, None);
    }

    #[test]
    fn colours() {
        assert_eq!(color_from_text("#FF0000"), Some(0x0000_00FF));
        assert_eq!(color_to_text(0x0000_00FF), "#FF0000");
        assert_eq!(color_from_text("FF0000"), None);
    }
}
