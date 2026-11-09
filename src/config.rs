use serde::Deserialize;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::widgets::KINDS;

#[derive(Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(default)]
pub struct WPos {
    pub enabled: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
}

impl Default for WPos {
    fn default() -> Self {
        WPos { enabled: true, x: None, y: None }
    }
}

#[derive(Deserialize, Clone, PartialEq, Debug)]
#[serde(default)]
pub struct Config {
    pub theme: String,
    pub accent: String,
    pub glass: f32,
    pub scale: f32,
    pub clock_24h: bool,
    pub analog_seconds: bool,
    pub week_start: String,
    pub city: String,
    pub units: String,
    pub locked: bool,
    pub visualizer: bool,
    /// Now playing look: "card", "island" or "taskbar".
    pub media_style: String,
    /// Island position (its own spot, so switching styles keeps both places).
    pub island_x: Option<i32>,
    pub island_y: Option<i32>,
    /// Keep the island above open windows (hidden while an app is full screen).
    pub island_top: bool,
    /// Visualizer bars in the taskbar style of Now playing.
    pub taskbar_vis: bool,
    /// Colored outline around the island and taskbar pill (so it shows on black screens).
    pub island_outline: bool,
    pub island_outline_color: String,
    /// GPU ring in the System widget, and which adapter ("" = the main one).
    pub gpu_on: bool,
    pub gpu: String,
    /// Photos widget folder ("" = Pictures) and minutes between photos.
    pub photo_dir: String,
    pub photo_every: u32,
    /// Show message text in the Messages widget (off = "New message").
    pub msg_preview: bool,
    /// Desktop pet on the taskbar: on/off, behavior, walking speed and size.
    pub pet_on: bool,
    pub pet_mode: String,
    pub pet_speed: String,
    pub pet_size: String,
    pub widgets: BTreeMap<String, WPos>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: "auto".into(),
            accent: "#FF6A88".into(),
            glass: 1.0,
            scale: 1.0,
            clock_24h: true,
            analog_seconds: true,
            week_start: "sunday".into(),
            city: "Colombo".into(),
            units: "metric".into(),
            locked: false,
            visualizer: true,
            media_style: "card".into(),
            island_x: None,
            island_y: None,
            island_top: false,
            taskbar_vis: true,
            island_outline: false,
            island_outline_color: "#FFFFFF".into(),
            gpu_on: true,
            gpu: String::new(),
            photo_dir: String::new(),
            photo_every: 60,
            msg_preview: true,
            pet_on: true,
            pet_mode: "roam".into(),
            pet_speed: "normal".into(),
            pet_size: "medium".into(),
            widgets: BTreeMap::new(),
        }
    }
}

impl Config {
    pub fn pos(&self, key: &str) -> WPos {
        self.widgets.get(key).copied().unwrap_or_default()
    }
    pub fn pos_mut(&mut self, key: &str) -> &mut WPos {
        self.widgets.entry(key.to_string()).or_default()
    }

    pub fn island(&self) -> bool {
        self.media_style.eq_ignore_ascii_case("island")
    }

    pub fn taskbar(&self) -> bool {
        self.media_style.eq_ignore_ascii_case("taskbar")
    }

    /// 0 = card, 1 = island, 2 = taskbar.
    pub fn media_style_index(&self) -> u8 {
        if self.island() {
            1
        } else if self.taskbar() {
            2
        } else {
            0
        }
    }

    pub fn to_toml(&self) -> String {
        let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let mut s = String::new();
        s.push_str("# Frost Widgets settings. Save this file and changes apply instantly.\n\n");
        let _ = writeln!(s, "# auto, light or dark\ntheme = \"{}\"", q(&self.theme));
        let _ = writeln!(s, "# Highlight color (calendar, second hand, reminders)\naccent = \"{}\"", q(&self.accent));
        let _ = writeln!(s, "# Glass tint strength, 0.0 (clear) to 2.0 (frosted)\nglass = {:.2}", self.glass);
        let _ = writeln!(s, "# Widget size multiplier on top of Windows display scaling\nscale = {:.2}", self.scale);
        let _ = writeln!(s, "clock_24h = {}", self.clock_24h);
        let _ = writeln!(s, "# Seconds hand on the analog clock (redraws once a second)\nanalog_seconds = {}", self.analog_seconds);
        let _ = writeln!(s, "# sunday or monday\nweek_start = \"{}\"", q(&self.week_start));
        let _ = writeln!(s, "# City for the weather and clock widgets\ncity = \"{}\"", q(&self.city));
        let _ = writeln!(s, "# metric or imperial\nunits = \"{}\"", q(&self.units));
        let _ = writeln!(s, "# true stops widgets from being dragged\nlocked = {}", self.locked);
        let _ = writeln!(s, "# Animated bars under the album art while music plays\nvisualizer = {}", self.visualizer);
        let _ = writeln!(s, "# Now playing style: card, island (a notch-style bar) or taskbar\nmedia_style = \"{}\"", q(&self.media_style));
        if let (Some(x), Some(y)) = (self.island_x, self.island_y) {
            let _ = writeln!(s, "island_x = {x}\nisland_y = {y}");
        }
        let _ = writeln!(s, "# Island stays above open windows (hidden in full screen)\nisland_top = {}", self.island_top);
        let _ = writeln!(s, "# Visualizer bars in the taskbar style of Now playing\ntaskbar_vis = {}", self.taskbar_vis);
        let _ = writeln!(s, "# GPU ring in the System widget, and the graphics card name (empty = main GPU)\ngpu_on = {}\ngpu = \"{}\"", self.gpu_on, q(&self.gpu));
        let _ = writeln!(s, "# Photos widget folder (empty = Pictures) and minutes between photos\nphoto_dir = \"{}\"\nphoto_every = {}", q(&self.photo_dir), self.photo_every);
        let _ = writeln!(s, "# Show message text in the Messages widget\nmsg_preview = {}", self.msg_preview);
        let _ = writeln!(s, "# Desktop pet on the taskbar\npet_on = {}", self.pet_on);
        let _ = writeln!(s, "# roam, calm, walk, laptop or nap\npet_mode = \"{}\"", q(&self.pet_mode));
        let _ = writeln!(s, "# slow, normal or fast\npet_speed = \"{}\"", q(&self.pet_speed));
        let _ = writeln!(s, "# small, medium or large\npet_size = \"{}\"", q(&self.pet_size));
        s.push_str("\n# Widget positions are in screen pixels. Delete x/y to reset a widget.\n");
        for k in KINDS {
            let p = self.pos(k.key());
            let _ = writeln!(s, "\n[widgets.{}]\nenabled = {}", k.key(), p.enabled);
            if let (Some(x), Some(y)) = (p.x, p.y) {
                let _ = writeln!(s, "x = {x}\ny = {y}");
            }
        }
        s
    }
}

/// Portable mode when config.toml sits next to the exe, otherwise %APPDATA%\FrostWidgets.
pub fn dir() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(d) = exe.parent() {
            if d.join("config.toml").exists() {
                return d.to_path_buf();
            }
        }
    }
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".into());
    let d = PathBuf::from(base).join("FrostWidgets");
    let _ = std::fs::create_dir_all(&d);
    d
}

pub fn load(dir: &Path) -> Config {
    let p = dir.join("config.toml");
    match std::fs::read_to_string(&p) {
        Ok(t) => toml::from_str(&t).unwrap_or_default(),
        Err(_) => {
            let c = Config::default();
            save(dir, &c);
            c
        }
    }
}

pub fn save(dir: &Path, c: &Config) {
    let _ = std::fs::write(dir.join("config.toml"), c.to_toml());
}

pub fn mtime(p: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

const DEFAULT_NOTES: &str = "[x] Install Frost widgets\n[ ] Pick a wallpaper you love\n[ ] Right-click a widget for options\n[ ] Edit this list from the tray menu\n";

pub fn load_notes(dir: &Path) -> Vec<(bool, String)> {
    let p = dir.join("notes.txt");
    let text = std::fs::read_to_string(&p).unwrap_or_else(|_| {
        let _ = std::fs::write(&p, DEFAULT_NOTES);
        DEFAULT_NOTES.to_string()
    });
    text.lines()
        .filter_map(|l| {
            let l = l.trim().trim_start_matches("- ").trim();
            if l.is_empty() {
                return None;
            }
            if let Some(r) = l.strip_prefix("[x]").or_else(|| l.strip_prefix("[X]")) {
                Some((true, r.trim().to_string()))
            } else if let Some(r) = l.strip_prefix("[ ]") {
                Some((false, r.trim().to_string()))
            } else {
                Some((false, l.to_string()))
            }
        })
        .collect()
}

pub fn save_notes(dir: &Path, notes: &[(bool, String)]) {
    let mut s = String::new();
    for (d, t) in notes {
        let _ = writeln!(s, "[{}] {}", if *d { "x" } else { " " }, t);
    }
    let _ = std::fs::write(dir.join("notes.txt"), s);
}
