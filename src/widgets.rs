//! Widget layouts. All coordinates are logical (1x) pixels inside the glass card;
//! the renderer applies DPI scaling.
use std::f32::consts::PI;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

/// Now playing is drawn as a notch-style island instead of a card.
pub static ISLAND: AtomicBool = AtomicBool::new(false);
/// Now playing sits on the Windows taskbar as a slim strip.
pub static TASKBAR: AtomicBool = AtomicBool::new(false);

use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Graphics::Direct2D::ID2D1Bitmap;

use crate::gfx::{Align, Color, Fam, Gfx, WHITE};
use crate::media::MediaState;
use crate::sys::{condition, Battery, Weather};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Clock,
    Weather,
    Media,
    System,
    Calendar,
    Battery,
    Notes,
    Network,
    Analog,
    Photos,
    Messages,
}

pub const KINDS: [Kind; 11] = [
    Kind::Clock,
    Kind::Weather,
    Kind::Media,
    Kind::System,
    Kind::Calendar,
    Kind::Battery,
    Kind::Notes,
    Kind::Network,
    Kind::Analog,
    Kind::Photos,
    Kind::Messages,
];

pub const SMALL: f32 = 170.0;
pub const GAP: f32 = 16.0;
pub const WIDE: f32 = SMALL * 2.0 + GAP;

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Clock => "clock",
            Kind::Weather => "weather",
            Kind::Media => "now_playing",
            Kind::System => "system",
            Kind::Calendar => "calendar",
            Kind::Battery => "battery",
            Kind::Notes => "reminders",
            Kind::Network => "network",
            Kind::Analog => "analog_clock",
            Kind::Photos => "photos",
            Kind::Messages => "messages",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Kind::Clock => "Clock",
            Kind::Weather => "Weather",
            Kind::Media => "Now playing",
            Kind::System => "System",
            Kind::Calendar => "Calendar",
            Kind::Battery => "Battery",
            Kind::Notes => "Reminders",
            Kind::Network => "Network",
            Kind::Analog => "Analog clock",
            Kind::Photos => "Photos",
            Kind::Messages => "Messages",
        }
    }
    pub fn size(self) -> (f32, f32) {
        match self {
            Kind::Media if ISLAND.load(Ordering::Relaxed) => (ISL_CW, ISL_CH),
            Kind::Media if TASKBAR.load(Ordering::Relaxed) => (TB_W, TB_H),
            Kind::Media | Kind::System | Kind::Notes | Kind::Photos | Kind::Messages => (WIDE, SMALL),
            _ => (SMALL, SMALL),
        }
    }
    /// Default grid slot (column, row) in a 4-column layout.
    pub fn slot(self) -> (i32, i32) {
        match self {
            Kind::Clock => (0, 0),
            Kind::Weather => (1, 0),
            Kind::Media => (2, 0),
            Kind::System => (0, 1),
            Kind::Calendar => (2, 1),
            Kind::Battery => (3, 1),
            Kind::Notes => (0, 2),
            Kind::Network => (2, 2),
            Kind::Analog => (3, 2),
            Kind::Photos => (0, 3),
            Kind::Messages => (2, 3),
        }
    }
}

/// Everything the widgets display.
pub struct Hub {
    pub now: SYSTEMTIME,
    pub now_ft: i64,
    pub cpu: f32,
    pub ram: f32,
    pub disk: f32,
    pub down: f64,
    pub up: f64,
    pub bat: Battery,
    pub weather: Option<Weather>,
    pub weather_failed: bool,
    /// Why weather is missing ("Offline", "City not found").
    pub weather_msg: &'static str,
    pub imperial: bool,
    /// Visualizer enabled in settings, and current band levels (0..1).
    pub vis_on: bool,
    pub vis: [f32; crate::vis::BANDS],
    /// Seek preview (0..1) while dragging the progress bar, and hover state.
    pub seek: Option<f32>,
    pub bar_hover: bool,
    /// Island style, and how far it is expanded (0 = compact pill, 1 = full; may overshoot).
    pub island: bool,
    pub isl_t: f32,
    /// Compact island width for the current title.
    pub isl_cw: f32,
    /// Taskbar style, its visualizer toggle, and what the pointer is over
    /// (0 = outside, 1..=3 = prev/play/next, 9 = elsewhere on the strip).
    pub taskbar: bool,
    pub tb_vis: bool,
    pub tb_hot: u32,
    /// Taskbar strip anchored to the right (beside the tray) instead of the left.
    pub tb_right: bool,
    /// Outline color (0xRRGGBB) around the island and taskbar pill, None when off.
    pub outline: Option<u32>,
    pub media: MediaState,
    /// GPU usage in percent, None when the GPU ring is off.
    pub gpu: Option<f32>,
    /// Photos: bumps on every new photo; message when there's none to show.
    pub photo_id: u64,
    pub photo_msg: &'static str,
    /// Messages: newest first, plus why they can't be read ("" when fine).
    pub msgs: Vec<crate::msgs::Msg>,
    pub msg_status: &'static str,
    pub msg_preview: bool,
    pub notes: Vec<(bool, String)>,
    pub city: String,
    pub h24: bool,
    pub seconds: bool,
    pub monday: bool,
}

impl Hub {
    /// Are visualizer bars part of the now-playing look right now?
    pub fn bars_on(&self) -> bool {
        if self.taskbar {
            self.tb_vis
        } else {
            self.vis_on && !self.island
        }
    }
}

pub struct Pal {
    pub fg: Color,
    pub fg2: Color,
    pub track: Color,
    pub accent: Color,
    pub dark: bool, // dark text on light glass
}

impl Pal {
    pub fn new(dark_text: bool, accent: Color) -> Self {
        let fg = if dark_text { Color::hex(0x1D1D1F) } else { WHITE };
        Pal {
            fg,
            fg2: fg.alpha(if dark_text { 0.62 } else { 0.72 }),
            track: fg.alpha(if dark_text { 0.12 } else { 0.22 }),
            accent,
            dark: dark_text,
        }
    }
    fn ring(&self, i: usize) -> Color {
        let light = [0x7CF5D4, 0xA99BFF, 0xFFC27A, 0x8CF58C, 0xFF7B7B];
        let deep = [0x0FA88A, 0x6C5CE7, 0xE08A1E, 0x2FB344, 0xE5484D];
        Color::hex(if self.dark { deep[i] } else { light[i] })
    }
}

const G_PLAY: char = '\u{E768}';
const G_PAUSE: char = '\u{E769}';
const G_PREV: char = '\u{E892}';
const G_NEXT: char = '\u{E893}';
const G_UP: char = '\u{E74A}';
const G_DOWN: char = '\u{E74B}';
const G_BOLT: char = '\u{E945}';
const G_MUSIC: char = '\u{EC4F}';
const G_CIRCLE: char = '\u{ECCA}';
const G_DONE: char = '\u{EC61}';
const G_ADD: char = '\u{E710}';
const G_PHOTO: char = '\u{EB9F}';
const G_CHAT: char = '\u{E8BD}';

/// Hit id for the + button on the reminders widget.
pub const HIT_ADD: u32 = 50;
/// Hit id for the "next photo" button.
pub const HIT_PHOTO_NEXT: u32 = 60;

const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];
const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December",
];

// ------------------------------------------------------------------ change detection

/// Hash of exactly what a widget shows; the widget is only redrawn when it changes.
pub fn signature(k: Kind, h: &Hub) -> u64 {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    let n = &h.now;
    match k {
        Kind::Clock => (n.wHour, n.wMinute, n.wDay, h.h24, &h.city).hash(&mut s),
        Kind::Analog => (n.wHour, n.wMinute, if h.seconds { n.wSecond } else { 0 }).hash(&mut s),
        Kind::Calendar => (n.wYear, n.wMonth, n.wDay, h.monday).hash(&mut s),
        Kind::Weather => {
            if let Some(w) = &h.weather {
                (&w.place, w.temp.round() as i32, w.hi.round() as i32, w.lo.round() as i32, w.code, w.day).hash(&mut s);
            }
            (h.weather_failed, h.weather_msg).hash(&mut s);
        }
        Kind::Media => {
            let m = &h.media;
            (m.has, &m.title, &m.artist, &m.app, m.playing, m.art_id, m.can_seek, h.vis_on, h.bar_hover).hash(&mut s);
            if m.dur > 0.0 {
                (m.position(h.now_ft) as i64, m.dur as i64).hash(&mut s);
            }
            h.seek.map(|f| (f * 1000.0) as i32).hash(&mut s);
            (h.island, (h.isl_t * 1000.0) as i32).hash(&mut s);
            (h.taskbar, h.tb_vis, h.tb_hot, h.tb_right, h.outline).hash(&mut s);
        }
        Kind::System => (h.cpu.round() as i32, h.ram.round() as i32, h.disk.round() as i32, h.gpu.map(|g| g.round() as i32)).hash(&mut s),
        Kind::Photos => (h.photo_id, h.photo_msg).hash(&mut s),
        Kind::Messages => {
            (&h.msgs, h.msg_status, h.msg_preview).hash(&mut s);
            if !h.msgs.is_empty() {
                (n.wDay, n.wHour, n.wMinute).hash(&mut s); // relative times
            }
        }
        Kind::Battery => (h.bat.has, h.bat.pct, h.bat.charging, h.bat.ac).hash(&mut s),
        Kind::Network => (speed(h.down), speed(h.up)).hash(&mut s),
        Kind::Notes => h.notes.hash(&mut s),
    }
    s.finish()
}

// ------------------------------------------------------------------ hit testing

/// Hit id for the now-playing progress bar.
pub const HIT_SEEK: u32 = 4;

/// Now-playing geometry. With the visualizer on, the art shrinks and the bars sit under it.
pub struct MediaLayout {
    pub art: f32,
    pub x0: f32,
    pub w: f32,
    pub bar_y: f32,
    pub btn_y: f32,
    pub btn_x: [f32; 3],
    pub vis: (f32, f32, f32, f32),
}

pub fn media_layout(vis_on: bool) -> MediaLayout {
    let art = if vis_on { 104.0 } else { 138.0 };
    let x0 = 16.0 + art + 18.0;
    let w = WIDE - 16.0 - x0;
    MediaLayout {
        art,
        x0,
        w,
        bar_y: 92.0,
        btn_y: 138.0,
        btn_x: [x0 + w * 0.18, x0 + w * 0.5, x0 + w * 0.82],
        vis: (16.0, 16.0 + art + 10.0, art, 154.0 - (16.0 + art + 10.0)),
    }
}

/// Fraction along the progress bar for a logical x.
pub fn seek_frac(x: f32, h: &Hub) -> f32 {
    let (x0, w) = if h.island {
        let l = island_layout(1.0, h.isl_cw);
        (l.bar_x, l.bar_w)
    } else if h.taskbar {
        let l = tb_layout(h);
        (l.bar_x, l.bar_w)
    } else {
        let l = media_layout(h.vis_on);
        (l.x0, l.w)
    };
    ((x - x0) / w).clamp(0.0, 1.0)
}

// ------------------------------------------------------------------ island

/// Island canvas (logical px). The pill hangs from the top centre and grows down.
pub const ISL_CW: f32 = 436.0;
pub const ISL_CH: f32 = 172.0;
/// Compact pill: height and radius. Its width follows the song title.
const ISL_COMPACT: (f32, f32) = (32.0, 16.0);
const ISL_ART: f32 = 22.0;
const ISL_PAD: f32 = 5.0;
/// Space on each side of the centred title (art + gaps, mirrored on the right).
const ISL_SIDE: f32 = ISL_PAD + ISL_ART + 10.0;
const ISL_TITLE: f32 = 12.5;
const ISL_FULL: (f32, f32, f32) = (400.0, 150.0, 40.0);
const ISL_TOP: f32 = 4.0;

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn island_title(m: &MediaState) -> &str {
    if !m.has {
        "Not playing"
    } else if m.title.is_empty() {
        "Unknown title"
    } else {
        &m.title
    }
}

/// Compact pill width: just wide enough for the title, within limits.
pub fn island_width(g: &mut Gfx, h: &Hub) -> f32 {
    let tw = g.text_width(island_title(&h.media), ISL_TITLE, 600, Fam::Text);
    (tw + 2.0 * ISL_SIDE + 4.0).clamp(150.0, 340.0)
}

/// Pill rect (x, y, w, h, radius) at expansion `t`, for a compact width `cw`.
pub fn island_shape(t: f32, cw: f32) -> (f32, f32, f32, f32, f32) {
    let (chh, cr) = ISL_COMPACT;
    let (fw, fh, fr) = ISL_FULL;
    // Height leads, width follows a little later: the corners visibly reshape.
    let tw = t;
    let th = if t <= 1.0 { (t * 1.15).min(1.0) } else { t };
    let w = lerp(cw, fw, tw).max(cw * 0.8);
    let h = lerp(chh, fh, th).max(chh * 0.8);
    let r = lerp(cr, fr, t.clamp(0.0, 1.0)).min(h / 2.0);
    ((ISL_CW - w) / 2.0, ISL_TOP, w, h, r)
}

pub struct IslandLayout {
    pub art: (f32, f32, f32, f32), // x, y, size, radius
    pub bar_x: f32,
    pub bar_w: f32,
    pub bar_y: f32,
    pub btn_y: f32,
    pub btn_x: [f32; 3],
}

pub fn island_layout(t: f32, cw: f32) -> IslandLayout {
    let (px, py, w, _, _) = island_shape(1.0, cw);
    let (cx, cy, _, _, _) = island_shape(0.0, cw);
    let k = t.clamp(0.0, 1.05);
    let art = (
        lerp(cx + ISL_PAD, px + 18.0, k),
        lerp(cy + ISL_PAD, py + 18.0, k),
        lerp(ISL_ART, 64.0, k),
        lerp(ISL_ART / 2.0, 15.0, k),
    );
    let mid = px + w / 2.0;
    IslandLayout {
        art,
        bar_x: px + 58.0,
        bar_w: w - 116.0,
        bar_y: py + 98.0,
        btn_y: py + 128.0,
        btn_x: [mid - 64.0, mid, mid + 64.0],
    }
}

/// Outline width (logical px) around the island and taskbar pill.
pub const OUTLINE_W: f32 = 1.5;

/// The optional colored ring, drawn just inside the shape's edge so it never
/// grows the area that catches the mouse.
fn outline(g: &mut Gfx, h: &Hub, x: f32, y: f32, w: f32, ht: f32, r: f32) {
    if let Some(c) = h.outline {
        let i = OUTLINE_W / 2.0;
        g.stroke_rrect(x + i, y + i, w - OUTLINE_W, ht - OUTLINE_W, (r - i).max(0.0), OUTLINE_W, Color::hex(c).alpha(0.9));
    }
}

fn island(g: &mut Gfx, h: &Hub, p: &Pal, art: Option<&ID2D1Bitmap>) {
    let m = &h.media;
    let t = h.isl_t;
    let (x, y, w, ht, r) = island_shape(t, h.isl_cw);
    // Soft shadow, then the black pill.
    for (grow, a) in [(10.0, 0.035), (6.0, 0.06), (3.0, 0.09)] {
        g.fill_rrect(x - grow, y - grow * 0.4, w + grow * 2.0, ht + grow * 1.6, r + grow, Color::hex(0x000000).alpha(a));
    }
    g.fill_rrect(x, y, w, ht, r, Color::hex(0x000000).alpha(0.96));
    let rim = Color::hex(0xFFFFFF).alpha(0.07);
    g.fill_rrect(x + r * 0.6, y + ht - 1.0, (w - r * 1.2).max(0.0), 1.0, 0.5, rim);
    outline(g, h, x, y, w, ht, r);
    unsafe {
        g.rt.PushAxisAlignedClip(
            &windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + ht },
            windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        );
    }
    let l = island_layout(t, h.isl_cw);
    let (ax, ay, asz, ar) = l.art;
    match (m.has, art, &m.art) {
        (true, Some(b), Some(a)) => g.bitmap_rounded(b, a.w as f32, a.h as f32, ax, ay, asz, asz, ar),
        _ => {
            g.fill_rrect(ax, ay, asz, asz, ar, WHITE.alpha(0.12));
            g.glyph(G_MUSIC, ax + asz / 2.0, ay + asz / 2.0, asz * 0.4, WHITE.alpha(0.7));
        }
    }
    let title = island_title(m);
    // Compact content fades out quickly; the full view fades in once the shape has room.
    let ca = (1.0 - t * 3.0).clamp(0.0, 1.0);
    if ca > 0.0 {
        let (cx, cy, cw, chh, _) = island_shape(0.0, h.isl_cw);
        let col = if m.has { WHITE } else { WHITE.alpha(0.6) };
        g.text(title, cx + ISL_SIDE, cy, cw - 2.0 * ISL_SIDE, chh, ISL_TITLE, 600, Align::Center, Fam::Text, col.alpha(ca * col.a), true);
    }
    let fa = ((t - 0.72) / 0.28).clamp(0.0, 1.0);
    if fa > 0.0 {
        let (px, py, pw, _, _) = island_shape(1.0, h.isl_cw);
        let tx = px + 96.0;
        let tw = pw - 96.0 - 22.0;
        g.text(title, tx, py + 20.0, tw, 22.0, 15.0, 600, Align::Left, Fam::Text, WHITE.alpha(fa), true);
        let sub = if m.has { m.artist.as_str() } else { "Play music in any app or browser" };
        g.text(sub, tx, py + 42.0, tw, 20.0, 13.0, 400, Align::Left, Fam::Text, WHITE.alpha(0.62 * fa), true);
        if m.has && !m.app.is_empty() {
            g.text(&m.app, tx, py + 62.0, tw, 18.0, 11.0, 600, Align::Left, Fam::Text, WHITE.alpha(0.4 * fa), true);
        }
        let fg = WHITE.alpha(fa);
        let dim = WHITE.alpha(0.3 * fa);
        if m.has && m.dur > 0.0 {
            let live = m.position(h.now_ft);
            let frac = h.seek.unwrap_or((live / m.dur) as f32).clamp(0.0, 1.0);
            let pos = frac as f64 * m.dur;
            let active = h.seek.is_some() || h.bar_hover;
            let bh = if active { 6.0 } else { 4.0 };
            let by = l.bar_y + 2.0 - bh / 2.0;
            g.fill_rrect(l.bar_x, by, l.bar_w, bh, bh / 2.0, WHITE.alpha(0.22 * fa));
            g.fill_rrect(l.bar_x, by, (l.bar_w * frac).max(bh), bh, bh / 2.0, fg);
            if active && m.can_seek {
                g.fill_circle(l.bar_x + l.bar_w * frac, l.bar_y + 2.0, 7.0, fg);
            }
            let tc = WHITE.alpha(0.55 * fa);
            g.text(&fmt_time(pos), l.bar_x - 50.0, l.bar_y - 6.0, 42.0, 16.0, 10.5, 600, Align::Right, Fam::Text, tc, false);
            let rem = format!("-{}", fmt_time(m.dur - pos));
            g.text(&rem, l.bar_x + l.bar_w + 8.0, l.bar_y - 6.0, 46.0, 16.0, 10.5, 600, Align::Left, Fam::Text, tc, false);
        }
        let c = if m.has { fg } else { dim };
        g.glyph(G_PREV, l.btn_x[0], l.btn_y, 18.0, c);
        g.glyph(if m.playing { G_PAUSE } else { G_PLAY }, l.btn_x[1], l.btn_y, 24.0, c);
        g.glyph(G_NEXT, l.btn_x[2], l.btn_y, 18.0, c);
    }
    unsafe {
        g.rt.PopAxisAlignedClip();
    }
    let _ = p;
}

const NOTE_Y0: f32 = 40.0;
const NOTE_ROW: f32 = 24.0;
const NOTE_MAX: usize = 5;

pub fn notes_view(notes: &[(bool, String)]) -> Vec<usize> {
    let mut v: Vec<usize> = (0..notes.len()).filter(|&i| !notes[i].0).collect();
    v.extend((0..notes.len()).filter(|&i| notes[i].0));
    v.truncate(NOTE_MAX);
    v
}

/// Clickable element under a point: media buttons 1..=3, reminder rows 100+index.
pub fn hit(k: Kind, x: f32, y: f32, h: &Hub) -> Option<u32> {
    match k {
        // The island only takes clicks once fully open; compact it is just dragged.
        Kind::Media if h.island => {
            if !h.media.has || h.isl_t < 0.95 {
                return None;
            }
            let l = island_layout(1.0, h.isl_cw);
            if h.media.can_seek && h.media.dur > 0.0 && x >= l.bar_x - 6.0 && x <= l.bar_x + l.bar_w + 6.0 && (y - l.bar_y - 2.0).abs() < 10.0 {
                return Some(HIT_SEEK);
            }
            l.btn_x
                .iter()
                .position(|&bx| (x - bx).powi(2) + (y - l.btn_y).powi(2) < 20.0f32.powi(2))
                .map(|i| i as u32 + 1)
        }
        Kind::Media if h.taskbar => taskbar_hit(x, y, h),
        Kind::Media if h.media.has => {
            let l = media_layout(h.vis_on);
            if h.media.can_seek && h.media.dur > 0.0 && x >= l.x0 - 6.0 && x <= l.x0 + l.w + 6.0 && (y - l.bar_y - 2.0).abs() < 10.0 {
                return Some(HIT_SEEK);
            }
            l.btn_x
                .iter()
                .position(|&bx| (x - bx).powi(2) + (y - l.btn_y).powi(2) < 20.0f32.powi(2))
                .map(|i| i as u32 + 1)
        }
        Kind::Photos if h.photo_id > 0 && (x - (WIDE - 26.0)).powi(2) + (y - (SMALL - 26.0)).powi(2) < 16.0f32.powi(2) => Some(HIT_PHOTO_NEXT),
        Kind::Notes if (x - (WIDE - 28.0)).powi(2) + (y - 22.0).powi(2) < 17.0f32.powi(2) => Some(HIT_ADD),
        Kind::Notes if (12.0..WIDE - 12.0).contains(&x) && y >= NOTE_Y0 => {
            let row = ((y - NOTE_Y0) / NOTE_ROW) as usize;
            notes_view(&h.notes).get(row).map(|&i| 100 + i as u32)
        }
        _ => None,
    }
}

// ------------------------------------------------------------------ drawing

pub fn draw(k: Kind, g: &mut Gfx, h: &Hub, p: &Pal, art: Option<&ID2D1Bitmap>) {
    match k {
        Kind::Clock => clock(g, h, p),
        Kind::Weather => weather(g, h, p),
        Kind::Media if h.island => island(g, h, p, art),
        Kind::Media if h.taskbar => taskbar(g, h, p, art),
        Kind::Media => media(g, h, p, art),
        Kind::System => system(g, h, p),
        Kind::Calendar => calendar(g, h, p),
        Kind::Battery => battery(g, h, p),
        Kind::Notes => notes(g, h, p),
        Kind::Network => network(g, h, p),
        Kind::Analog => analog(g, h, p),
        Kind::Photos => photos(g, h, p, art),
        Kind::Messages => messages(g, h, p),
    }
}

fn photos(g: &mut Gfx, h: &Hub, p: &Pal, photo: Option<&ID2D1Bitmap>) {
    match photo.filter(|_| h.photo_id > 0) {
        Some(b) => {
            let sz = unsafe { b.GetSize() };
            g.bitmap_rounded(b, sz.width, sz.height, 0.0, 0.0, WIDE, SMALL, 24.0);
            // Small "next photo" button in the corner.
            let (cx, cy) = (WIDE - 26.0, SMALL - 26.0);
            g.fill_circle(cx, cy, 14.0, Color::hex(0x000000).alpha(0.32));
            g.glyph(G_NEXT, cx + 0.5, cy, 11.0, WHITE.alpha(0.92));
        }
        None => {
            label(g, "Photos", p);
            g.glyph(G_PHOTO, WIDE / 2.0, 72.0, 30.0, p.fg2);
            let msg = if h.photo_msg.is_empty() { "Loading\u{2026}" } else { h.photo_msg };
            g.text(msg, 16.0, 100.0, WIDE - 32.0, 22.0, 14.0, 600, Align::Center, Fam::Text, p.fg, true);
            if !h.photo_msg.is_empty() {
                g.text("Choose a folder in Settings", 16.0, 122.0, WIDE - 32.0, 20.0, 12.5, 400, Align::Center, Fam::Text, p.fg2, true);
            }
        }
    }
}

/// "now", "5m", "3h", "Mon", or a date, for a FILETIME.
fn ago(ft: i64, now_ft: i64) -> String {
    let secs = (now_ft - ft) / 10_000_000;
    match secs {
        i64::MIN..=59 => "now".into(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86_400),
    }
}

fn messages(g: &mut Gfx, h: &Hub, p: &Pal) {
    g.text("Messages", 16.0, 12.0, 200.0, 20.0, 13.0, 700, Align::Left, Fam::Text, p.accent, false);
    if !h.msgs.is_empty() {
        g.text(&h.msgs.len().to_string(), WIDE - 116.0, 12.0, 100.0, 20.0, 13.0, 600, Align::Right, Fam::Text, p.fg2, false);
    }
    if h.msgs.is_empty() {
        let (a, b) = if h.msg_status.is_empty() {
            ("No new messages", "Notifications from your apps show up here")
        } else {
            (h.msg_status, "Allow it in Windows Settings \u{203A} Privacy \u{203A} Notifications")
        };
        g.glyph(G_CHAT, 34.0, 82.0, 22.0, p.fg2);
        g.text(a, 58.0, 62.0, WIDE - 74.0, 22.0, 15.0, 600, Align::Left, Fam::Text, p.fg, true);
        g.text(b, 58.0, 84.0, WIDE - 74.0, 20.0, 12.5, 400, Align::Left, Fam::Text, p.fg2, true);
        return;
    }
    for (row, m) in h.msgs.iter().take(3).enumerate() {
        let y = 38.0 + row as f32 * 42.0;
        // App badge: a coloured circle with the app's first letter.
        let mut hs = std::collections::hash_map::DefaultHasher::new();
        m.app.hash(&mut hs);
        let palette = [0x30D158, 0x0A84FF, 0xFF9F0A, 0xBF5AF2, 0xFF375F, 0x64D2FF, 0xFFD60A];
        let c = Color::hex(palette[(hs.finish() % palette.len() as u64) as usize]);
        g.fill_circle(30.0, y + 18.0, 14.0, c);
        let ini: String = m.app.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
        g.text(&ini, 16.0, y + 4.0, 28.0, 28.0, 13.0, 700, Align::Center, Fam::Text, WHITE, false);
        let when = ago(m.time, h.now_ft);
        let ww = g.text_width(&when, 11.5, 500, Fam::Text);
        g.text(&when, WIDE - 16.0 - ww - 2.0, y + 1.0, ww + 4.0, 18.0, 11.5, 500, Align::Right, Fam::Text, p.fg2, false);
        let tw = WIDE - 54.0 - 24.0 - ww;
        let head = if m.app.is_empty() || m.title.contains(&m.app) { m.title.clone() } else { format!("{} \u{B7} {}", m.title, m.app) };
        g.text(&head, 54.0, y, tw, 19.0, 13.0, 600, Align::Left, Fam::Text, p.fg, true);
        let body = if !h.msg_preview {
            "New message"
        } else if m.body.is_empty() {
            m.app.as_str()
        } else {
            m.body.as_str()
        };
        g.text(body, 54.0, y + 18.0, WIDE - 70.0, 18.0, 12.5, 400, Align::Left, Fam::Text, p.fg2, true);
    }
}

fn label(g: &mut Gfx, s: &str, p: &Pal) {
    g.text(s, 16.0, 12.0, 140.0, 20.0, 13.0, 600, Align::Left, Fam::Text, p.fg2, true);
}

fn clock(g: &mut Gfx, h: &Hub, p: &Pal) {
    let n = &h.now;
    let city = if h.city.is_empty() { "Local time" } else { &h.city };
    g.text(city, 16.0, 12.0, 138.0, 20.0, 13.0, 600, Align::Left, Fam::Text, p.fg2, true);
    let (time, ampm) = if h.h24 {
        (format!("{:02}:{:02}", n.wHour, n.wMinute), "")
    } else {
        let hr = match n.wHour % 12 {
            0 => 12,
            x => x,
        };
        (format!("{}:{:02}", hr, n.wMinute), if n.wHour < 12 { "AM" } else { "PM" })
    };
    g.text(&time, 13.0, 34.0, 150.0, 68.0, 54.0, 300, Align::Left, Fam::Display, p.fg, false);
    if !ampm.is_empty() {
        let w = g.text_width(&time, 54.0, 300, Fam::Display);
        g.text(ampm, 13.0 + w + 4.0, 70.0, 40.0, 24.0, 14.0, 600, Align::Left, Fam::Text, p.fg2, false);
    }
    g.text(DAYS[n.wDayOfWeek as usize % 7], 16.0, 106.0, 140.0, 22.0, 15.0, 600, Align::Left, Fam::Text, p.fg, true);
    let date = format!("{} {}", MONTHS[(n.wMonth as usize + 11) % 12], n.wDay);
    g.text(&date, 16.0, 127.0, 140.0, 20.0, 13.5, 400, Align::Left, Fam::Text, p.fg2, true);
}

fn weather(g: &mut Gfx, h: &Hub, p: &Pal) {
    let Some(w) = &h.weather else {
        label(g, if h.city.is_empty() { "Weather" } else { &h.city }, p);
        g.text("--\u{B0}", 13.0, 30.0, 140.0, 58.0, 46.0, 300, Align::Left, Fam::Display, p.fg, false);
        let (msg, hint) = match (h.weather_failed, h.weather_msg) {
            (false, _) => ("Loading\u{2026}", ""),
            (true, "City not found") => ("City not found", "Check it in Settings"),
            (true, m) => (m, "Retrying shortly"),
        };
        g.text(msg, 16.0, 110.0, 140.0, 20.0, 13.0, 600, Align::Left, Fam::Text, p.fg, true);
        g.text(hint, 16.0, 130.0, 140.0, 18.0, 12.5, 400, Align::Left, Fam::Text, p.fg2, true);
        return;
    };
    g.text(&w.place, 16.0, 12.0, 138.0, 20.0, 13.0, 600, Align::Left, Fam::Text, p.fg, true);
    g.text(&format!("{}\u{B0}", w.temp.round() as i32), 13.0, 30.0, 140.0, 58.0, 46.0, 300, Align::Left, Fam::Display, p.fg, false);
    weather_icon(g, w.code, w.day, 27.0, 98.0, 11.0, p);
    g.text(condition(w.code), 16.0, 110.0, 140.0, 20.0, 13.0, 600, Align::Left, Fam::Text, p.fg, true);
    let hl = format!("H:{}\u{B0}  L:{}\u{B0}", w.hi.round() as i32, w.lo.round() as i32);
    g.text(&hl, 16.0, 130.0, 140.0, 18.0, 12.5, 400, Align::Left, Fam::Text, p.fg2, false);
}

fn cloud(g: &mut Gfx, cx: f32, cy: f32, s: f32, c: Color) {
    g.fill_circle(cx - 0.38 * s, cy + 0.12 * s, 0.38 * s, c);
    g.fill_circle(cx + 0.02 * s, cy - 0.1 * s, 0.52 * s, c);
    g.fill_circle(cx + 0.45 * s, cy + 0.14 * s, 0.36 * s, c);
    g.fill_rrect(cx - 0.76 * s, cy + 0.1 * s, 1.56 * s, 0.4 * s, 0.2 * s, c);
}

fn sun(g: &mut Gfx, cx: f32, cy: f32, s: f32) {
    let c = Color::hex(0xFFCC33);
    g.fill_circle(cx, cy, 0.42 * s, c);
    for i in 0..8 {
        let a = i as f32 * PI / 4.0;
        g.line(cx + a.cos() * 0.64 * s, cy + a.sin() * 0.64 * s, cx + a.cos() * 0.86 * s, cy + a.sin() * 0.86 * s, 0.13 * s, c);
    }
}

fn weather_icon(g: &mut Gfx, code: i32, day: bool, cx: f32, cy: f32, s: f32, p: &Pal) {
    let cl = if p.dark { Color::hex(0x9AA6B8) } else { Color::hex(0xF4F6FA) };
    let moon = Color::hex(0xF2E3A0);
    match code {
        0 | 1 => {
            if day {
                sun(g, cx, cy, s)
            } else {
                g.fill_crescent(cx, cy, 0.7 * s, 0.38 * s, -0.3 * s, moon)
            }
        }
        2 => {
            if day {
                sun(g, cx - 0.3 * s, cy - 0.3 * s, s * 0.8)
            } else {
                g.fill_crescent(cx - 0.3 * s, cy - 0.35 * s, 0.55 * s, 0.3 * s, -0.25 * s, moon)
            }
            cloud(g, cx + 0.2 * s, cy + 0.2 * s, s * 0.8, cl);
        }
        45 | 48 => {
            for i in 0..3 {
                let y = cy - 0.45 * s + i as f32 * 0.45 * s;
                g.line(cx - 0.8 * s, y, cx + 0.8 * s, y, 0.16 * s, cl);
            }
        }
        51..=67 | 80..=82 => {
            cloud(g, cx, cy - 0.25 * s, s, cl);
            let rain = Color::hex(0x5AC8FA);
            for i in 0..3 {
                let x = cx - 0.45 * s + i as f32 * 0.45 * s;
                g.line(x, cy + 0.45 * s, x - 0.15 * s, cy + 0.85 * s, 0.14 * s, rain);
            }
        }
        71..=77 | 85 | 86 => {
            cloud(g, cx, cy - 0.25 * s, s, cl);
            for i in 0..3 {
                let x = cx - 0.45 * s + i as f32 * 0.45 * s;
                g.fill_circle(x, cy + 0.65 * s, 0.11 * s, cl);
            }
        }
        95..=99 => {
            cloud(g, cx, cy - 0.25 * s, s, cl);
            let y = Color::hex(0xFFD60A);
            g.fill_poly(
                &[
                    (cx + 0.05 * s, cy + 0.2 * s),
                    (cx - 0.25 * s, cy + 0.65 * s),
                    (cx - 0.02 * s, cy + 0.65 * s),
                    (cx - 0.15 * s, cy + 1.0 * s),
                    (cx + 0.25 * s, cy + 0.5 * s),
                    (cx + 0.02 * s, cy + 0.5 * s),
                ],
                y,
            );
        }
        _ => cloud(g, cx, cy, s, cl),
    }
}

fn fmt_time(t: f64) -> String {
    let t = t.max(0.0) as u64;
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
    } else {
        format!("{}:{:02}", t / 60, t % 60)
    }
}

fn media(g: &mut Gfx, h: &Hub, p: &Pal, art: Option<&ID2D1Bitmap>) {
    let m = &h.media;
    let l = media_layout(h.vis_on);
    let (ax, ay, asz) = (16.0, 16.0, l.art);
    let radius = if h.vis_on { 12.0 } else { 14.0 };
    match (m.has, art, &m.art) {
        (true, Some(b), Some(a)) => g.bitmap_rounded(b, a.w as f32, a.h as f32, ax, ay, asz, asz, radius),
        _ => {
            g.fill_rrect(ax, ay, asz, asz, radius, p.fg.alpha(0.10));
            g.glyph(G_MUSIC, ax + asz / 2.0, ay + asz / 2.0, asz * 0.28, p.fg2);
        }
    }
    let (x0, w) = (l.x0, l.w);
    if !m.has {
        g.text("NOW PLAYING", x0, 16.0, w, 16.0, 11.0, 600, Align::Left, Fam::Text, p.fg2, true);
        g.text("Not playing", x0, 36.0, w, 24.0, 16.0, 600, Align::Left, Fam::Text, p.fg, true);
        g.text("Play music or a video", x0, 58.0, w, 20.0, 13.0, 400, Align::Left, Fam::Text, p.fg2, true);
        g.text("in any app or browser", x0, 76.0, w, 20.0, 13.0, 400, Align::Left, Fam::Text, p.fg2, true);
        let dim = p.fg.alpha(0.3);
        g.glyph(G_PREV, l.btn_x[0], l.btn_y, 18.0, dim);
        g.glyph(G_PLAY, l.btn_x[1], l.btn_y, 24.0, dim);
        g.glyph(G_NEXT, l.btn_x[2], l.btn_y, 18.0, dim);
        return;
    }
    g.text(&m.app.to_uppercase(), x0, 16.0, w, 16.0, 11.0, 600, Align::Left, Fam::Text, p.fg2, true);
    let title = if m.title.is_empty() { "Unknown title" } else { &m.title };
    g.text(title, x0, 36.0, w, 24.0, 16.0, 600, Align::Left, Fam::Text, p.fg, true);
    g.text(&m.artist, x0, 58.0, w, 20.0, 13.0, 400, Align::Left, Fam::Text, p.fg2, true);
    if m.dur > 0.0 {
        let live = m.position(h.now_ft);
        let frac = h.seek.unwrap_or((live / m.dur) as f32).clamp(0.0, 1.0);
        let pos = frac as f64 * m.dur;
        let active = h.seek.is_some() || h.bar_hover;
        let bh = if active { 6.0 } else { 4.0 };
        let by = l.bar_y + 2.0 - bh / 2.0;
        g.fill_rrect(x0, by, w, bh, bh / 2.0, p.track);
        g.fill_rrect(x0, by, (w * frac).max(bh), bh, bh / 2.0, p.fg);
        if active && m.can_seek {
            g.fill_circle(x0 + w * frac, l.bar_y + 2.0, 7.0, p.fg);
        }
        g.text(&fmt_time(pos), x0, 99.0, 70.0, 16.0, 10.5, 600, Align::Left, Fam::Text, p.fg2, false);
        let rem = format!("-{}", fmt_time(m.dur - pos));
        g.text(&rem, x0 + w - 70.0, 99.0, 70.0, 16.0, 10.5, 600, Align::Right, Fam::Text, p.fg2, false);
    }
    g.glyph(G_PREV, l.btn_x[0], l.btn_y, 18.0, p.fg);
    g.glyph(if m.playing { G_PAUSE } else { G_PLAY }, l.btn_x[1], l.btn_y, 24.0, p.fg);
    g.glyph(G_NEXT, l.btn_x[2], l.btn_y, 18.0, p.fg);
}

/// Visualizer bars under the album art. Drawn separately so they can animate
/// without redrawing the rest of the widget.
pub fn media_bars(g: &mut Gfx, h: &Hub, p: &Pal) {
    if !h.bars_on() {
        return;
    }
    let tint = h.media.art.as_ref().filter(|_| h.media.has).map(|a| Color::hex(a.tint)).unwrap_or(p.accent);
    let tint = if p.dark && tint == WHITE { p.fg } else { tint };
    if h.taskbar {
        return taskbar_bars(g, h, p, tint);
    }
    let (x, y, w, ht) = media_layout(true).vis;
    let n = crate::vis::BANDS;
    let gap = 2.6;
    let bw = (w - gap * (n - 1) as f32) / n as f32;
    let cy = y + ht / 2.0;
    for (i, v) in h.vis.iter().enumerate() {
        let bh = (bw + (ht - bw) * v.clamp(0.0, 1.0)).max(bw);
        let bx = x + i as f32 * (bw + gap);
        let c = if *v < 0.02 { p.fg.alpha(0.25) } else { tint.alpha(0.55 + 0.45 * v) };
        g.fill_rrect(bx, cy - bh / 2.0, bw, bh, bw / 2.0, c);
    }
}

/// Visualizer strip in logical coordinates (x, y, w, h), padded.
pub fn media_bars_rect(hub: &Hub) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = if hub.taskbar { tb_layout(hub).vis } else { media_layout(true).vis };
    (x - 2.0, y - 2.0, w + 4.0, h + 4.0)
}

// ------------------------------------------------------------------ taskbar

/// Taskbar canvas (logical px). A small glass pill sits in the bottom 48 px (the
/// taskbar); on hover it grows up and out into a mini island. Everything outside the
/// shape is fully transparent, so clicks there fall through to the taskbar/desktop.
pub const TB_W: f32 = 300.0;
pub const TB_H: f32 = 132.0;
/// Compact tile (width, height) and the open card (width, height, radius).
pub const TB_PILL: (f32, f32) = (176.0, 34.0);
const TB_OPEN: (f32, f32, f32) = (288.0, 116.0, 24.0);
/// Corner radius of the compact tile: a full pill.
const TB_PILL_R: f32 = 17.0;
const TB_SIDE: f32 = 6.0;
const TB_BARS: usize = 5;

/// Top-left of the compact pill inside the canvas.
pub fn tb_pill_origin(right: bool) -> (f32, f32) {
    let x = if right { TB_W - TB_SIDE - TB_PILL.0 } else { TB_SIDE };
    (x, TB_H - 48.0 + (48.0 - TB_PILL.1) / 2.0)
}

/// Shape (x, y, w, h, radius) at expansion `t`. It grows from its taskbar corner:
/// up, and away from the screen edge it's anchored to.
pub fn tb_shape(t: f32, right: bool) -> (f32, f32, f32, f32, f32) {
    let (pw, ph) = TB_PILL;
    let (ow, oh, or) = TB_OPEN;
    let (px, py) = tb_pill_origin(right);
    let bottom = py + ph;
    // Height leads, width follows a touch later, like the island.
    let th = if t <= 1.0 { (t * 1.15).min(1.0) } else { t };
    let w = lerp(pw, ow, t).clamp(pw * 0.9, TB_W - 2.0 * TB_SIDE + 4.0);
    let h = lerp(ph, oh, th).clamp(ph * 0.9, bottom - 2.0);
    let r = lerp(TB_PILL_R, or, t.clamp(0.0, 1.0)).min(h / 2.0);
    let x = if right { px + pw - w } else { px };
    (x, bottom - h, w, h, r)
}

pub struct TbLayout {
    pub art: (f32, f32, f32, f32), // x, y, size, radius
    pub bar_x: f32,
    pub bar_w: f32,
    pub bar_y: f32,
    pub btn_y: f32,
    pub btn_x: [f32; 3],
    pub vis: (f32, f32, f32, f32),
}

/// Content positions at expansion `t` (art and bars glide between the two looks).
pub fn tb_layout(h: &Hub) -> TbLayout {
    let right = h.tb_right;
    let (cx, cy, cw, chh, _) = tb_shape(0.0, right);
    let (ox, oy, ow, _, _) = tb_shape(1.0, right);
    let k = h.isl_t.clamp(0.0, 1.05);
    let art = (lerp(cx + 5.0, ox + 14.0, k), lerp(cy + 5.0, oy + 14.0, k), lerp(chh - 10.0, 60.0, k), lerp(12.0, 18.0, k));
    let small = (cx + cw - 30.0, cy + 10.0, 18.0, chh - 20.0);
    let big = (ox + ow - 16.0 - 26.0, oy + 56.0, 26.0, 16.0);
    let vis = (lerp(small.0, big.0, k), lerp(small.1, big.1, k), lerp(small.2, big.2, k), lerp(small.3, big.3, k));
    let tx = ox + 86.0;
    TbLayout {
        art,
        bar_x: ox + 14.0,
        bar_w: ow - 28.0,
        bar_y: oy + 88.0,
        btn_y: oy + 64.0,
        btn_x: [tx + 10.0, tx + 46.0, tx + 82.0],
        vis,
    }
}

/// Buttons and the progress bar take clicks only once the card is open.
fn taskbar_hit(x: f32, y: f32, h: &Hub) -> Option<u32> {
    if !h.media.has || h.isl_t < 0.95 {
        return None;
    }
    let l = tb_layout(h);
    if h.media.can_seek && h.media.dur > 0.0 && x >= l.bar_x - 6.0 && x <= l.bar_x + l.bar_w + 6.0 && (y - l.bar_y - 2.0).abs() < 8.0 {
        return Some(HIT_SEEK);
    }
    l.btn_x.iter().position(|&bx| (x - bx).powi(2) + (y - l.btn_y).powi(2) < 16.0f32.powi(2)).map(|i| i as u32 + 1)
}

/// Album tint for the glass glow (accent when there's no art).
fn tb_tint(h: &Hub, p: &Pal) -> Color {
    h.media.art.as_ref().filter(|_| h.media.has).map(|a| Color::hex(a.tint)).unwrap_or(p.accent)
}

fn taskbar(g: &mut Gfx, h: &Hub, p: &Pal, art: Option<&ID2D1Bitmap>) {
    let m = &h.media;
    let t = h.isl_t;
    let te = t.clamp(0.0, 1.0);
    let (x, y, w, ht, r) = tb_shape(t, h.tb_right);
    let light = p.dark; // light taskbar: dark text
    let black = Color::hex(0x000000);
    // Soft shadow, stronger as it lifts off the taskbar.
    for (grow, a) in [(9.0, 0.03), (5.0, 0.05), (2.5, 0.07)] {
        let a = a * (0.35 + 0.65 * te);
        g.fill_rrect(x - grow, y - grow * 0.5, w + grow * 2.0, ht + grow * 1.5, r + grow, black.alpha(a));
    }
    // Frosted body: see-through glass on the taskbar, a deeper frost once open
    // (it then floats over whatever is behind it and must stay readable).
    let (base, sheen_top, sheen_bot, rim) = if light {
        (Color::hex(0xF7F7F9), WHITE.alpha(0.45), WHITE.alpha(0.20), black.alpha(0.06))
    } else {
        (Color::hex(0x1F1F24), WHITE.alpha(0.09), WHITE.alpha(0.03), WHITE.alpha(0.09))
    };
    g.fill_rrect(x, y, w, ht, r, base.alpha(lerp(if light { 0.35 } else { 0.25 }, 0.93, te)));
    g.fill_rrect_vgrad(x, y, w, ht, r, sheen_top, sheen_bot);
    // A gentle wash of the album color, like light through tinted glass.
    let tint = tb_tint(h, p);
    g.fill_rrect_vgrad(x, y, w, ht, r, tint.alpha(if light { 0.06 } else { 0.09 }), tint.alpha(0.01));
    g.stroke_rrect(x + 0.5, y + 0.5, w - 1.0, ht - 1.0, (r - 0.5).max(0.0), 1.0, rim);
    outline(g, h, x, y, w, ht, r);
    // Specular highlight along the top edge.
    g.fill_rrect(x + r * 0.7, y + 1.0, (w - r * 1.4).max(0.0), 1.0, 0.5, WHITE.alpha(if light { 0.5 } else { 0.07 }));
    unsafe {
        g.rt.PushAxisAlignedClip(
            &windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + ht },
            windows::Win32::Graphics::Direct2D::D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
        );
    }
    let l = tb_layout(h);
    let (ax, ay, asz, ar) = l.art;
    match (m.has, art, &m.art) {
        (true, Some(b), Some(a)) => g.bitmap_rounded(b, a.w as f32, a.h as f32, ax, ay, asz, asz, ar),
        _ => {
            g.fill_rrect(ax, ay, asz, asz, ar, p.fg.alpha(0.12));
            g.glyph(G_MUSIC, ax + asz / 2.0, ay + asz / 2.0, asz * 0.42, p.fg2);
        }
    }
    let title = if !m.has {
        "Not playing"
    } else if m.title.is_empty() {
        "Unknown title"
    } else {
        &m.title
    };
    // Compact: just the title, snug beside the little art and bars.
    let ca = (1.0 - t * 3.0).clamp(0.0, 1.0);
    if ca > 0.0 {
        let (cx, cy, cw, chh, _) = tb_shape(0.0, h.tb_right);
        let tx = cx + chh - 10.0 + 12.0;
        let tw = cw - (tx - cx) - if h.tb_vis { 36.0 } else { 14.0 };
        g.text(title, tx, cy, tw, chh, 11.5, 600, Align::Left, Fam::Text, p.fg.alpha(ca), true);
    }
    // Open: title, artist, controls and progress.
    let fa = ((t - 0.65) / 0.35).clamp(0.0, 1.0);
    if fa > 0.0 {
        let (ox, oy, ow, _, _) = tb_shape(1.0, h.tb_right);
        let tx = ox + 86.0;
        let tw = ow - 86.0 - 14.0;
        g.text(title, tx, oy + 14.0, tw, 20.0, 14.0, 600, Align::Left, Fam::Text, p.fg.alpha(fa), true);
        let sub = if m.has { if m.artist.is_empty() { m.app.as_str() } else { m.artist.as_str() } } else { "Play music in any app" };
        g.text(sub, tx, oy + 34.0, tw, 18.0, 12.0, 400, Align::Left, Fam::Text, p.fg2.alpha(fa * p.fg2.a), true);
        let c = if m.has { p.fg.alpha(fa) } else { p.fg.alpha(0.3 * fa) };
        for (i, &bx) in l.btn_x.iter().enumerate() {
            if m.has && h.tb_hot == i as u32 + 1 {
                g.fill_circle(bx, l.btn_y, 15.0, p.fg.alpha(0.10 * fa));
            }
            let (glyph, size) = match i {
                0 => (G_PREV, 13.0),
                1 => (if m.playing { G_PAUSE } else { G_PLAY }, 18.0),
                _ => (G_NEXT, 13.0),
            };
            g.glyph(glyph, bx, l.btn_y, size, c);
        }
        if m.has && m.dur > 0.0 {
            let live = m.position(h.now_ft);
            let frac = h.seek.unwrap_or((live / m.dur) as f32).clamp(0.0, 1.0);
            let pos = frac as f64 * m.dur;
            let active = h.seek.is_some() || h.tb_hot == HIT_SEEK;
            let bh = if active { 5.0 } else { 4.0 };
            let by = l.bar_y + 2.0 - bh / 2.0;
            g.fill_rrect(l.bar_x, by, l.bar_w, bh, bh / 2.0, p.track.alpha(p.track.a * fa));
            g.fill_rrect(l.bar_x, by, (l.bar_w * frac).max(bh), bh, bh / 2.0, p.fg.alpha(0.9 * fa));
            if active && m.can_seek {
                g.fill_circle(l.bar_x + l.bar_w * frac, l.bar_y + 2.0, 6.0, p.fg.alpha(fa));
            }
            let tc = p.fg2.alpha(p.fg2.a * fa);
            g.text(&fmt_time(pos), l.bar_x, l.bar_y + 7.0, 60.0, 14.0, 10.0, 600, Align::Left, Fam::Text, tc, false);
            let rem = format!("-{}", fmt_time(m.dur - pos));
            g.text(&rem, l.bar_x + l.bar_w - 60.0, l.bar_y + 7.0, 60.0, 14.0, 10.0, 600, Align::Right, Fam::Text, tc, false);
        }
    }
    unsafe {
        g.rt.PopAxisAlignedClip();
    }
}

/// A few slim bars (the 16 bands folded down); they glide with the shape.
fn taskbar_bars(g: &mut Gfx, h: &Hub, p: &Pal, tint: Color) {
    let (x, y, w, ht) = tb_layout(h).vis;
    let gap = 2.0;
    let bw = (w - gap * (TB_BARS - 1) as f32) / TB_BARS as f32;
    let cy = y + ht / 2.0;
    let per = crate::vis::BANDS / TB_BARS;
    for i in 0..TB_BARS {
        let v = (h.vis[i * per..(i + 1) * per].iter().sum::<f32>() / per as f32).clamp(0.0, 1.0);
        let v = if h.media.has && h.media.playing { v } else { 0.0 };
        let bh = (bw + (ht - bw) * v).max(bw);
        let bx = x + i as f32 * (bw + gap);
        let c = if v < 0.02 { p.fg.alpha(0.3) } else { tint.alpha(0.7 + 0.3 * v) };
        g.fill_rrect(bx, cy - bh / 2.0, bw, bh, bw / 2.0, c);
    }
}
fn ring_stat(g: &mut Gfx, cx: f32, cy: f32, r: f32, w: f32, v: f32, c: Color, p: &Pal, size: f32) {
    g.stroke_circle(cx, cy, r, w, p.track);
    g.arc(cx, cy, r, w, v / 100.0, c);
    let s = format!("{}%", v.round() as i32);
    g.text(&s, cx - r, cy - r, r * 2.0, r * 2.0, size, 500, Align::Center, Fam::Display, p.fg, false);
}

fn system(g: &mut Gfx, h: &Hub, p: &Pal) {
    label(g, "System", p);
    let mut items = vec![(h.cpu, "CPU", 0), (h.ram, "Memory", 1), (h.disk, "Disk", 2)];
    if let Some(gpu) = h.gpu {
        items.insert(1, (gpu, "GPU", 3));
    }
    // Three rings, or four slightly smaller ones with the GPU.
    let (x0, step, r, w, fs) = if items.len() == 4 { (52.0, 84.0, 29.0, 6.5, 13.5) } else { (62.0, 116.0, 33.0, 7.5, 15.0) };
    for (i, (v, name, ci)) in items.iter().enumerate() {
        let cx = x0 + i as f32 * step;
        ring_stat(g, cx, 84.0, r, w, *v, p.ring(*ci), p, fs);
        g.text(name, cx - 42.0, 128.0, 84.0, 20.0, 12.5, 600, Align::Center, Fam::Text, p.fg2, false);
    }
}

fn days_in_month(y: u16, m: u16) -> u16 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

fn calendar(g: &mut Gfx, h: &Hub, p: &Pal) {
    let n = &h.now;
    let month = MONTHS[(n.wMonth as usize + 11) % 12].to_uppercase();
    g.text(&month, 14.0, 10.0, 142.0, 20.0, 12.5, 700, Align::Left, Fam::Text, p.accent, false);
    let letters = if h.monday { ["M", "T", "W", "T", "F", "S", "S"] } else { ["S", "M", "T", "W", "T", "F", "S"] };
    let (x0, cw) = (10.0, 150.0 / 7.0);
    for (i, l) in letters.iter().enumerate() {
        g.text(l, x0 + i as f32 * cw, 30.0, cw, 16.0, 10.5, 600, Align::Center, Fam::Text, p.fg2, false);
    }
    let shift = if h.monday { 6 } else { 0 };
    let today_col = (n.wDayOfWeek as i32 + shift) % 7;
    let first = (today_col - ((n.wDay as i32 - 1) % 7) + 7) % 7;
    let days = days_in_month(n.wYear, n.wMonth) as i32;
    let rows = (first + days + 6) / 7;
    let rh = 112.0 / rows as f32;
    for d in 1..=days {
        let idx = first + d - 1;
        let (col, row) = (idx % 7, idx / 7);
        let cx = x0 + (col as f32 + 0.5) * cw;
        let cy = 52.0 + (row as f32 + 0.5) * rh;
        let s = d.to_string();
        if d == n.wDay as i32 {
            g.fill_circle(cx, cy, 9.5, p.accent);
            g.text(&s, cx - 12.0, cy - 9.0, 24.0, 18.0, 11.0, 700, Align::Center, Fam::Text, WHITE, false);
        } else {
            g.text(&s, cx - 12.0, cy - 9.0, 24.0, 18.0, 11.0, 500, Align::Center, Fam::Text, p.fg, false);
        }
    }
}

fn battery(g: &mut Gfx, h: &Hub, p: &Pal) {
    label(g, "Battery", p);
    let b = h.bat;
    let col = if b.has && b.pct <= 20 && !b.charging { p.ring(4) } else { p.ring(3) };
    let (cx, cy, r) = (85.0, 84.0, 40.0);
    g.stroke_circle(cx, cy, r, 8.0, p.track);
    if b.has {
        g.arc(cx, cy, r, 8.0, b.pct as f32 / 100.0, col);
        g.text(&format!("{}%", b.pct), cx - r, cy - r, r * 2.0, r * 2.0, 22.0, 300, Align::Center, Fam::Display, p.fg, false);
    } else {
        g.arc(cx, cy, r, 8.0, 1.0, col);
        g.glyph(G_BOLT, cx, cy, 22.0, p.fg);
    }
    let status = match (b.has, b.charging, b.ac) {
        (false, _, _) => "Plugged in",
        (true, true, _) => "Charging",
        (true, false, true) => "Fully charged",
        (true, false, false) => "On battery",
    };
    let w = g.text_width(status, 13.0, 600, Fam::Text);
    let show_bolt = b.has && b.charging;
    let total = w + if show_bolt { 16.0 } else { 0.0 };
    let x = cx - total / 2.0;
    if show_bolt {
        g.glyph(G_BOLT, x + 6.0, 142.0, 11.0, col);
    }
    g.text(status, x + total - w, 132.0, w + 4.0, 20.0, 13.0, 600, Align::Left, Fam::Text, p.fg2, false);
}

fn speed(bps: f64) -> (String, &'static str) {
    let (v, u) = if bps >= 1024.0 * 1024.0 * 1024.0 {
        (bps / (1024.0 * 1024.0 * 1024.0), "GB/s")
    } else if bps >= 1024.0 * 1024.0 {
        (bps / (1024.0 * 1024.0), "MB/s")
    } else if bps >= 1024.0 {
        (bps / 1024.0, "KB/s")
    } else {
        (bps, "B/s")
    };
    let s = if v >= 100.0 || u == "B/s" { format!("{:.0}", v) } else { format!("{:.1}", v) };
    (s, u)
}

fn network(g: &mut Gfx, h: &Hub, p: &Pal) {
    label(g, "Network", p);
    for (i, (v, glyph, c)) in [(h.down, G_DOWN, p.ring(0)), (h.up, G_UP, p.ring(2))].iter().enumerate() {
        let y = 40.0 + i as f32 * 56.0;
        g.glyph(*glyph, 26.0, y + 22.0, 15.0, *c);
        let (num, unit) = speed(*v);
        g.text(&num, 42.0, y, 100.0, 44.0, 30.0, 300, Align::Left, Fam::Display, p.fg, false);
        let w = g.text_width(&num, 30.0, 300, Fam::Display);
        g.text(unit, 42.0 + w + 4.0, y + 12.0, 60.0, 24.0, 12.5, 600, Align::Left, Fam::Text, p.fg2, false);
    }
}

fn notes(g: &mut Gfx, h: &Hub, p: &Pal) {
    g.text("Reminders", 16.0, 12.0, 200.0, 20.0, 13.0, 700, Align::Left, Fam::Text, p.accent, false);
    let open = h.notes.iter().filter(|n| !n.0).count();
    g.text(&open.to_string(), WIDE - 150.0, 12.0, 100.0, 20.0, 13.0, 600, Align::Right, Fam::Text, p.fg2, false);
    g.fill_circle(WIDE - 28.0, 22.0, 13.0, p.accent.alpha(0.22));
    g.glyph(G_ADD, WIDE - 28.0, 22.0, 11.0, p.accent);
    if h.notes.is_empty() {
        g.text("No reminders", 16.0, 60.0, 300.0, 24.0, 15.0, 600, Align::Left, Fam::Text, p.fg, false);
        g.text("Press + to add one", 16.0, 84.0, 320.0, 20.0, 13.0, 400, Align::Left, Fam::Text, p.fg2, false);
        return;
    }
    for (row, &i) in notes_view(&h.notes).iter().enumerate() {
        let (done, t) = &h.notes[i];
        let y = NOTE_Y0 + row as f32 * NOTE_ROW;
        if *done {
            g.glyph(G_DONE, 25.0, y + 12.0, 15.0, p.accent);
        } else {
            g.glyph(G_CIRCLE, 25.0, y + 12.0, 15.0, p.fg2);
        }
        g.text(t, 42.0, y, WIDE - 58.0, NOTE_ROW, 14.0, 400, Align::Left, Fam::Text, if *done { p.fg2 } else { p.fg }, true);
    }
}

fn analog(g: &mut Gfx, h: &Hub, p: &Pal) {
    let (cx, cy, r) = (85.0, 85.0, 66.0);
    g.fill_circle(cx, cy, r + 4.0, p.fg.alpha(0.06));
    for i in 0..12 {
        let a = i as f32 * PI / 6.0;
        let major = i % 3 == 0;
        let (r0, w) = if major { (r - 11.0, 3.0) } else { (r - 7.0, 2.0) };
        let c = if major { p.fg } else { p.fg2 };
        g.line(cx + a.sin() * r0, cy - a.cos() * r0, cx + a.sin() * (r - 2.0), cy - a.cos() * (r - 2.0), w, c);
    }
    let n = &h.now;
    let sec = n.wSecond as f32;
    let min = n.wMinute as f32 + if h.seconds { sec / 60.0 } else { 0.0 };
    let hr = (n.wHour % 12) as f32 + min / 60.0;
    let hand = |g: &mut Gfx, ang: f32, len: f32, tail: f32, w: f32, c: Color| {
        let (s, co) = (ang.sin(), ang.cos());
        g.line(cx - s * tail, cy + co * tail, cx + s * len, cy - co * len, w, c);
    };
    hand(g, hr / 12.0 * 2.0 * PI, 34.0, 0.0, 5.5, p.fg);
    hand(g, min / 60.0 * 2.0 * PI, 52.0, 0.0, 3.5, p.fg);
    g.fill_circle(cx, cy, 4.5, p.fg);
    if h.seconds {
        hand(g, sec / 60.0 * 2.0 * PI, 58.0, 12.0, 1.5, p.accent);
    }
    g.fill_circle(cx, cy, 2.5, p.accent);
}

