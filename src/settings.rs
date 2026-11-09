//! Settings panel (sidebar + cards, Windows 11 style), drawn with Direct2D.
//! Created on demand and destroyed on close, so it costs nothing while closed.
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::config::Config;
use crate::editor::Theme;
use crate::gfx::{Align, Color, Fam, Gfx, Surface, WHITE};
use crate::widgets::KINDS;

pub const W0: f32 = 780.0;
pub const H0: f32 = 600.0;
const SIDE: f32 = 220.0;
const TOP: f32 = 76.0;

pub const ACCENTS: [u32; 7] = [0xFF6A88, 0xFF9F0A, 0xFFD60A, 0x30D158, 0x64D2FF, 0x0A84FF, 0xBF5AF2];
pub const SCALES: [(f32, &str); 4] = [(0.85, "Small"), (1.0, "Default"), (1.15, "Large"), (1.3, "Huge")];

const TABS: [(char, &str); 8] = [
    ('\u{E713}', "General"),
    ('\u{E790}', "Appearance"),
    ('\u{F0E2}', "Widgets"),
    ('\u{E823}', "Clock & weather"),
    ('\u{E7F4}', "Widget options"),
    ('\u{E8D6}', "Dynamic Island"),
    ('\u{E7FC}', "Pet"),
    ('\u{E946}', "About"),
];
pub const TAB_COUNT: u8 = TABS.len() as u8;
/// Index of the About tab (it shows live memory use).
pub const TAB_ABOUT: u8 = 7;
/// Preset outline colors for the island; "Custom" picks anything else.
pub const OUTLINE_COLORS: [u32; 8] = [0xFFFFFF, 0xFF6A88, 0xFF9F0A, 0xFFD60A, 0x30D158, 0x64D2FF, 0x0A84FF, 0xBF5AF2];
pub const PHOTO_EVERY: [(u32, &str); 3] = [(15, "15 min"), (60, "1 hour"), (1440, "1 day")];

const WIDGET_INFO: [(char, &str); 11] = [
    ('\u{E823}', "Time and date"),
    ('\u{E9CA}', "Current conditions and today's range"),
    ('\u{E8D6}', "Music or video from any app or browser"),
    ('\u{E9D9}', "CPU, GPU, memory and disk usage"),
    ('\u{E787}', "This month at a glance"),
    ('\u{E83F}', "Charge level and power state"),
    ('\u{E8FD}', "Your to-do list"),
    ('\u{E968}', "Live download and upload speed"),
    ('\u{E916}', "Classic clock face"),
    ('\u{EB9F}', "Pictures from a folder, a new one every so often"),
    ('\u{E8BD}', "Incoming messages and notifications"),
];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Act {
    Tab(u8),
    Autostart,
    Locked,
    EditReminders,
    ResetLayout,
    OpenFolder,
    Theme(u8),
    Accent(u32),
    Glass,
    Scale(u8),
    Widget(usize),
    SaveCity,
    Units(bool),
    H24,
    Seconds,
    Monday(bool),
    RefreshWeather,
    OpenApps,
    Visualizer,
    MediaStyle(u8),
    GpuOn,
    GpuNext,
    PhotoDir,
    PhotoEvery(u32),
    MsgPreview,
    IslandTop,
    TaskbarVis,
    IslandOutline,
    OutlineColor(u32),
    OutlineCustom,
    PetOn,
    PetMode(u8),
    PetSpeed(u8),
    PetSize(u8),
}

struct Zone {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    act: Act,
}

/// Snapshot of everything the panel displays.
pub struct View<'a> {
    pub cfg: &'a Config,
    pub autostart: bool,
    pub widgets_on: [bool; 11],
    /// Graphics card shown in the System widget, and how many there are.
    pub gpu_name: String,
    pub gpu_count: usize,
    pub photo_dir: String,
    pub weather: String,
    pub mem_mb: f32,
    pub dir: String,
    pub exe_dir: String,
    pub icon: HICON,
}

pub struct Settings {
    pub hwnd: HWND,
    pub edit: HWND,
    pub surf: Option<Surface>,
    pub s: f32,
    pub tab: u8,
    pub hover: Option<Act>,
    zones: Vec<Zone>,
    pub dragging: bool,
    pub glass_preview: f32,
    pub font: HFONT,
    pub brush: HBRUSH,
    pub tracking: bool,
    edit_rect: (i32, i32, i32, i32),
    slider: (f32, f32), // x, width of the glass slider track
}

impl Settings {
    pub fn new(hwnd: HWND, edit: HWND, s: f32, font: HFONT, brush: HBRUSH, glass: f32) -> Self {
        Settings {
            hwnd,
            edit,
            surf: None,
            s,
            tab: 0,
            hover: None,
            zones: Vec::new(),
            dragging: false,
            glass_preview: glass,
            font,
            brush,
            tracking: false,
            edit_rect: (0, 0, 0, 0),
            slider: (0.0, 1.0),
        }
    }

    pub fn hit(&self, x: f32, y: f32) -> Option<Act> {
        self.zones.iter().rev().find(|z| x >= z.x && x <= z.x + z.w && y >= z.y && y <= z.y + z.h).map(|z| z.act)
    }

    /// Glass value under a logical x while dragging the slider.
    pub fn glass_at(&self, x: f32) -> f32 {
        (((x - self.slider.0) / self.slider.1).clamp(0.0, 1.0) * 2.0 * 20.0).round() / 20.0
    }

    fn zone(&mut self, x: f32, y: f32, w: f32, h: f32, act: Act) {
        self.zones.push(Zone { x, y, w, h, act });
    }

    fn hovered(&self, a: Act) -> bool {
        self.hover == Some(a)
    }

    // ------------------------------------------------------------ controls

    fn toggle(&mut self, g: &mut Gfx, th: &Theme, x: f32, cy: f32, on: bool, act: Act) {
        let (w, h) = (44.0, 22.0);
        let y = cy - h / 2.0;
        let hov = self.hovered(act);
        if on {
            g.fill_rrect(x, y, w, h, h / 2.0, if hov { th.accent.alpha(0.85) } else { th.accent });
            g.fill_circle(x + w - h / 2.0, cy, 7.0, WHITE);
        } else {
            g.fill_rrect(x, y, w, h, h / 2.0, th.fg.alpha(if hov { 0.22 } else { 0.14 }));
            g.fill_circle(x + h / 2.0, cy, 7.0, th.fg.alpha(0.7));
        }
        self.zone(x - 6.0, y - 6.0, w + 12.0, h + 12.0, act);
    }

    fn button(&mut self, g: &mut Gfx, th: &Theme, x: f32, cy: f32, w: f32, label: &str, act: Act) {
        let h = 32.0;
        let hov = self.hovered(act);
        g.fill_rrect(x, cy - h / 2.0, w, h, 8.0, th.fg.alpha(if hov { 0.14 } else { 0.08 }));
        g.text(label, x, cy - h / 2.0, w, h, 13.5, 600, Align::Center, Fam::Text, th.fg, false);
        self.zone(x, cy - h / 2.0, w, h, act);
    }

    fn segmented(&mut self, g: &mut Gfx, th: &Theme, right: f32, cy: f32, items: &[(&str, Act)], selected: usize) {
        let seg = 76.0;
        let h = 32.0;
        let w = seg * items.len() as f32;
        let x0 = right - w;
        g.fill_rrect(x0, cy - h / 2.0, w, h, 9.0, th.fg.alpha(0.08));
        for (i, (label, act)) in items.iter().enumerate() {
            let x = x0 + i as f32 * seg;
            if i == selected {
                g.fill_rrect(x + 3.0, cy - h / 2.0 + 3.0, seg - 6.0, h - 6.0, 7.0, th.accent);
                g.text(label, x, cy - h / 2.0, seg, h, 13.0, 600, Align::Center, Fam::Text, WHITE, false);
            } else {
                if self.hovered(*act) {
                    g.fill_rrect(x + 3.0, cy - h / 2.0 + 3.0, seg - 6.0, h - 6.0, 7.0, th.fg.alpha(0.08));
                }
                g.text(label, x, cy - h / 2.0, seg, h, 13.0, 500, Align::Center, Fam::Text, th.fg, false);
            }
            self.zone(x, cy - h / 2.0, seg, h, *act);
        }
    }

    /// A settings row: title + description on the left, control on the right.
    fn row_text(g: &mut Gfx, th: &Theme, x: f32, y: f32, w: f32, title: &str, desc: &str) {
        if desc.is_empty() {
            g.text(title, x, y, w, 60.0, 14.5, 500, Align::Left, Fam::Text, th.fg, true);
        } else {
            g.text(title, x, y + 9.0, w, 22.0, 14.5, 500, Align::Left, Fam::Text, th.fg, true);
            g.text(desc, x, y + 30.0, w, 20.0, 12.5, 400, Align::Left, Fam::Text, th.fg2, true);
        }
    }

    /// Card with `n` rows of 60px; returns the y of the first row.
    fn card(g: &mut Gfx, th: &Theme, x: f32, y: f32, w: f32, n: usize) -> f32 {
        let h = n as f32 * 60.0;
        g.fill_rrect(x, y, w, h, 10.0, th.input);
        for i in 1..n {
            let ly = y + i as f32 * 60.0;
            g.line(x + 16.0, ly, x + w - 16.0, ly, 1.0, th.line);
        }
        y
    }

    // ------------------------------------------------------------ paint

    pub fn paint(&mut self, g: &mut Gfx, screen: HDC, hdc: HDC, v: &View, th: &Theme) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        let (cw, ch) = (r.right.max(1), r.bottom.max(1));
        if self.surf.as_ref().map(|s| (s.w, s.h)) != Some((cw, ch)) {
            self.surf = Surface::new(screen, cw, ch);
        }
        let Some(dc) = self.surf.as_ref().map(|s| s.dc) else { return };
        let (w, h) = (cw as f32 / self.s, ch as f32 / self.s);
        self.zones.clear();
        let mut icon_at: Option<(f32, f32, f32)> = None;
        let mut edit_at: Option<(f32, f32, f32, f32)> = None;
        unsafe {
            if g.rt.BindDC(dc, &RECT { left: 0, top: 0, right: cw, bottom: ch }).is_err() {
                return;
            }
            g.rt.BeginDraw();
            g.rt.SetTransform(&Matrix3x2 { M11: self.s, M12: 0.0, M21: 0.0, M22: self.s, M31: 0.0, M32: 0.0 });
            g.fill_rrect(-1.0, -1.0, w + 2.0, h + 2.0, 0.0, th.bg);

            // Sidebar
            g.text("Frost Widgets", 24.0, 20.0, SIDE - 30.0, 30.0, 18.0, 700, Align::Left, Fam::Display, th.fg, false);
            g.text("Settings", 24.0, 44.0, SIDE - 30.0, 20.0, 12.5, 400, Align::Left, Fam::Text, th.fg2, false);
            for (i, (glyph, name)) in TABS.iter().enumerate() {
                let y = TOP + 8.0 + i as f32 * 42.0;
                let act = Act::Tab(i as u8);
                let sel = self.tab == i as u8;
                if sel {
                    g.fill_rrect(12.0, y, SIDE - 24.0, 36.0, 8.0, th.fg.alpha(0.08));
                    g.fill_rrect(12.0, y + 10.0, 3.0, 16.0, 1.5, th.accent);
                } else if self.hovered(act) {
                    g.fill_rrect(12.0, y, SIDE - 24.0, 36.0, 8.0, th.fg.alpha(0.05));
                }
                g.glyph(*glyph, 36.0, y + 18.0, 14.0, if sel { th.accent } else { th.fg2 });
                g.text(name, 56.0, y, SIDE - 70.0, 36.0, 14.0, if sel { 600 } else { 400 }, Align::Left, Fam::Text, th.fg, false);
                self.zone(12.0, y, SIDE - 24.0, 36.0, act);
            }

            // Content
            let x0 = SIDE + 8.0;
            let cwid = w - x0 - 28.0;
            let right = x0 + cwid - 16.0;
            let title = TABS[self.tab as usize].1;
            g.text(title, x0, 22.0, cwid, 40.0, 26.0, 700, Align::Left, Fam::Display, th.fg, false);
            let tw = cwid - 260.0;
            let tw_toggle = cwid - 110.0;
            match self.tab {
                0 => {
                    let y = Self::card(g, th, x0, TOP, cwid, 2);
                    Self::row_text(g, th, x0 + 18.0, y, tw_toggle, "Start with Windows", "Open Frost Widgets automatically when you sign in");
                    self.toggle(g, th, right - 44.0, y + 30.0, v.autostart, Act::Autostart);
                    Self::row_text(g, th, x0 + 18.0, y + 60.0, tw_toggle, "Lock widget positions", "Stop widgets from being dragged by accident");
                    self.toggle(g, th, right - 44.0, y + 90.0, v.cfg.locked, Act::Locked);

                    let y = Self::card(g, th, x0, TOP + 136.0, cwid, 3);
                    Self::row_text(g, th, x0 + 18.0, y, tw, "Reminders", "Add, edit, reorder and tick off reminders");
                    self.button(g, th, right - 96.0, y + 30.0, 96.0, "Edit", Act::EditReminders);
                    Self::row_text(g, th, x0 + 18.0, y + 60.0, tw, "Layout", "Move every widget back to the default grid");
                    self.button(g, th, right - 96.0, y + 90.0, 96.0, "Reset", Act::ResetLayout);
                    Self::row_text(g, th, x0 + 18.0, y + 120.0, tw, "Settings folder", &v.dir);
                    self.button(g, th, right - 96.0, y + 150.0, 96.0, "Open", Act::OpenFolder);
                }
                1 => {
                    let y = Self::card(g, th, x0, TOP, cwid, 5);
                    Self::row_text(g, th, x0 + 18.0, y, tw - 20.0, "Theme", "Glass tint follows Windows or stays fixed");
                    let t = v.cfg.theme.to_ascii_lowercase();
                    let sel = if t == "light" { 1 } else if t == "dark" { 2 } else { 0 };
                    self.segmented(g, th, right, y + 30.0, &[("Auto", Act::Theme(0)), ("Light", Act::Theme(1)), ("Dark", Act::Theme(2))], sel);

                    Self::row_text(g, th, x0 + 18.0, y + 60.0, tw - 40.0, "Accent color", "Highlights, calendar and clock hand");
                    let cur = Color::parse(&v.cfg.accent).unwrap_or(Color::hex(ACCENTS[0]));
                    for (i, hex) in ACCENTS.iter().enumerate() {
                        let cx = right - 12.0 - (ACCENTS.len() - 1 - i) as f32 * 30.0;
                        let c = Color::hex(*hex);
                        let sel = (c.r - cur.r).abs() + (c.g - cur.g).abs() + (c.b - cur.b).abs() < 0.02;
                        if sel {
                            g.stroke_circle(cx, y + 90.0, 13.0, 2.0, th.fg);
                        } else if self.hovered(Act::Accent(*hex)) {
                            g.stroke_circle(cx, y + 90.0, 13.0, 2.0, th.fg.alpha(0.3));
                        }
                        g.fill_circle(cx, y + 90.0, 9.5, c);
                        self.zone(cx - 14.0, y + 76.0, 28.0, 28.0, Act::Accent(*hex));
                    }

                    Self::row_text(g, th, x0 + 18.0, y + 120.0, tw, "Glass strength", "How frosted the widgets look");
                    let (sx, sw) = (right - 220.0, 180.0);
                    let val = if self.dragging { self.glass_preview } else { v.cfg.glass };
                    let f = (val / 2.0).clamp(0.0, 1.0);
                    let cy = y + 150.0;
                    g.fill_rrect(sx, cy - 2.0, sw, 4.0, 2.0, th.fg.alpha(0.15));
                    g.fill_rrect(sx, cy - 2.0, (sw * f).max(4.0), 4.0, 2.0, th.accent);
                    g.fill_circle(sx + sw * f, cy, 10.0, th.fg.alpha(0.15));
                    g.fill_circle(sx + sw * f, cy, 8.0, WHITE);
                    g.fill_circle(sx + sw * f, cy, 3.5, th.accent);
                    g.text(&format!("{:.0}%", val * 50.0), right - 34.0, cy - 12.0, 36.0, 24.0, 13.0, 600, Align::Right, Fam::Text, th.fg2, false);
                    self.slider = (sx, sw);
                    self.zone(sx - 10.0, cy - 14.0, sw + 20.0, 28.0, Act::Glass);

                    Self::row_text(g, th, x0 + 18.0, y + 180.0, cwid - 350.0, "Widget size", "Bigger or smaller widgets");
                    let sel = SCALES
                        .iter()
                        .enumerate()
                        .min_by(|a, b| (a.1 .0 - v.cfg.scale).abs().total_cmp(&(b.1 .0 - v.cfg.scale).abs()))
                        .map(|(i, _)| i)
                        .unwrap_or(1);
                    let items: Vec<(&str, Act)> = SCALES.iter().enumerate().map(|(i, s)| (s.1, Act::Scale(i as u8))).collect();
                    self.segmented(g, th, right, y + 210.0, &items, sel);
                    Self::row_text(g, th, x0 + 18.0, y + 240.0, tw_toggle, "Music visualizer", "Animated bars under the album art while music plays");
                    self.toggle(g, th, right - 44.0, y + 270.0, v.cfg.visualizer, Act::Visualizer);
                }
                2 => {
                    let rh = 44.0;
                    let y0 = TOP;
                    g.fill_rrect(x0, y0, cwid, rh * KINDS.len() as f32, 10.0, th.input);
                    for (i, k) in KINDS.iter().enumerate() {
                        let y = y0 + i as f32 * rh;
                        if i > 0 {
                            g.line(x0 + 16.0, y, x0 + cwid - 16.0, y, 1.0, th.line);
                        }
                        let (glyph, desc) = WIDGET_INFO[i];
                        let media = *k == crate::widgets::Kind::Media;
                        let (desc, dw) = if media { ("Card, island or taskbar", cwid - 380.0) } else { (desc, cwid - 140.0) };
                        g.glyph(glyph, x0 + 32.0, y + rh / 2.0, 15.0, th.accent);
                        g.text(k.name(), x0 + 56.0, y + 3.0, 220.0, 20.0, 14.0, 500, Align::Left, Fam::Text, th.fg, false);
                        g.text(desc, x0 + 56.0, y + 22.0, dw, 18.0, 12.0, 400, Align::Left, Fam::Text, th.fg2, true);
                        if media {
                            let items = [("Card", Act::MediaStyle(0)), ("Island", Act::MediaStyle(1)), ("Taskbar", Act::MediaStyle(2))];
                            self.segmented(g, th, right - 64.0, y + rh / 2.0, &items, v.cfg.media_style_index() as usize);
                        }
                        self.toggle(g, th, right - 44.0, y + rh / 2.0, v.widgets_on[i], Act::Widget(i));
                    }
                }
                3 => {
                    let y = Self::card(g, th, x0, TOP, cwid, 6);
                    Self::row_text(g, th, x0 + 18.0, y, 150.0, "City", "Weather and clock label");
                    let (ex, ew) = (right - 300.0, 220.0);
                    g.fill_rrect(ex, y + 12.0, ew, 36.0, 8.0, th.bg);
                    edit_at = Some((ex + 10.0, y + 19.0, ew - 20.0, 22.0));
                    let hov = self.hovered(Act::SaveCity);
                    g.fill_rrect(right - 72.0, y + 14.0, 72.0, 32.0, 8.0, if hov { th.accent.alpha(0.85) } else { th.accent });
                    g.text("Save", right - 72.0, y + 14.0, 72.0, 32.0, 13.5, 600, Align::Center, Fam::Text, WHITE, false);
                    self.zone(right - 72.0, y + 14.0, 72.0, 32.0, Act::SaveCity);

                    Self::row_text(g, th, x0 + 18.0, y + 60.0, tw, "Temperature", "");
                    let f = v.cfg.units.eq_ignore_ascii_case("imperial");
                    self.segmented(g, th, right, y + 90.0, &[("\u{B0}C", Act::Units(false)), ("\u{B0}F", Act::Units(true))], f as usize);
                    Self::row_text(g, th, x0 + 18.0, y + 120.0, tw, "24-hour clock", "");
                    self.toggle(g, th, right - 44.0, y + 150.0, v.cfg.clock_24h, Act::H24);
                    Self::row_text(g, th, x0 + 18.0, y + 180.0, tw_toggle, "Seconds hand", "Analog clock redraws every second when on");
                    self.toggle(g, th, right - 44.0, y + 210.0, v.cfg.analog_seconds, Act::Seconds);
                    Self::row_text(g, th, x0 + 18.0, y + 240.0, tw, "Week starts on", "");
                    let mon = v.cfg.week_start.eq_ignore_ascii_case("monday");
                    self.segmented(g, th, right, y + 270.0, &[("Sunday", Act::Monday(false)), ("Monday", Act::Monday(true))], mon as usize);
                    Self::row_text(g, th, x0 + 18.0, y + 300.0, tw, "Weather", &v.weather);
                    self.button(g, th, right - 96.0, y + 330.0, 96.0, "Refresh", Act::RefreshWeather);
                }
                4 => {
                    let y = Self::card(g, th, x0, TOP, cwid, 2);
                    Self::row_text(g, th, x0 + 18.0, y, tw_toggle, "GPU usage", "Adds a GPU ring to the System widget");
                    self.toggle(g, th, right - 44.0, y + 30.0, v.cfg.gpu_on, Act::GpuOn);
                    let desc = if v.gpu_name.is_empty() { "No graphics card found".to_string() } else { v.gpu_name.clone() };
                    Self::row_text(g, th, x0 + 18.0, y + 60.0, tw, "Graphics card", &desc);
                    if v.gpu_count > 1 {
                        self.button(g, th, right - 96.0, y + 90.0, 96.0, "Switch", Act::GpuNext);
                    }

                    let y = Self::card(g, th, x0, TOP + 136.0, cwid, 3);
                    Self::row_text(g, th, x0 + 18.0, y, tw, "Photos folder", &v.photo_dir);
                    self.button(g, th, right - 96.0, y + 30.0, 96.0, "Change", Act::PhotoDir);
                    Self::row_text(g, th, x0 + 18.0, y + 60.0, tw - 40.0, "New photo every", "");
                    let sel = PHOTO_EVERY.iter().position(|p| p.0 == v.cfg.photo_every).unwrap_or(1);
                    let items: Vec<(&str, Act)> = PHOTO_EVERY.iter().map(|p| (p.1, Act::PhotoEvery(p.0))).collect();
                    self.segmented(g, th, right, y + 90.0, &items, sel);
                    Self::row_text(g, th, x0 + 18.0, y + 120.0, tw_toggle, "Message previews", "Show message text in the Messages widget");
                    self.toggle(g, th, right - 44.0, y + 150.0, v.cfg.msg_preview, Act::MsgPreview);
                }
                5 => {
                    let y = Self::card(g, th, x0, TOP, cwid, 3);
                    Self::row_text(g, th, x0 + 18.0, y, tw - 20.0, "Now playing style", "Card, island (notch) or taskbar pill");
                    let items = [("Card", Act::MediaStyle(0)), ("Island", Act::MediaStyle(1)), ("Taskbar", Act::MediaStyle(2))];
                    self.segmented(g, th, right, y + 30.0, &items, v.cfg.media_style_index() as usize);
                    let desc = if v.cfg.island() { "Keep the island above open apps; it hides when an app goes full screen" } else { "For the Island style" };
                    Self::row_text(g, th, x0 + 18.0, y + 60.0, tw_toggle, "Island over windows", desc);
                    self.toggle(g, th, right - 44.0, y + 90.0, v.cfg.island_top, Act::IslandTop);
                    let desc = if v.cfg.taskbar() { "Animated bars beside the song on the taskbar" } else { "For the Taskbar style" };
                    Self::row_text(g, th, x0 + 18.0, y + 120.0, tw_toggle, "Taskbar visualizer", desc);
                    self.toggle(g, th, right - 44.0, y + 150.0, v.cfg.taskbar_vis, Act::TaskbarVis);

                    let y = Self::card(g, th, x0, TOP + 196.0, cwid, 2);
                    Self::row_text(g, th, x0 + 18.0, y, tw_toggle, "Outline", "A colored ring so the island stands out on black screens");
                    self.toggle(g, th, right - 44.0, y + 30.0, v.cfg.island_outline, Act::IslandOutline);
                    // Color row: dimmed (but still usable) while the outline is off.
                    let fade = if v.cfg.island_outline { 1.0 } else { 0.4 };
                    let swatch_w = OUTLINE_COLORS.len() as f32 * 30.0;
                    let cust_w = 84.0;
                    Self::row_text(g, th, x0 + 18.0, y + 60.0, cwid - swatch_w - cust_w - 60.0, "Outline color", "Pick one, or any color");
                    let cur = Color::parse(&v.cfg.island_outline_color).unwrap_or(WHITE);
                    let same = |c: Color| (c.r - cur.r).abs() + (c.g - cur.g).abs() + (c.b - cur.b).abs() < 0.02;
                    let mut preset = false;
                    let cy = y + 90.0;
                    for (i, hex) in OUTLINE_COLORS.iter().enumerate() {
                        let cx = right - cust_w - 26.0 - (OUTLINE_COLORS.len() - 1 - i) as f32 * 30.0;
                        let c = Color::hex(*hex);
                        let act = Act::OutlineColor(*hex);
                        if same(c) {
                            preset = true;
                            g.stroke_circle(cx, cy, 13.0, 2.0, th.fg.alpha(fade));
                        } else if self.hovered(act) {
                            g.stroke_circle(cx, cy, 13.0, 2.0, th.fg.alpha(0.3));
                        }
                        g.fill_circle(cx, cy, 9.5, c.alpha(fade));
                        // White needs an edge to be seen on a light panel.
                        g.stroke_circle(cx, cy, 9.5, 1.0, th.fg.alpha(0.18 * fade));
                        self.zone(cx - 14.0, cy - 14.0, 28.0, 28.0, act);
                    }
                    // Custom: shows the picked color, ringed when it isn't a preset.
                    let bx = right - cust_w;
                    let hov = self.hovered(Act::OutlineCustom);
                    let bh = 32.0;
                    g.fill_rrect(bx, cy - bh / 2.0, cust_w, bh, 8.0, th.fg.alpha(if hov { 0.14 } else { 0.08 }));
                    if !preset {
                        g.stroke_rrect(bx + 1.0, cy - bh / 2.0 + 1.0, cust_w - 2.0, bh - 2.0, 7.0, 2.0, th.fg.alpha(fade));
                    }
                    g.fill_circle(bx + 16.0, cy, 6.5, cur.alpha(fade));
                    g.stroke_circle(bx + 16.0, cy, 6.5, 1.0, th.fg.alpha(0.25));
                    g.text("Custom", bx + 26.0, cy - bh / 2.0, cust_w - 30.0, bh, 13.0, 600, Align::Center, Fam::Text, th.fg, false);
                    self.zone(bx, cy - bh / 2.0, cust_w, bh, Act::OutlineCustom);
                }
                6 => {
                    use crate::desk_pet::{option_index, Mode, MODES, SIZES, SPEEDS};
                    let y = Self::card(g, th, x0, TOP, cwid, 3);
                    Self::row_text(g, th, x0 + 18.0, y, tw_toggle, "Desktop pet", "A cozy critter that lives on top of your taskbar");
                    self.toggle(g, th, right - 44.0, y + 30.0, v.cfg.pet_on, Act::PetOn);
                    Self::row_text(g, th, x0 + 18.0, y + 60.0, tw - 20.0, "Walking speed", "");
                    let items: Vec<(&str, Act)> = SPEEDS.iter().enumerate().map(|(i, o)| (o.1, Act::PetSpeed(i as u8))).collect();
                    self.segmented(g, th, right, y + 90.0, &items, option_index(&SPEEDS, &v.cfg.pet_speed));
                    Self::row_text(g, th, x0 + 18.0, y + 120.0, tw - 20.0, "Size", "");
                    let items: Vec<(&str, Act)> = SIZES.iter().enumerate().map(|(i, o)| (o.1, Act::PetSize(i as u8))).collect();
                    self.segmented(g, th, right, y + 150.0, &items, option_index(&SIZES, &v.cfg.pet_size));

                    // Behavior: pick one row.
                    let y = Self::card(g, th, x0, TOP + 196.0, cwid, MODES.len());
                    let cur = Mode::from_key(&v.cfg.pet_mode).index();
                    for (i, m) in MODES.iter().enumerate() {
                        let ry = y + i as f32 * 60.0;
                        let act = Act::PetMode(i as u8);
                        if self.hovered(act) && i != cur {
                            g.fill_rrect(x0 + 6.0, ry + 6.0, cwid - 12.0, 48.0, 8.0, th.fg.alpha(0.05));
                        }
                        Self::row_text(g, th, x0 + 18.0, ry, tw_toggle, m.2, m.3);
                        if i == cur {
                            g.glyph('\u{E73E}', right - 12.0, ry + 30.0, 15.0, th.accent);
                        }
                        self.zone(x0, ry, cwid, 60.0, act);
                    }
                }
                _ => {
                    icon_at = Some((x0, TOP, 64.0));
                    g.text("Frost Widgets", x0 + 80.0, TOP + 4.0, cwid - 80.0, 30.0, 20.0, 700, Align::Left, Fam::Display, th.fg, false);
                    let ver = format!("Version {}", env!("CARGO_PKG_VERSION"));
                    g.text(&ver, x0 + 80.0, TOP + 32.0, cwid - 80.0, 20.0, 13.0, 400, Align::Left, Fam::Text, th.fg2, false);
                    let blurb = "Glass desktop widgets built to use almost no memory or CPU.";
                    g.text(blurb, x0, TOP + 80.0, cwid, 22.0, 13.5, 400, Align::Left, Fam::Text, th.fg, true);
                    let y = Self::card(g, th, x0, TOP + 120.0, cwid, 3);
                    Self::row_text(g, th, x0 + 18.0, y, tw, "Memory in use", "Private working set, as Task Manager shows it");
                    let mem = format!("{:.1} MB", v.mem_mb);
                    g.text(&mem, right - 160.0, y, 160.0, 60.0, 15.0, 600, Align::Right, Fam::Text, th.accent, false);
                    Self::row_text(g, th, x0 + 18.0, y + 60.0, cwid - 40.0, "Installed in", &v.exe_dir);
                    Self::row_text(g, th, x0 + 18.0, y + 120.0, tw, "Uninstall", "Remove it from Settings \u{203A} Apps \u{203A} Installed apps");
                    self.button(g, th, right - 120.0, y + 150.0, 120.0, "Open Apps", Act::OpenApps);
                }
            }
            let _ = g.rt.EndDraw(None, None);
            if let Some((x, y, size)) = icon_at {
                let s = self.s;
                let _ = DrawIconEx(dc, (x * s) as i32, (y * s) as i32, v.icon, (size * s) as i32, (size * s) as i32, 0, None, DI_NORMAL);
            }
            let _ = BitBlt(hdc, 0, 0, cw, ch, dc, 0, 0, SRCCOPY);
        }
        // Native text box only lives on the Clock & weather tab.
        let s = self.s;
        let er = edit_at.map(|(x, y, w, h)| ((x * s).round() as i32, (y * s).round() as i32, (w * s).round() as i32, (h * s).round() as i32));
        unsafe {
            match er {
                Some(rc) => {
                    if rc != self.edit_rect {
                        let _ = MoveWindow(self.edit, rc.0, rc.1, rc.2, rc.3, true);
                        self.edit_rect = rc;
                    }
                    let _ = ShowWindow(self.edit, SW_SHOWNA);
                }
                None => {
                    let _ = ShowWindow(self.edit, SW_HIDE);
                }
            }
        }
    }
}
