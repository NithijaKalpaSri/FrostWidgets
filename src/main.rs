#![windows_subsystem = "windows"]
//! Frost Widgets: tiny, event-driven glass desktop widgets for Windows.

mod config;
mod gfx;
mod media;
mod sys;
mod util;
mod widgets;
mod editor;
mod settings;
mod vis;
mod gpu;
mod photos;
mod msgs;
mod desk_pet;
mod taskbar;

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::time::SystemTime;

use windows::core::*;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::*;
use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::System::RemoteDesktop::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::SystemServices::GUID_CONSOLE_DISPLAY_STATE;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use config::Config;
use gfx::{Color, Gfx, Look, Surface, Wall};
use widgets::{Hub, Kind, Pal, GAP, KINDS, SMALL};

const WM_APP_MEDIA: u32 = WM_APP + 1;
const WM_APP_WEATHER: u32 = WM_APP + 2;
const WM_APP_TRAY: u32 = WM_APP + 3;
const WM_APP_MENU: u32 = WM_APP + 4;
const WM_APP_FG: u32 = WM_APP + 5;
const WM_APP_VIS: u32 = WM_APP + 6;
const WM_APP_PHOTO: u32 = WM_APP + 7;
const WM_APP_MSGS: u32 = WM_APP + 8;
const WM_APP_PICKDIR: u32 = WM_APP + 9;
const WM_APP_PICKCOLOR: u32 = WM_APP + 10;
/// Index of the now-playing widget in KINDS.
const MEDIA: usize = 2;
const SYSTEM: usize = 3;
const PHOTOS: usize = 9;
const MESSAGES: usize = 10;

const T_TICK: usize = 1;
const T_STATS: usize = 2;
const T_WEATHER: usize = 3;
const T_RELOAD: usize = 4;
const T_WALL: usize = 5;
const T_DRAG: usize = 6;
const T_ANIM: usize = 7;
const T_HOVER: usize = 8;
const T_HOLD: usize = 9;
const T_PHOTO: usize = 10;
const T_FS: usize = 11;
const T_PET: usize = 12;
const T_TBAR: usize = 13;
/// How long the island stays open when a song starts.
const ISLAND_SHOW_MS: u32 = 3000;

/// Space around the glass card for its shadow (logical px).
const MARGIN: f32 = 30.0;
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_NAME: &str = "FrostWidgets";

static MAIN: AtomicIsize = AtomicIsize::new(0);
static DESK_FRONT: AtomicBool = AtomicBool::new(false);
/// Now playing floats above open windows: Island style + "Island over windows",
/// or the Taskbar style (it has to sit above the taskbar).
static ISLAND_TOP: AtomicBool = AtomicBool::new(false);

fn island_top(i: usize) -> bool {
    i == MEDIA && ISLAND_TOP.load(Ordering::Relaxed)
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Run `f` on the app state. Returns None when re-entered (nested window messages),
/// in which case the caller falls back to default handling.
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|a| a.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

fn main_hwnd() -> HWND {
    HWND(MAIN.load(Ordering::Relaxed) as *mut _)
}

struct W {
    kind: Kind,
    hwnd: HWND,
    surf: Option<Surface>,
    bg: Vec<u32>,
    bg_ok: bool,
    s: f32,
    gx: i32,
    gy: i32,
    sig: u64,
    dirty: bool,
    dark_text: bool,
    art: Option<ID2D1Bitmap>,
    art_id: u64,
    mon: RECT,
    /// Now playing only: the frame without visualizer bars, while bars animate.
    cache: Vec<u32>,
}

impl W {
    fn new(kind: Kind) -> Self {
        W {
            kind,
            hwnd: HWND::default(),
            surf: None,
            bg: Vec::new(),
            bg_ok: false,
            s: 1.0,
            gx: 0,
            gy: 0,
            sig: 0,
            dirty: true,
            dark_text: false,
            art: None,
            art_id: 0,
            mon: RECT::default(),
            cache: Vec::new(),
        }
    }
    fn alive(&self) -> bool {
        !self.hwnd.0.is_null()
    }
    /// Window size in px and the shadow margin in px.
    fn dims(&self) -> (i32, i32, i32) {
        let (lw, lh) = self.kind.size();
        // The taskbar strip has no shadow, so no margin around it.
        let m = if self.kind == Kind::Media && widgets::TASKBAR.load(Ordering::Relaxed) { 0 } else { (MARGIN * self.s).round() as i32 };
        ((lw * self.s).round() as i32 + 2 * m, (lh * self.s).round() as i32 + 2 * m, m)
    }
}

struct App {
    hinst: HINSTANCE,
    main: HWND,
    gfx: Gfx,
    screen: HDC,
    dir: PathBuf,
    cfg: Config,
    cfg_time: Option<SystemTime>,
    notes_time: Option<SystemTime>,
    hub: Hub,
    stats: sys::Stats,
    ws: Vec<W>,
    walls: Vec<Wall>,
    media: media::Media,
    geo: Option<sys::Geo>,
    weather_busy: bool,
    dark: bool,
    accent: Color,
    display_off: bool,
    locked_session: bool,
    moving: Option<usize>,
    drag_dirty: bool,
    editor: Option<editor::Editor>,
    settings: Option<settings::Settings>,
    big_icon: HICON,
    mem_mb: f32,
    vis: vis::Vis,
    seeking: Option<usize>,
    mouse_tracking: bool,
    weather_fails: u32,
    tray_icon: HICON,
    taskbar_created: u32,
    isl: IslandAnim,
    gpu: Option<gpu::Gpu>,
    gpus: Option<Vec<gpu::Adapter>>,
    photos: Option<photos::Photos>,
    photo: Option<photos::Photo>,
    photo_seq: u64,
    msgs: msgs::Msgs,
    /// Floating island currently shown (false while an app is full screen).
    island_shown: bool,
    /// Desktop pet window, while it's on.
    dpet: Option<PetWin>,
    /// Taskbar style: last measurement of the taskbar's buttons, when it was taken,
    /// and whether the icons are centred.
    tb_probe: taskbar::Probe,
    tb_probe_at: Option<std::time::Instant>,
    tb_centered: bool,
    /// The alignment setting at the last measurement, to spot it being changed.
    tb_setting: bool,
}

/// Desktop pet window and its state.
struct PetWin {
    hwnd: HWND,
    surf: Option<Surface>,
    /// Screen px per sprite cell.
    s: i32,
    pet: desk_pet::Pet,
    /// Ground line (top of the taskbar) and the walkable range for the window's left edge.
    ground: i32,
    lo: i32,
    hi: i32,
    mon: RECT,
    shown: bool,
    timer: bool,
    /// Mouse drag: cursor and pet position at the press, and whether it has moved.
    drag: Option<(POINT, f32, f32, bool)>,
    buf: Vec<u32>,
}

/// Island expand/collapse animation state.
#[derive(Default)]
struct IslandAnim {
    from: f32,
    target: f32,
    start: Option<std::time::Instant>,
    hover: bool,
    /// Stay open until this time (after a song starts, or briefly after the pointer leaves).
    hold: Option<std::time::Instant>,
    /// Last track seen, to spot "started playing" and "new song".
    last: (bool, String),
}

fn local_now() -> (SYSTEMTIME, i64) {
    unsafe {
        let st = GetLocalTime();
        let ft = GetSystemTimePreciseAsFileTime();
        (st, ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64)
    }
}

fn system_dark() -> bool {
    util::reg_dword(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "AppsUseLightTheme",
    ) == Some(0)
}

/// Is the taskbar light? It follows the Windows mode, not the app mode.
fn taskbar_light() -> bool {
    util::reg_dword(
        HKEY_CURRENT_USER,
        "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
        "SystemUsesLightTheme",
    ) == Some(1)
}

fn monitor_rect(hmon: HMONITOR) -> (RECT, RECT) {
    let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    unsafe {
        let _ = GetMonitorInfoW(hmon, &mut mi);
    }
    (mi.rcMonitor, mi.rcWork)
}

fn monitor_dpi(hmon: HMONITOR) -> u32 {
    let (mut x, mut y) = (96u32, 96u32);
    unsafe {
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut x, &mut y);
    }
    x.max(48)
}

impl App {
    fn look(&self) -> Look {
        Look { dark: self.dark, glass: self.cfg.glass }
    }

    fn sync_cfg_to_hub(&mut self) {
        let c = &self.cfg;
        self.hub.city = self.geo.as_ref().filter(|g| g.query == c.city).map(|g| g.name.clone()).unwrap_or(c.city.clone());
        self.hub.h24 = c.clock_24h;
        self.hub.seconds = c.analog_seconds;
        self.hub.monday = c.week_start.eq_ignore_ascii_case("monday");
        self.hub.imperial = c.units.eq_ignore_ascii_case("imperial");
        self.hub.vis_on = c.visualizer;
        self.hub.island = c.island();
        self.hub.taskbar = c.taskbar();
        self.hub.tb_vis = c.taskbar_vis;
        self.hub.outline = c.island_outline.then(|| self.outline_rgb());
        self.hub.msg_preview = c.msg_preview;
        widgets::ISLAND.store(self.hub.island, Ordering::Relaxed);
        widgets::TASKBAR.store(self.hub.taskbar, Ordering::Relaxed);
        ISLAND_TOP.store((self.hub.island && c.island_top) || self.hub.taskbar, Ordering::Relaxed);
        self.dark = match c.theme.to_ascii_lowercase().as_str() {
            "dark" => true,
            "light" => false,
            _ => system_dark(),
        };
        self.accent = Color::parse(&c.accent).unwrap_or(Color::hex(0xFF6A88));
    }

    fn save_cfg(&mut self) {
        config::save(&self.dir, &self.cfg);
        self.cfg_time = config::mtime(&self.dir.join("config.toml"));
    }

    /// Fill in positions for widgets that have none, using a 4-column grid
    /// in the top-right corner of the primary monitor.
    fn layout_defaults(&mut self) {
        unsafe {
            let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let (_, work) = monitor_rect(hmon);
            let s = monitor_dpi(hmon) as f32 / 96.0 * self.cfg.scale;
            let cell = (SMALL + GAP) * s;
            let total = (4.0 * SMALL + 3.0 * GAP) * s;
            let ox = work.right as f32 - total - 36.0 * s;
            let oy = work.top as f32 + 36.0 * s;
            for k in KINDS {
                let p = self.cfg.pos_mut(k.key());
                if p.x.is_none() || p.y.is_none() {
                    let (c, r) = k.slot();
                    p.x = Some((ox + c as f32 * cell).round() as i32);
                    p.y = Some((oy + r as f32 * cell).round() as i32);
                }
            }
        }
    }

    /// Saved top-left of a widget. The island has its own spot, defaulting to
    /// the top centre of the main screen, like a notch.
    fn origin(&self, i: usize) -> (i32, i32) {
        if i == MEDIA && self.hub.taskbar {
            if let Some(p) = self.tb_spot() {
                return p;
            }
        }
        if i == MEDIA && self.hub.island {
            if let (Some(x), Some(y)) = (self.cfg.island_x, self.cfg.island_y) {
                return (x, y);
            }
            unsafe {
                let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
                let (_, work) = monitor_rect(hmon);
                let s = monitor_dpi(hmon) as f32 / 96.0 * self.cfg.scale;
                let w = (widgets::ISL_CW * s).round() as i32;
                return (work.left + (work.right - work.left - w) / 2, work.top);
            }
        }
        let p = self.cfg.pos(KINDS[i].key());
        (p.x.unwrap_or(100), p.y.unwrap_or(100))
    }

    fn create_widget(&mut self, i: usize, show: bool) {
        if self.ws[i].alive() {
            return;
        }
        let k = KINDS[i];
        if i == MEDIA && self.hub.taskbar {
            self.tb_measure();
        }
        let (gx, gy) = self.origin(i);
        let zoom = self.zoom(i);
        // The taskbar strip must NOT be owned by the taskbar: an owner in another process
        // ties Explorer's input to this thread (the taskbar freezes whenever we're busy)
        // and every re-raise of the strip drags the taskbar over Start and the flyouts.
        // It floats on its own instead and is kept just above the taskbar (tb_raise).
        // A floating island can't be pinned to the desktop and still sit above apps.
        let owner = if island_top(i) {
            HWND::default()
        } else {
            desktop_owner()
        };
        unsafe {
            let hmon = MonitorFromPoint(POINT { x: gx, y: gy }, MONITOR_DEFAULTTONEAREST);
            let s = monitor_dpi(hmon) as f32 / 96.0 * zoom;
            let w = &mut self.ws[i];
            *w = W::new(k);
            w.s = s;
            w.gx = gx;
            w.gy = gy;
            let (ww, wh, m) = w.dims();
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("FrostWidget"),
                w!(""),
                WS_POPUP,
                gx - m,
                gy - m,
                ww,
                wh,
                owner,
                None,
                self.hinst,
                None,
            ) else {
                return;
            };
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, i as isize + 1);
            w.hwnd = hwnd;
            // Correct scale for the monitor the window actually landed on.
            let s2 = GetDpiForWindow(hwnd) as f32 / 96.0 * zoom;
            if (s2 - s).abs() > 0.01 {
                w.s = s2;
                let (_, _, m2) = w.dims();
                let _ = SetWindowPos(hwnd, None, gx - m2, gy - m2, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
        if i == PHOTOS {
            self.photo_check_size();
            self.schedule_photo();
        }
        self.draw(i);
        let show = show && !(i == MEDIA && self.tb_idle());
        if show {
            unsafe {
                let h = self.ws[i].hwnd;
                let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                let after = if island_top(i) {
                    HWND_TOPMOST
                } else if DESK_FRONT.load(Ordering::Relaxed) {
                    HWND_TOP
                } else {
                    HWND_BOTTOM
                };
                if i == MEDIA && self.hub.taskbar {
                    self.tb_raise();
                } else {
                    let _ = SetWindowPos(h, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                }
            }
        }
        if island_top(i) {
            self.island_shown = show;
            unsafe {
                // Cheap once-a-second check for apps going full screen (no focus change needed).
                SetCoalescableTimer(self.main, T_FS, 1000, None, 250);
            }
        }
    }

    /// Widgets are owned by the desktop window so "Show desktop" (Win+D, touchpad
    /// swipe down) keeps them on screen. If Explorer restarts, the old owner dies and
    /// takes the widgets with it: recreate those and re-own the survivors.
    fn reattach(&mut self) {
        let owner = desktop_owner();
        for i in 0..self.ws.len() {
            let w = &mut self.ws[i];
            if w.alive() && !unsafe { IsWindow(w.hwnd) }.as_bool() {
                *w = W::new(w.kind);
            }
            if w.alive() && island_top(i) {
                continue;
            }
            if w.alive() {
                unsafe {
                    if GetWindowLongPtrW(w.hwnd, GWLP_HWNDPARENT) != owner.0 as isize {
                        SetWindowLongPtrW(w.hwnd, GWLP_HWNDPARENT, owner.0 as isize);
                    }
                }
            } else if self.cfg.pos(KINDS[i].key()).enabled {
                self.create_widget(i, true);
            }
        }
    }

    fn destroy_widget(&mut self, i: usize) {
        let w = &mut self.ws[i];
        if w.alive() {
            unsafe {
                let _ = DestroyWindow(w.hwnd);
            }
        }
        *w = W::new(w.kind);
    }

    fn rebuild_all(&mut self) {
        for i in 0..self.ws.len() {
            self.destroy_widget(i);
        }
        self.layout_defaults();
        for (i, k) in KINDS.iter().enumerate() {
            if self.cfg.pos(k.key()).enabled {
                self.create_widget(i, true);
            }
        }
        self.schedule_tick();
    }

    fn wall_index(&mut self, mon: RECT) -> usize {
        if let Some(i) = self.walls.iter().position(|w| w.mon == mon) {
            return i;
        }
        self.walls.push(gfx::load_wall(&self.gfx.wic, mon));
        self.walls.len() - 1
    }

    fn rebuild_bg(&mut self, i: usize) {
        let hwnd = self.ws[i].hwnd;
        let (mon, _) = monitor_rect(unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) });
        if self.ws[i].kind == Kind::Media && (self.hub.island || self.hub.taskbar) {
            // The island draws its own black pill: no glass, fully transparent around it.
            // The taskbar strip shows the taskbar through it; a barely-there fill keeps the
            // whole strip clickable, and the text follows the taskbar's light/dark color.
            // Only the pill itself gets that fill: the rest of the canvas (the room it
            // opens into, above the taskbar) must stay click-through, or merely passing
            // near the taskbar would open it.
            let tb = self.hub.taskbar;
            let right = self.hub.tb_right;
            let w = &mut self.ws[i];
            let (ww, wh, _) = w.dims();
            w.bg = vec![0; (ww * wh) as usize];
            if tb {
                let (px, py, pw, ph, _) = widgets::tb_shape(0.0, right);
                let s = w.s;
                let (x0, y0) = (((px * s).floor() as i32).max(0), ((py * s).floor() as i32).max(0));
                let (x1, y1) = ((((px + pw) * s).ceil() as i32).min(ww), (((py + ph) * s).ceil() as i32).min(wh));
                for y in y0..y1 {
                    for x in x0..x1 {
                        w.bg[(y * ww + x) as usize] = 0x0100_0000;
                    }
                }
            }
            w.dark_text = tb && taskbar_light();
            w.bg_ok = true;
            w.mon = mon;
            return;
        }
        let wi = self.wall_index(mon);
        let look = self.look();
        let w = &mut self.ws[i];
        let (ww, wh, m) = w.dims();
        let (bg, lum) = gfx::build_glass(&self.walls[wi], w.gx - m, w.gy - m, ww, wh, m, w.s, &look);
        w.bg = bg;
        w.dark_text = lum > 0.6;
        w.bg_ok = true;
        w.mon = mon;
    }

    fn draw(&mut self, i: usize) {
        if !self.ws[i].alive() {
            return;
        }
        if !self.ws[i].bg_ok {
            self.rebuild_bg(i);
        }
        // Album art lives as a D2D bitmap so it's decoded only once per track.
        if self.ws[i].kind == Kind::Photos && self.ws[i].art_id != self.hub.photo_id {
            self.ws[i].art = self.photo.as_ref().and_then(|p| self.gfx.bitmap(&p.px, p.w, p.h));
            self.ws[i].art_id = self.hub.photo_id;
        }
        if self.ws[i].kind == Kind::Media && self.ws[i].art_id != self.hub.media.art_id {
            self.ws[i].art = self.hub.media.art.as_ref().and_then(|a| self.gfx.bitmap(&a.px, a.w, a.h));
            self.ws[i].art_id = self.hub.media.art_id;
        }
        let pal = Pal::new(self.ws[i].dark_text, self.accent);
        let w = &mut self.ws[i];
        let (ww, wh, m) = w.dims();
        if w.surf.as_ref().map(|s| (s.w, s.h)) != Some((ww, wh)) {
            w.surf = Surface::new(self.screen, ww, wh);
        }
        let Some(surf) = w.surf.as_mut() else { return };
        if w.bg.len() == (ww * wh) as usize {
            surf.pixels().copy_from_slice(&w.bg);
        }
        let bars = w.kind == Kind::Media && self.hub.bars_on();
        let keep_cache = bars && self.vis.is_active();
        unsafe {
            let rt = &self.gfx.rt;
            if rt.BindDC(surf.dc, &RECT { left: 0, top: 0, right: ww, bottom: wh }).is_err() {
                return;
            }
            let tf = Matrix3x2 { M11: w.s, M12: 0.0, M21: 0.0, M22: w.s, M31: m as f32, M32: m as f32 };
            rt.BeginDraw();
            rt.SetTransform(&tf);
            if w.kind == Kind::Media && self.hub.island {
                self.hub.isl_cw = widgets::island_width(&mut self.gfx, &self.hub);
            }
            widgets::draw(w.kind, &mut self.gfx, &self.hub, &pal, w.art.as_ref());
            let _ = self.gfx.rt.EndDraw(None, None);
            if bars {
                if keep_cache {
                    // Remember the frame without bars so animation frames only repaint the bar strip.
                    let px = surf.pixels();
                    if w.cache.len() == px.len() {
                        w.cache.copy_from_slice(px);
                    } else {
                        w.cache = px.to_vec();
                    }
                }
                self.gfx.rt.BeginDraw();
                self.gfx.rt.SetTransform(&tf);
                widgets::media_bars(&mut self.gfx, &self.hub, &pal);
                let _ = self.gfx.rt.EndDraw(None, None);
            }
            if !keep_cache && !w.cache.is_empty() {
                w.cache = Vec::new();
            }
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(
                w.hwnd,
                self.screen,
                None,
                Some(&SIZE { cx: ww, cy: wh }),
                surf.dc,
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }

    /// Is a maximized or fullscreen app covering this monitor?
    fn covered(&self, mon: RECT, fg: HWND, fg_mon: RECT, fg_rect: RECT, fg_max: bool) -> bool {
        if fg.0.is_null() {
            return false;
        }
        if fg_mon != mon {
            return false;
        }
        fg_max
            || (fg_rect.left <= mon.left && fg_rect.top <= mon.top && fg_rect.right >= mon.right && fg_rect.bottom >= mon.bottom)
    }

    /// Is an app full screen on this monitor (games, videos, presentations)?
    /// Maximized windows don't count, even when they cover the screen (auto-hide taskbar).
    fn fullscreen(&self, mon: RECT, quns: QUERY_USER_NOTIFICATION_STATE, fg: HWND, fg_mon: RECT, fg_rect: RECT) -> bool {
        if matches!(quns, QUNS_RUNNING_D3D_FULL_SCREEN | QUNS_PRESENTATION_MODE) {
            return true;
        }
        !fg.0.is_null()
            && !unsafe { IsZoomed(fg) }.as_bool()
            && fg_mon == mon
            && fg_rect.left <= mon.left
            && fg_rect.top <= mon.top
            && fg_rect.right >= mon.right
            && fg_rect.bottom >= mon.bottom
    }

    /// Redraw only widgets whose visible content changed, and only when visible.
    fn refresh(&mut self, force: bool) {
        let paused = self.display_off || self.locked_session;
        let quns = unsafe { SHQueryUserNotificationState() }.unwrap_or(QUNS_ACCEPTS_NOTIFICATIONS);
        let exclusive = quns == QUNS_RUNNING_D3D_FULL_SCREEN;
        let (fg, fg_mon, fg_rect, fg_max) = unsafe {
            let fg = GetForegroundWindow();
            let cls = util::class_name(fg);
            if fg.0.is_null()
                || IsIconic(fg).as_bool()
                || matches!(cls.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd" | "FrostWidget" | "FrostHost" | "FrostPet")
            {
                (HWND::default(), RECT::default(), RECT::default(), false)
            } else {
                let mut r = RECT::default();
                let _ = GetWindowRect(fg, &mut r);
                let (mon, _) = monitor_rect(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST));
                (fg, mon, r, IsZoomed(fg).as_bool())
            }
        };
        for i in 0..self.ws.len() {
            if !self.ws[i].alive() {
                continue;
            }
            let sig = widgets::signature(self.ws[i].kind, &self.hub);
            let w = &self.ws[i];
            if !force && !w.dirty && sig == w.sig {
                continue;
            }
            // The floating island only gives way to full-screen apps, not maximized ones.
            let hidden = if island_top(i) {
                (i == MEDIA && self.tb_idle()) || self.fullscreen(w.mon, quns, fg, fg_mon, fg_rect)
            } else {
                self.covered(w.mon, fg, fg_mon, fg_rect, fg_max)
            };
            if paused || exclusive || hidden {
                self.ws[i].dirty = true;
                continue;
            }
            self.draw(i);
            self.ws[i].sig = sig;
            self.ws[i].dirty = false;
        }
        // Floating island: hide while an app is full screen on its monitor.
        if island_top(MEDIA) && self.ws[MEDIA].alive() {
            let show = !self.tb_idle() && !self.fullscreen(self.ws[MEDIA].mon, quns, fg, fg_mon, fg_rect);
            if show != self.island_shown {
                self.island_shown = show;
                if show {
                    // Coming back (music started): settle on the right spot first.
                    self.tb_follow(false);
                }
                unsafe {
                    let h = self.ws[MEDIA].hwnd;
                    if show {
                        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                        if self.hub.taskbar {
                            self.tb_raise();
                        } else {
                            let _ = SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                        }
                    } else {
                        let _ = ShowWindow(h, SW_HIDE);
                    }
                }
                if show && self.ws[MEDIA].dirty {
                    self.draw(MEDIA);
                    self.ws[MEDIA].sig = widgets::signature(Kind::Media, &self.hub);
                    self.ws[MEDIA].dirty = false;
                }
            }
        }
        // Desktop pet: steps aside while an app is full screen on its monitor.
        if let Some(pw) = self.dpet.as_ref() {
            let show = !self.fullscreen(pw.mon, quns, fg, fg_mon, fg_rect);
            if show != pw.shown {
                let h = pw.hwnd;
                unsafe {
                    if show {
                        let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                        let _ = SetWindowPos(h, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                    } else {
                        let _ = ShowWindow(h, SW_HIDE);
                    }
                }
                if let Some(pw) = self.dpet.as_mut() {
                    pw.shown = show;
                }
                self.pet_schedule();
            }
        }
        // Messages are read only while that widget exists and the screen is in use.
        self.msgs.set_active(self.ws[MESSAGES].alive() && !paused);
        // Visualizer runs only while music plays and its widget is actually visible.
        let mw = &self.ws[MEDIA];
        // The taskbar strip stays visible over maximized apps; only full screen hides it.
        let hidden = if self.hub.taskbar {
            self.fullscreen(mw.mon, quns, fg, fg_mon, fg_rect)
        } else {
            self.covered(mw.mon, fg, fg_mon, fg_rect, fg_max)
        };
        let want = self.hub.bars_on() && mw.alive() && self.hub.media.has && self.hub.media.playing && !paused && !exclusive && !hidden;
        if want != self.vis.is_active() {
            self.vis.set_active(want);
            if !want {
                self.ws[MEDIA].cache = Vec::new();
            }
        }
    }

    /// New visualizer levels: repaint just the bar strip of the now-playing widget.
    fn on_vis(&mut self) {
        if let Ok(g) = self.vis.levels.lock() {
            self.hub.vis = *g;
        }
        let w = &self.ws[MEDIA];
        // While the island morphs, animation frames redraw everything anyway.
        if !w.alive() || w.dirty || !self.hub.bars_on() {
            return;
        }
        let (ww, wh, m) = w.dims();
        if w.cache.len() != (ww * wh) as usize || w.surf.is_none() {
            self.draw(MEDIA);
            return;
        }
        let pal = Pal::new(w.dark_text, self.accent);
        let w = &mut self.ws[MEDIA];
        let s = w.s;
        let (bx, by, bw, bh) = widgets::media_bars_rect(&self.hub);
        let x0 = ((m as f32 + bx * s).floor() as i32).clamp(0, ww);
        let y0 = ((m as f32 + by * s).floor() as i32).clamp(0, wh);
        let x1 = ((m as f32 + (bx + bw) * s).ceil() as i32).clamp(0, ww);
        let y1 = ((m as f32 + (by + bh) * s).ceil() as i32).clamp(0, wh);
        let Some(surf) = w.surf.as_mut() else { return };
        {
            let px = surf.pixels();
            for y in y0..y1 {
                let a = (y * ww + x0) as usize;
                let b = (y * ww + x1) as usize;
                px[a..b].copy_from_slice(&w.cache[a..b]);
            }
        }
        unsafe {
            if self.gfx.rt.BindDC(surf.dc, &RECT { left: 0, top: 0, right: ww, bottom: wh }).is_err() {
                return;
            }
            self.gfx.rt.BeginDraw();
            self.gfx.rt.SetTransform(&Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: m as f32, M32: m as f32 });
            widgets::media_bars(&mut self.gfx, &self.hub, &pal);
            let _ = self.gfx.rt.EndDraw(None, None);
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let size = SIZE { cx: ww, cy: wh };
            let src = POINT { x: 0, y: 0 };
            let dirty = RECT { left: x0, top: y0, right: x1, bottom: y1 };
            let info = UPDATELAYEREDWINDOWINFO {
                cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
                hdcDst: self.screen,
                pptDst: std::ptr::null(),
                psize: &size,
                hdcSrc: surf.dc,
                pptSrc: &src,
                crKey: COLORREF(0),
                pblend: &blend,
                dwFlags: ULW_ALPHA,
                prcDirty: &dirty,
            };
            let _ = UpdateLayeredWindowIndirect(w.hwnd, &info);
        }
    }

    // ---------------------------------------------------------- widget mouse

    fn client_logical(&self, i: usize, cx: i32, cy: i32) -> (f32, f32) {
        let w = &self.ws[i];
        let (_, _, m) = w.dims();
        ((cx - m) as f32 / w.s, (cy - m) as f32 / w.s)
    }

    fn mouse_down(&mut self, i: usize, cx: i32, cy: i32) {
        let (x, y) = self.client_logical(i, cx, cy);
        if widgets::hit(self.ws[i].kind, x, y, &self.hub) == Some(widgets::HIT_SEEK) {
            self.seeking = Some(i);
            self.hub.seek = Some(widgets::seek_frac(x, &self.hub));
            unsafe {
                SetCapture(self.ws[i].hwnd);
            }
            self.refresh(false);
        }
    }

    fn mouse_move(&mut self, i: usize, cx: i32, cy: i32) {
        let (x, y) = self.client_logical(i, cx, cy);
        if self.seeking == Some(i) {
            self.hub.seek = Some(widgets::seek_frac(x, &self.hub));
            self.refresh(false);
            return;
        }
        if self.ws[i].kind != Kind::Media {
            return;
        }
        if !self.mouse_tracking {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.ws[i].hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            self.mouse_tracking = true;
        }
        if self.hub.taskbar {
            let hot = widgets::hit(Kind::Media, x, y, &self.hub).unwrap_or(9);
            if hot != self.hub.tb_hot {
                self.hub.tb_hot = hot;
                self.refresh(false);
            }
            return;
        }
        let over = widgets::hit(Kind::Media, x, y, &self.hub) == Some(widgets::HIT_SEEK);
        if over != self.hub.bar_hover {
            self.hub.bar_hover = over;
            self.refresh(false);
        }
    }

    fn mouse_up(&mut self, i: usize, cx: i32, cy: i32) {
        if self.seeking != Some(i) {
            return self.click(i, cx, cy);
        }
        let (x, _) = self.client_logical(i, cx, cy);
        let frac = widgets::seek_frac(x, &self.hub);
        self.seeking = None;
        self.hub.seek = None;
        unsafe {
            let _ = ReleaseCapture();
        }
        let target = frac as f64 * self.hub.media.dur;
        // Jump immediately; the player's own update follows.
        (self.hub.now, self.hub.now_ft) = local_now();
        self.hub.media.pos = target;
        self.hub.media.updated = self.hub.now_ft;
        self.media.cmd(media::Cmd::Seek(target));
        self.refresh(false);
    }

    fn cancel_seek(&mut self) {
        if self.seeking.take().is_some() {
            self.hub.seek = None;
            self.refresh(false);
        }
    }

    fn mouse_leave(&mut self) {
        self.mouse_tracking = false;
        if self.hub.bar_hover || self.hub.tb_hot != 0 {
            self.hub.bar_hover = false;
            self.hub.tb_hot = 0;
            self.refresh(false);
        }
    }

    /// After monitors change, bring back widgets that ended up off every screen.
    fn ensure_onscreen(&mut self) {
        let mut moved = false;
        for (i, k) in KINDS.iter().enumerate() {
            let p = self.cfg.pos(k.key());
            let (Some(x), Some(y)) = (p.x, p.y) else { continue };
            let (w, h) = k.size();
            let center = POINT { x: x + (w * 0.5) as i32, y: y + (h * 0.5) as i32 };
            let on = unsafe { !MonitorFromPoint(center, MONITOR_DEFAULTTONULL).is_invalid() };
            if !on {
                let p = self.cfg.pos_mut(k.key());
                p.x = None;
                p.y = None;
                moved = true;
                let _ = i;
            }
        }
        if let (Some(x), Some(y)) = (self.cfg.island_x, self.cfg.island_y) {
            let center = POINT { x: x + (widgets::ISL_CW * 0.5) as i32, y: y + 20 };
            if unsafe { MonitorFromPoint(center, MONITOR_DEFAULTTONULL).is_invalid() } {
                (self.cfg.island_x, self.cfg.island_y) = (None, None);
                moved = true;
            }
        }
        if moved {
            self.rebuild_all();
            self.save_cfg();
        }
    }

    fn visible(&self, k: Kind) -> bool {
        self.ws.iter().any(|w| w.kind == k && w.alive())
    }

    /// One timer drives clocks: aligned to the next second only when something
    /// shows seconds, otherwise to the next minute.
    fn schedule_tick(&mut self) {
        let paused = self.display_off || self.locked_session;
        let need_sec = !paused
            && ((self.visible(Kind::Analog) && self.cfg.analog_seconds)
                || (self.visible(Kind::Media) && self.hub.media.playing && self.hub.media.dur > 0.0));
        let (st, _) = local_now();
        let ms = st.wMilliseconds as u32;
        let delay = if need_sec { 1000 - ms + 8 } else { (60 - st.wSecond as u32) * 1000 - ms + 30 };
        unsafe {
            SetTimer(self.main, T_TICK, delay.max(10), None);
        }
    }

    fn on_tick(&mut self) {
        (self.hub.now, self.hub.now_ft) = local_now();
        self.refresh(false);
        self.schedule_tick();
    }

    fn on_stats(&mut self) {
        self.stats.sample();
        let s = &self.stats;
        (self.hub.cpu, self.hub.ram, self.hub.disk, self.hub.down, self.hub.up, self.hub.bat) =
            (s.cpu, s.ram, s.disk, s.down, s.up, s.bat);
        self.sync_gpu();
        self.hub.gpu = match self.gpu.as_mut() {
            Some(g) => Some(g.sample().unwrap_or(0.0)),
            None => None,
        };
        (self.hub.now, self.hub.now_ft) = local_now();
        self.refresh(false);
        if self.settings.as_ref().map(|s| s.tab == settings::TAB_ABOUT).unwrap_or(false) {
            self.mem_mb = private_mem_mb();
            self.settings_redraw();
        }
    }

    fn set_stats_timer(&self, on: bool) {
        unsafe {
            if on {
                SetCoalescableTimer(self.main, T_STATS, 2000, None, 500);
            } else {
                let _ = KillTimer(self.main, T_STATS);
            }
        }
    }

    fn start_weather(&mut self) {
        if self.weather_busy || self.cfg.city.trim().is_empty() {
            return;
        }
        self.weather_busy = true;
        let (city, imperial, geo, hwnd) = (self.cfg.city.clone(), self.hub.imperial, self.geo.clone(), self.main.0 as isize);
        std::thread::spawn(move || {
            let r = Box::new(sys::fetch_weather(&city, imperial, geo));
            unsafe {
                let _ = PostMessageW(HWND(hwnd as *mut _), WM_APP_WEATHER, WPARAM(Box::into_raw(r) as usize), LPARAM(0));
            }
        });
    }

    fn on_weather(&mut self, r: sys::WeatherResult) {
        self.weather_busy = false;
        // Drop results for a city that was changed while the request was running.
        if r.geo.as_ref().is_some_and(|g| g.query != self.cfg.city) {
            return;
        }
        if r.geo.is_some() {
            self.geo = r.geo;
        }
        let ok = r.weather.is_some();
        if ok {
            self.hub.weather = r.weather;
        }
        self.hub.weather_failed = !ok && self.hub.weather.is_none();
        self.hub.weather_msg = r.error;
        self.sync_cfg_to_hub();
        // Back off on failure: quick retries first (e.g. network not up yet at sign-in).
        self.weather_fails = if ok { 0 } else { self.weather_fails + 1 };
        let secs = match self.weather_fails {
            0 => 30 * 60,
            1..=3 => 60,
            _ => 5 * 60,
        };
        unsafe {
            SetTimer(self.main, T_WEATHER, secs * 1000, None);
        }
        self.refresh(false);
        self.settings_redraw();
    }

    fn on_media(&mut self) {
        let st = self.media.state.lock().map(|g| g.clone()).unwrap_or_default();
        self.hub.media = st;
        (self.hub.now, self.hub.now_ft) = local_now();
        self.island_track();
        self.refresh(false);
        self.schedule_tick();
    }

    fn apply_look(&mut self) {
        self.sync_cfg_to_hub();
        for w in &mut self.ws {
            w.bg_ok = false;
        }
        self.refresh(true);
        self.restyle_panels();
    }

    fn reload_wallpaper(&mut self) {
        self.walls.clear();
        self.apply_look();
        trim_memory();
    }

    // ---------------------------------------------------------- input

    fn to_logical(&self, i: usize, sx: i32, sy: i32) -> (f32, f32) {
        let w = &self.ws[i];
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(w.hwnd, &mut r);
        }
        let (_, _, m) = w.dims();
        ((sx - r.left - m) as f32 / w.s, (sy - r.top - m) as f32 / w.s)
    }

    fn hit_test(&self, i: usize, sx: i32, sy: i32) -> LRESULT {
        let (x, y) = self.to_logical(i, sx, sy);
        let pinned = i == MEDIA && self.hub.taskbar;
        if pinned || widgets::hit(self.ws[i].kind, x, y, &self.hub).is_some() || self.cfg.locked {
            LRESULT(HTCLIENT as isize)
        } else {
            LRESULT(HTCAPTION as isize)
        }
    }

    fn click(&mut self, i: usize, cx: i32, cy: i32) {
        let w = &self.ws[i];
        let (_, _, m) = w.dims();
        let (x, y) = ((cx - m) as f32 / w.s, (cy - m) as f32 / w.s);
        match widgets::hit(w.kind, x, y, &self.hub) {
            Some(1) => self.media.cmd(media::Cmd::Prev),
            Some(2) => {
                // Instant feedback; the real state follows from the media event.
                self.hub.media.playing = !self.hub.media.playing;
                self.refresh(false);
                self.media.cmd(media::Cmd::Toggle)
            }
            Some(3) => self.media.cmd(media::Cmd::Next),
            Some(widgets::HIT_ADD) => self.open_editor(),
            Some(widgets::HIT_PHOTO_NEXT) => self.next_photo(),
            Some(n) if n >= 100 => {
                let idx = (n - 100) as usize;
                if let Some(item) = self.hub.notes.get_mut(idx) {
                    item.0 = !item.0;
                    self.notes_changed();
                }
            }
            _ => {}
        }
    }

    fn begin_move(&mut self, i: usize) {
        self.moving = Some(i);
        self.drag_dirty = false;
        unsafe {
            SetTimer(self.main, T_DRAG, 33, None);
        }
    }

    /// Moves only flag the glass as stale; the low-priority drag timer repaints it,
    /// so the window itself always follows the cursor without waiting on us.
    fn on_move(&mut self, i: usize) {
        if self.moving == Some(i) {
            self.drag_dirty = true;
        }
    }

    fn drag_tick(&mut self) {
        let Some(i) = self.moving else {
            unsafe {
                let _ = KillTimer(self.main, T_DRAG);
            }
            return;
        };
        if !self.drag_dirty {
            return;
        }
        self.drag_dirty = false;
        self.update_pos(i);
        self.ws[i].bg_ok = false;
        self.draw(i);
    }

    fn update_pos(&mut self, i: usize) {
        let w = &mut self.ws[i];
        let mut r = RECT::default();
        unsafe {
            let _ = GetWindowRect(w.hwnd, &mut r);
        }
        let (_, _, m) = w.dims();
        w.gx = r.left + m;
        w.gy = r.top + m;
    }

    fn end_move(&mut self, i: usize) {
        self.moving = None;
        unsafe {
            let _ = KillTimer(self.main, T_DRAG);
        }
        self.update_pos(i);
        // Snap the glass card to an 8px grid once the drag ends.
        let w = &mut self.ws[i];
        let (_, _, m) = w.dims();
        let grid = (8.0 * w.s).round().max(1.0);
        w.gx = ((w.gx as f32 / grid).round() * grid) as i32;
        w.gy = ((w.gy as f32 / grid).round() * grid) as i32;
        unsafe {
            let _ = SetWindowPos(w.hwnd, None, w.gx - m, w.gy - m, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.ws[i].bg_ok = false;
        self.draw(i);
        let (gx, gy) = (self.ws[i].gx, self.ws[i].gy);
        if i == MEDIA && self.hub.island {
            (self.cfg.island_x, self.cfg.island_y) = (Some(gx), Some(gy));
        } else {
            let p = self.cfg.pos_mut(KINDS[i].key());
            p.x = Some(gx);
            p.y = Some(gy);
        }
        self.save_cfg();
    }

    // ---------------------------------------------------------- island

    /// Open or close the island to match what's wanted now (hovered, or within
    /// the few seconds after a song starts).
    fn island_update(&mut self) {
        // The taskbar pill opens on hover the same way, just into a smaller card.
        if !(self.hub.island || self.hub.taskbar) || !self.ws[MEDIA].alive() {
            return;
        }
        let now = std::time::Instant::now();
        let open = self.isl.hover || self.seeking.is_some() || self.isl.hold.is_some_and(|t| t > now);
        let target = if open { 1.0 } else { 0.0 };
        if target == self.isl.target {
            return;
        }
        self.isl.from = self.hub.isl_t;
        self.isl.target = target;
        self.isl.start = Some(now);
        unsafe {
            SetTimer(self.main, T_ANIM, 15, None);
        }
    }

    /// Keep the island open for `ms` from now.
    fn island_hold(&mut self, ms: u32) {
        let until = std::time::Instant::now() + std::time::Duration::from_millis(ms as u64);
        if self.isl.hold.is_none_or(|t| t < until) {
            self.isl.hold = Some(until);
            unsafe {
                SetTimer(self.main, T_HOLD, ms, None);
            }
        }
        self.island_update();
    }

    fn island_hold_done(&mut self) {
        unsafe {
            let _ = KillTimer(self.main, T_HOLD);
        }
        self.isl.hold = None;
        self.island_update();
    }

    /// One animation frame. Opening springs past full size and settles (the
    /// corners visibly reshape); closing eases back into the pill.
    fn island_frame(&mut self) {
        let Some(start) = self.isl.start else {
            unsafe {
                let _ = KillTimer(self.main, T_ANIM);
            }
            return;
        };
        let opening = self.isl.target > self.isl.from;
        let dur = if opening { 0.55 } else { 0.38 };
        let e = (start.elapsed().as_secs_f32() / dur).min(1.0);
        let p = if opening {
            // easeOutBack: a small overshoot, like a spring.
            let c = 1.5;
            1.0 + (c + 1.0) * (e - 1.0).powi(3) + c * (e - 1.0).powi(2)
        } else {
            e * e * (3.0 - 2.0 * e)
        };
        self.hub.isl_t = self.isl.from + (self.isl.target - self.isl.from) * p;
        if e >= 1.0 {
            self.hub.isl_t = self.isl.target;
            self.isl.start = None;
            unsafe {
                let _ = KillTimer(self.main, T_ANIM);
            }
        }
        self.refresh(false);
    }

    /// Pointer over the island: open it and watch for the pointer leaving.
    fn island_hover(&mut self) {
        if !(self.hub.island || self.hub.taskbar) || self.isl.hover {
            return;
        }
        // Taskbar style: only the pill itself opens it, not its shadow or the canvas around it.
        if self.hub.taskbar && !self.pointer_on_media(0.0) {
            return;
        }
        self.isl.hover = true;
        unsafe {
            SetTimer(self.main, T_HOVER, 100, None);
        }
        self.island_update();
    }

    /// Is the pointer on the island / taskbar pill as it is now (plus `pad` logical px)?
    fn pointer_on_media(&self, pad: f32) -> bool {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        self.ws[MEDIA].alive() && {
            let (x, y) = self.to_logical(MEDIA, pt.x, pt.y);
            let t = self.hub.isl_t.max(self.isl.target);
            let (sx, sy, sw, sh, _) = if self.hub.taskbar {
                widgets::tb_shape(t, self.hub.tb_right)
            } else {
                widgets::island_shape(t, self.hub.isl_cw)
            };
            x >= sx - pad && x <= sx + sw + pad && y >= sy - pad && y <= sy + sh + pad
        }
    }

    fn island_hover_check(&mut self) {
        let inside = self.pointer_on_media(4.0);
        if inside || self.seeking.is_some() || self.moving == Some(MEDIA) {
            return;
        }
        unsafe {
            let _ = KillTimer(self.main, T_HOVER);
        }
        self.isl.hover = false;
        self.hub.tb_hot = 0;
        // Close a moment after the pointer leaves, not instantly.
        self.island_hold(500);
    }

    /// Opens the island briefly when music starts or the song changes.
    fn island_track(&mut self) {
        let m = &self.hub.media;
        let now = (m.has && m.playing, m.title.clone());
        let started = now.0 && (!self.isl.last.0 || now.1 != self.isl.last.1);
        self.isl.last = now;
        if started && self.hub.island {
            self.island_hold(ISLAND_SHOW_MS);
        }
    }

    /// Re-create the now-playing window so it picks up its new layer (desktop or floating).
    fn restack_media(&mut self) {
        unsafe {
            let _ = KillTimer(self.main, T_FS);
        }
        self.island_shown = true;
        if self.ws[MEDIA].alive() {
            self.destroy_widget(MEDIA);
            self.create_widget(MEDIA, true);
        }
        self.refresh(false);
    }

    /// Switch now playing between the card (0), the island (1) and the taskbar strip (2).
    fn set_media_style(&mut self, style: u8) {
        if self.cfg.media_style_index() == style {
            return;
        }
        let island = style == 1;
        self.cfg.media_style = ["card", "island", "taskbar"][style.min(2) as usize].into();
        self.save_cfg();
        self.sync_cfg_to_hub();
        self.isl = IslandAnim { last: self.isl.last.clone(), ..Default::default() };
        self.hub.isl_t = 0.0;
        self.hub.tb_hot = 0;
        self.island_shown = true;
        unsafe {
            let _ = KillTimer(self.main, T_ANIM);
            let _ = KillTimer(self.main, T_HOVER);
            let _ = KillTimer(self.main, T_HOLD);
        }
        if self.ws[MEDIA].alive() {
            self.destroy_widget(MEDIA);
            self.create_widget(MEDIA, true);
            // Show off the new look once.
            if island && self.hub.media.has {
                self.island_hold(ISLAND_SHOW_MS);
            }
        }
        self.refresh(false);
    }

    // ---------------------------------------------------------- taskbar strip

    /// Taskbar style with nothing to show: the strip steps off the taskbar until
    /// something plays (the media watcher wakes us; no polling).
    fn tb_idle(&self) -> bool {
        self.hub.taskbar && !self.hub.media.has
    }

    /// Size multiplier on top of display scaling. The taskbar strip ignores the widget
    /// size setting: it always matches the taskbar.
    fn zoom(&self, i: usize) -> f32 {
        if i == MEDIA && self.hub.taskbar {
            1.0
        } else {
            self.cfg.scale
        }
    }

    /// Measure the taskbar's buttons (UI Automation; a few ms, done rarely).
    fn tb_measure(&mut self) {
        self.tb_probe = taskbar::probe();
        self.tb_centered = taskbar::centered(&self.tb_probe);
        self.tb_setting = taskbar::centered_setting();
        let now = std::time::Instant::now();
        // Found nothing (taskbar covered by a full-screen app, Explorer restarting):
        // don't trust that for the usual 30 s, try again in about 5.
        let blind = self.tb_probe.start_left.is_none() && self.tb_probe.tray_left.is_none();
        self.tb_probe_at = Some(if blind { now.checked_sub(std::time::Duration::from_secs(25)).unwrap_or(now) } else { now });
        // Beside the tray it grows leftwards, on the left it grows rightwards.
        if self.hub.tb_right == self.tb_centered {
            self.hub.tb_right = !self.tb_centered;
            // The clickable fill sits under the pill, which just changed sides.
            self.ws[MEDIA].bg_ok = false;
        }
    }

    /// Where the strip belongs right now, from the last measurement.
    fn tb_spot(&self) -> Option<(i32, i32)> {
        let bar = taskbar::bar_rect()?;
        let hmon = unsafe { MonitorFromWindow(taskbar::tray_hwnd(), MONITOR_DEFAULTTOPRIMARY) };
        let s = monitor_dpi(hmon) as f32 / 96.0;
        // Place the compact pill, then step back to the canvas corner around it.
        let (pw, ph) = widgets::TB_PILL;
        let (w, h) = ((pw * s).round() as i32, (ph * s).round() as i32);
        let (x, y) = taskbar::spot(&self.tb_probe, bar, self.tb_centered, w, h, (6.0 * s).round() as i32)?;
        let (px, py) = widgets::tb_pill_origin(!self.tb_centered);
        Some((x - (px * s).round() as i32, y - (py * s).round() as i32))
    }

    /// Keep the strip on its spot. Cheap every second; the buttons are re-measured
    /// every 30 s (tray icons come and go, the weather button changes width), when the
    /// taskbar changes size, or when asked.
    fn tb_follow(&mut self, remeasure: bool) {
        // Nothing to place while hidden; it re-measures when it comes back.
        if !self.hub.taskbar || !self.ws[MEDIA].alive() || self.tb_idle() {
            return;
        }
        let bar = taskbar::bar_rect().unwrap_or_default();
        let p = self.tb_probe.bar;
        let resized = (bar.right - bar.left, bar.bottom - bar.top) != (p.right - p.left, p.bottom - p.top);
        // Alignment flipped in Settings: let the icons finish sliding, then re-measure.
        if !remeasure && taskbar::centered_setting() != self.tb_setting {
            self.tb_setting = !self.tb_setting;
            self.tb_soon();
        }
        if remeasure || resized || self.tb_probe_at.is_none_or(|t| t.elapsed().as_secs() >= 30) {
            self.tb_measure();
        }
        let Some((x, y)) = self.tb_spot() else { return };
        let w = &mut self.ws[MEDIA];
        if (x, y) != (w.gx, w.gy) {
            (w.gx, w.gy) = (x, y);
            let (_, _, m) = w.dims();
            unsafe {
                let _ = SetWindowPos(w.hwnd, None, x - m, y - m, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }

    /// Keep the strip directly above the taskbar in z-order, and no higher: Start,
    /// search, the notification center and other shell flyouts stay on top of it.
    /// Does nothing (no window calls) when it's already above the taskbar.
    fn tb_raise(&self) {
        let w = &self.ws[MEDIA];
        if !self.hub.taskbar || !w.alive() || !unsafe { IsWindowVisible(w.hwnd) }.as_bool() {
            return;
        }
        let tray = taskbar::tray_hwnd();
        unsafe {
            if tray.0.is_null() {
                let _ = SetWindowPos(w.hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                return;
            }
            // Already somewhere above the taskbar? Walk up from it looking for us.
            let mut h = tray;
            for _ in 0..512 {
                h = GetWindow(h, GW_HWNDPREV).unwrap_or_default();
                if h.0.is_null() {
                    break;
                }
                if h == w.hwnd {
                    return;
                }
            }
            // Slot in right above the taskbar: below whatever sits on top of it.
            let above = GetWindow(tray, GW_HWNDPREV).unwrap_or_default();
            let after = if above.0.is_null() { HWND_TOPMOST } else { above };
            let _ = SetWindowPos(w.hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }

    /// Re-measure a moment after the taskbar may have changed (settings, Explorer
    /// restart, displays), once things have settled.
    fn tb_soon(&self) {
        if self.hub.taskbar {
            unsafe {
                SetTimer(self.main, T_TBAR, 700, None);
            }
        }
    }

    // ---------------------------------------------------------- gpu, photos, messages

    fn adapters(&mut self) -> &[gpu::Adapter] {
        self.gpus.get_or_insert_with(gpu::adapters)
    }

    /// The graphics card picked in settings, or the main one.
    fn gpu_adapter(&mut self) -> Option<gpu::Adapter> {
        let name = self.cfg.gpu.clone();
        let list = self.adapters();
        list.iter().find(|a| a.name == name).or(list.first()).cloned()
    }

    /// GPU counters are only open while the System widget shows the GPU ring.
    fn sync_gpu(&mut self) {
        let want = self.cfg.gpu_on && self.ws[SYSTEM].alive();
        if !want {
            self.gpu = None;
            return;
        }
        match self.gpu_adapter() {
            None => self.gpu = None,
            Some(a) if self.gpu.as_ref().map(|g| g.luid) != Some(a.luid) => self.gpu = gpu::Gpu::new(a.luid),
            _ => {}
        }
    }

    fn photo_dir(&self) -> PathBuf {
        let d = self.cfg.photo_dir.trim();
        if d.is_empty() {
            photos::default_dir()
        } else {
            PathBuf::from(d)
        }
    }

    fn set_photo_dir(&mut self, dir: PathBuf) {
        self.cfg.photo_dir = dir.display().to_string();
        self.save_cfg();
        self.next_photo();
        self.settings_redraw();
    }

    /// Photo size in pixels for the Photos widget at its current scale.
    fn photo_px(&self) -> (u32, u32) {
        let s = self.ws[PHOTOS].s;
        ((widgets::WIDE * s).round() as u32, (SMALL * s).round() as u32)
    }

    fn next_photo(&mut self) {
        if !self.ws[PHOTOS].alive() {
            return;
        }
        let (w, h) = self.photo_px();
        let dir = self.photo_dir();
        let main = self.main.0 as isize;
        self.photos.get_or_insert_with(|| photos::start(main, WM_APP_PHOTO)).next(&dir, w, h);
        self.schedule_photo();
    }

    /// Re-decode when the widget's pixel size no longer matches the photo.
    fn photo_check_size(&mut self) {
        let want = self.photo_px();
        if self.photo.as_ref().map(|p| (p.w, p.h)) != Some(want) {
            self.next_photo();
        }
    }

    fn schedule_photo(&mut self) {
        unsafe {
            if self.ws[PHOTOS].alive() {
                let ms = self.cfg.photo_every.clamp(1, 7 * 1440) * 60_000;
                SetCoalescableTimer(self.main, T_PHOTO, ms, None, 5000);
            } else {
                let _ = KillTimer(self.main, T_PHOTO);
            }
        }
    }

    fn on_photo(&mut self, r: photos::Loaded) {
        match r {
            photos::Loaded::Photo(p) => {
                self.photo = Some(p);
                self.photo_seq += 1;
                self.hub.photo_id = self.photo_seq;
                self.hub.photo_msg = "";
            }
            photos::Loaded::Empty(m) => {
                self.photo = None;
                self.hub.photo_id = 0;
                self.hub.photo_msg = m;
            }
        }
        self.refresh(false);
        trim_memory();
    }

    fn on_msgs(&mut self) {
        let st = self.msgs.state.lock().map(|g| g.clone()).unwrap_or_default();
        self.hub.msgs = st.list;
        self.hub.msg_status = st.status;
        (self.hub.now, self.hub.now_ft) = local_now();
        self.refresh(false);
    }

    fn on_dpi(&mut self, i: usize, dpi: u32) {
        let zoom = self.zoom(i);
        let w = &mut self.ws[i];
        w.s = dpi as f32 / 96.0 * zoom;
        let (_, _, m) = w.dims();
        let (hwnd, gx, gy) = (w.hwnd, w.gx, w.gy);
        w.bg_ok = false;
        unsafe {
            let _ = SetWindowPos(hwnd, None, gx - m, gy - m, 0, 0, SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.draw(i);
        if i == PHOTOS {
            self.photo_check_size();
        }
    }

    fn on_foreground(&mut self) {
        let fg = unsafe { GetForegroundWindow() };
        let cls = util::class_name(fg);
        if !matches!(cls.as_str(), "FrostHost" | "FrostWidget") {
            // "Show desktop" (Win+D) puts the desktop in front; follow it so widgets stay visible.
            let desk = matches!(cls.as_str(), "WorkerW" | "Progman");
            if island_top(MEDIA) && self.ws[MEDIA].alive() && self.island_shown {
                if self.hub.taskbar {
                    // Clicking the taskbar raises it over the strip: step back above it,
                    // but never above Start, search or the flyouts.
                    self.tb_raise();
                } else {
                    // Another always-on-top window may have pushed in front: stay on top.
                    unsafe {
                        let _ = SetWindowPos(self.ws[MEDIA].hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                    }
                }
            }
            if desk != DESK_FRONT.swap(desk, Ordering::Relaxed) {
                for (_, w) in self.ws.iter().enumerate().filter(|(i, w)| w.alive() && !island_top(*i)) {
                    unsafe {
                        let after = if desk { HWND_TOP } else { HWND_BOTTOM };
                        let _ = SetWindowPos(w.hwnd, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
                    }
                }
            }
        }
        self.refresh(false);
    }

    fn reload_files(&mut self) {
        let ct = config::mtime(&self.dir.join("config.toml"));
        if ct != self.cfg_time {
            self.cfg_time = ct;
            let new = config::load(&self.dir);
            if new != self.cfg {
                let weather_changed = new.city != self.cfg.city || new.units != self.cfg.units;
                self.cfg = new;
                self.sync_cfg_to_hub();
                self.walls.clear();
                self.rebuild_all();
                self.pet_sync();
                if weather_changed {
                    self.hub.weather = None;
                    self.start_weather();
                }
            }
        }
        let nt = config::mtime(&self.dir.join("notes.txt"));
        if nt != self.notes_time {
            self.notes_time = nt;
            self.hub.notes = config::load_notes(&self.dir);
            if let Some(e) = self.editor.as_mut() {
                e.editing = None;
            }
            self.editor_redraw();
            self.refresh(false);
        }
    }

    fn set_paused(&mut self) {
        let paused = self.display_off || self.locked_session;
        self.set_stats_timer(!paused);
        if !paused {
            self.on_stats();
        }
        self.schedule_tick();
        self.pet_schedule();
    }

    // ---------------------------------------------------------- desktop pet

    /// Screen px per sprite cell for the chosen size on this monitor.
    fn pet_cell(&self, hmon: HMONITOR) -> i32 {
        let i = desk_pet::option_index(&desk_pet::SIZES, &self.cfg.pet_size);
        (desk_pet::SIZES[i].2 * monitor_dpi(hmon) as f32 / 96.0).round().max(1.0) as i32
    }

    /// The pet walks along the top of the taskbar on the main screen.
    fn pet_bounds(&mut self) {
        let hmon = unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) };
        let (mon, work) = monitor_rect(hmon);
        let s = self.pet_cell(hmon);
        let Some(pw) = self.dpet.as_mut() else { return };
        pw.s = s;
        pw.mon = mon;
        pw.ground = work.bottom;
        pw.lo = work.left;
        pw.hi = (work.right - desk_pet::GW as i32 * s).max(work.left);
    }

    /// Match the pet to the settings: create, remove, or change its behavior.
    fn pet_sync(&mut self) {
        if !self.cfg.pet_on {
            if let Some(pw) = self.dpet.take() {
                unsafe {
                    let _ = KillTimer(self.main, T_PET);
                    let _ = DestroyWindow(pw.hwnd);
                }
            }
            return;
        }
        let mode = desk_pet::Mode::from_key(&self.cfg.pet_mode);
        match self.dpet.as_mut() {
            Some(pw) => pw.pet.set_mode(mode),
            None => self.pet_create(mode),
        }
        self.pet_bounds();
        self.pet_draw();
        self.pet_schedule();
    }

    fn pet_create(&mut self, mode: desk_pet::Mode) {
        unsafe {
            let Ok(hwnd) = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOACTIVATE,
                w!("FrostPet"),
                w!("Frost pet"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                self.hinst,
                None,
            ) else {
                return;
            };
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, PET_TAG);
            let seed = GetTickCount() ^ (hwnd.0 as usize as u32).rotate_left(16);
            self.dpet = Some(PetWin {
                hwnd,
                surf: None,
                s: 3,
                pet: desk_pet::Pet::new(mode, 0.0, seed),
                ground: 0,
                lo: 0,
                hi: 0,
                mon: RECT::default(),
                shown: true,
                timer: false,
                drag: None,
                buf: vec![0; desk_pet::GW * desk_pet::GH],
            });
            self.pet_bounds();
            if let Some(pw) = self.dpet.as_mut() {
                // Start on the right side of the taskbar, near the clock.
                pw.pet.x = pw.lo as f32 + (pw.hi - pw.lo) as f32 * 0.78;
            }
            self.pet_draw();
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            // Cheap once-a-second check for apps going full screen.
            SetCoalescableTimer(self.main, T_FS, 1000, None, 250);
        }
    }

    /// Render the current frame, scale it up and move the window to the pet.
    fn pet_draw(&mut self) {
        let Some(pw) = self.dpet.as_mut() else { return };
        let s = pw.s.max(1);
        let (ww, wh) = (desk_pet::GW as i32 * s, desk_pet::GH as i32 * s);
        if pw.surf.as_ref().map(|x| (x.w, x.h)) != Some((ww, wh)) {
            pw.surf = Surface::new(self.screen, ww, wh);
        }
        let Some(surf) = pw.surf.as_mut() else { return };
        pw.pet.render(&mut pw.buf);
        let (su, wu) = (s as usize, ww as usize);
        let px = surf.pixels();
        for gy in 0..desk_pet::GH {
            let src = &pw.buf[gy * desk_pet::GW..(gy + 1) * desk_pet::GW];
            let top = gy * su * wu;
            for (gx, v) in src.iter().enumerate() {
                px[top + gx * su..top + (gx + 1) * su].fill(*v);
            }
            for r in 1..su {
                px.copy_within(top..top + wu, top + r * wu);
            }
        }
        let x = pw.pet.x.round() as i32;
        let y = pw.ground - desk_pet::GROUND_ROWS * s - pw.pet.lift.round() as i32;
        unsafe {
            let blend = BLENDFUNCTION { BlendOp: AC_SRC_OVER as u8, BlendFlags: 0, SourceConstantAlpha: 255, AlphaFormat: AC_SRC_ALPHA as u8 };
            let _ = UpdateLayeredWindow(
                pw.hwnd,
                self.screen,
                Some(&POINT { x, y }),
                Some(&SIZE { cx: ww, cy: wh }),
                surf.dc,
                Some(&POINT { x: 0, y: 0 }),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
        }
    }

    fn pet_running(&self) -> bool {
        let paused = self.display_off || self.locked_session;
        self.dpet.as_ref().is_some_and(|pw| pw.shown && !paused && pw.pet.act != desk_pet::Act::Held)
    }

    /// Start or stop the animation when the pet is shown, hidden or paused.
    fn pet_schedule(&mut self) {
        let run = self.pet_running();
        let Some(pw) = self.dpet.as_mut() else { return };
        unsafe {
            if run && !pw.timer {
                SetTimer(self.main, T_PET, 60, None);
                pw.timer = true;
            } else if !run && pw.timer {
                let _ = KillTimer(self.main, T_PET);
                pw.timer = false;
            }
        }
    }

    fn pet_frame(&mut self) {
        let idle = unsafe {
            let mut li = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
            if GetLastInputInfo(&mut li).as_bool() {
                GetTickCount().wrapping_sub(li.dwTime) / 1000
            } else {
                0
            }
        };
        let hour = local_now().0.wHour;
        let speed = desk_pet::SPEEDS[desk_pet::option_index(&desk_pet::SPEEDS, &self.cfg.pet_speed)].2;
        let run = self.pet_running();
        let Some(pw) = self.dpet.as_mut() else {
            unsafe {
                let _ = KillTimer(self.main, T_PET);
            }
            return;
        };
        let ctx = desk_pet::Ctx { lo: pw.lo as f32, hi: pw.hi as f32, speed: speed * pw.s as f32, idle, hour };
        let ms = pw.pet.tick(&ctx);
        unsafe {
            if run && ms > 0 {
                SetTimer(self.main, T_PET, ms, None);
                pw.timer = true;
            } else {
                let _ = KillTimer(self.main, T_PET);
                pw.timer = false;
            }
        }
        self.pet_draw();
    }

    fn pet_press(&mut self) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        if let Some(pw) = self.dpet.as_mut() {
            pw.drag = Some((pt, pw.pet.x, pw.pet.lift, false));
        }
    }

    /// Carry the pet around; it dangles until you let go.
    fn pet_move(&mut self) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let Some(pw) = self.dpet.as_mut() else { return };
        let Some((p0, x0, lift0, moved)) = pw.drag else { return };
        let (dx, dy) = (pt.x - p0.x, pt.y - p0.y);
        if !moved {
            if dx.abs() + dy.abs() < 5 {
                return;
            }
            pw.drag = Some((p0, x0, lift0, true));
            pw.pet.grab();
        }
        let ww = desk_pet::GW as i32 * pw.s;
        pw.pet.x = (x0 + dx as f32).clamp(pw.mon.left as f32, (pw.mon.right - ww) as f32);
        let top_room = (pw.ground - pw.mon.top - desk_pet::GROUND_ROWS * pw.s).max(0) as f32;
        pw.pet.lift = (lift0 - dy as f32).clamp(0.0, top_room);
        self.pet_schedule();
        self.pet_draw();
    }

    /// Let go: a click makes it hop, a drop makes it fall back to the taskbar.
    fn pet_release(&mut self) {
        let Some(pw) = self.dpet.as_mut() else { return };
        let Some((_, _, _, moved)) = pw.drag.take() else { return };
        if moved {
            pw.pet.release();
        } else {
            pw.pet.poke();
        }
        unsafe {
            SetTimer(self.main, T_PET, 16, None);
        }
        pw.timer = true;
        self.pet_schedule();
    }

    fn pet_menu(&self) -> HMENU {
        unsafe {
            let menu = CreatePopupMenu().unwrap_or_default();
            let item = |m: HMENU, id: usize, text: &str, checked: bool| {
                let t = util::wide(text);
                let f = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
                let _ = AppendMenuW(m, f, id, PCWSTR(t.as_ptr()));
            };
            let sub = |m: HMENU, sm: HMENU, text: &str| {
                let t = util::wide(text);
                let _ = AppendMenuW(m, MF_POPUP, sm.0 as usize, PCWSTR(t.as_ptr()));
            };
            let mode = desk_pet::Mode::from_key(&self.cfg.pet_mode).index();
            let bm = CreatePopupMenu().unwrap_or_default();
            for (i, m) in desk_pet::MODES.iter().enumerate() {
                item(bm, 210 + i, m.2, i == mode);
            }
            sub(menu, bm, "Behavior");
            let speed = desk_pet::option_index(&desk_pet::SPEEDS, &self.cfg.pet_speed);
            let sm = CreatePopupMenu().unwrap_or_default();
            for (i, o) in desk_pet::SPEEDS.iter().enumerate() {
                item(sm, 220 + i, o.1, i == speed);
            }
            sub(menu, sm, "Walking speed");
            let size = desk_pet::option_index(&desk_pet::SIZES, &self.cfg.pet_size);
            let zm = CreatePopupMenu().unwrap_or_default();
            for (i, o) in desk_pet::SIZES.iter().enumerate() {
                item(zm, 230 + i, o.1, i == size);
            }
            sub(menu, zm, "Size");
            let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
            item(menu, 26, "Settings\u{2026}", false);
            item(menu, 200, "Hide pet", false);
            menu
        }
    }

    fn pet_command(&mut self, cmd: u32) {
        match cmd {
            200 => self.cfg.pet_on = !self.cfg.pet_on,
            210..=214 => self.cfg.pet_mode = desk_pet::MODES[(cmd - 210) as usize].1.into(),
            220..=222 => self.cfg.pet_speed = desk_pet::SPEEDS[(cmd - 220) as usize].0.into(),
            230..=232 => self.cfg.pet_size = desk_pet::SIZES[(cmd - 230) as usize].0.into(),
            _ => return,
        }
        self.save_cfg();
        self.pet_sync();
    }

    // ---------------------------------------------------------- tray & menu

    // ---------------------------------------------------------- reminders editor

    /// Persist reminders and update the widget and the editor.
    fn notes_changed(&mut self) {
        config::save_notes(&self.dir, &self.hub.notes);
        self.notes_time = config::mtime(&self.dir.join("notes.txt"));
        self.editor_redraw();
        self.refresh(false);
    }

    fn editor_redraw(&self) {
        if let Some(e) = &self.editor {
            unsafe {
                let _ = InvalidateRect(e.hwnd, None, false);
            }
        }
    }

    fn open_editor(&mut self) {
        unsafe {
            if let Some(e) = &self.editor {
                let _ = ShowWindow(e.hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(e.hwnd);
                let _ = SetFocus(e.edit);
                return;
            }
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
            let (_, work) = monitor_rect(hmon);
            let dpi = monitor_dpi(hmon);
            let s = dpi as f32 / 96.0;
            let style = (WS_OVERLAPPEDWINDOW & !WS_MAXIMIZEBOX) | WS_CLIPCHILDREN;
            let mut r = RECT { left: 0, top: 0, right: (editor::W0 * s) as i32, bottom: (editor::H0 * s) as i32 };
            let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE::default(), dpi);
            let (ww, wh) = (r.right - r.left, r.bottom - r.top);
            let x = work.left + ((work.right - work.left) - ww) / 2;
            let y = work.top + ((work.bottom - work.top) - wh) / 2;
            let Ok(hwnd) = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("FrostEditor"),
                w!("Reminders"),
                style,
                x,
                y,
                ww,
                wh,
                None,
                None,
                self.hinst,
                None,
            ) else {
                return;
            };
            let th = editor::theme(self.dark, self.accent);
            // Title bar that blends with the content (Windows 11; ignored elsewhere).
            let dark = BOOL::from(self.dark);
            let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), &dark as *const _ as *const _, 4);
            let cap = editor::colorref(th.bg);
            let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(35), &cap as *const _ as *const _, 4);
            let s = GetDpiForWindow(hwnd) as f32 / 96.0;

            let Ok(edit) = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                w!("EDIT"),
                w!(""),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
                0,
                0,
                10,
                10,
                hwnd,
                HMENU(1 as *mut _),
                self.hinst,
                None,
            ) else {
                let _ = DestroyWindow(hwnd);
                return;
            };
            let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_subclass as *const () as isize);
            OLD_EDIT_PROC.store(old, Ordering::Relaxed);
            let font = make_font(s);
            SendMessageW(edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            SendMessageW(edit, EM_SETMARGINS_, WPARAM(3), LPARAM(0));
            SendMessageW(edit, EM_LIMITTEXT_, WPARAM(300), LPARAM(0));
            let brush = CreateSolidBrush(editor::colorref(th.input));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, EDITOR_TAG);
            if !self.tray_icon.is_invalid() {
                SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(self.tray_icon.0 as isize));
                SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(self.tray_icon.0 as isize));
            }
            let e = editor::Editor {
                hwnd,
                edit,
                surf: None,
                s,
                hover: editor::Hit::None,
                editing: None,
                scroll: 0.0,
                font,
                brush,
                tracking: false,
            };
            e.place_edit();
            self.editor = Some(e);
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
            let _ = SetFocus(edit);
        }
    }

    fn editor_closed(&mut self, hwnd: HWND) {
        if self.editor.as_ref().map(|e| e.hwnd) == Some(hwnd) {
            if let Some(e) = self.editor.take() {
                unsafe {
                    let _ = DeleteObject(e.font);
                    let _ = DeleteObject(e.brush);
                }
            }
            trim_memory();
        }
    }

    fn editor_paint(&mut self) {
        let th = editor::theme(self.dark, self.accent);
        let Some(e) = self.editor.as_mut() else { return };
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(e.hwnd, &mut ps);
            e.paint(&mut self.gfx, self.screen, hdc, &self.hub.notes, &th);
            let _ = EndPaint(e.hwnd, &ps);
        }
    }

    fn editor_text(&self) -> String {
        let Some(e) = &self.editor else { return String::new() };
        unsafe {
            let n = GetWindowTextLengthW(e.edit).max(0) as usize;
            let mut buf = vec![0u16; n + 1];
            let got = GetWindowTextW(e.edit, &mut buf).max(0) as usize;
            String::from_utf16_lossy(&buf[..got]).trim().to_string()
        }
    }

    fn editor_set_text(&self, s: &str) {
        if let Some(e) = &self.editor {
            let w = util::wide(s);
            unsafe {
                let _ = SetWindowTextW(e.edit, PCWSTR(w.as_ptr()));
                SendMessageW(e.edit, EM_SETSEL_, WPARAM(0), LPARAM(-1));
                let _ = SetFocus(e.edit);
            }
        }
    }

    fn editor_submit(&mut self) {
        let text = self.editor_text().replace(['\r', '\n'], " ");
        let Some(e) = self.editor.as_mut() else { return };
        match e.editing.take() {
            Some(i) if i < self.hub.notes.len() => {
                if !text.is_empty() {
                    self.hub.notes[i].1 = text;
                }
            }
            _ => {
                if text.is_empty() {
                    return;
                }
                self.hub.notes.push((false, text));
                e.scroll = e.max_scroll(self.hub.notes.len());
            }
        }
        self.editor_set_text("");
        self.notes_changed();
    }

    /// Esc: cancel an edit, or close the window when there's nothing to cancel.
    fn editor_cancel(&mut self) {
        let empty = self.editor_text().is_empty();
        let Some(e) = self.editor.as_mut() else { return };
        if e.editing.take().is_some() || !empty {
            self.editor_set_text("");
            self.editor_redraw();
        } else {
            let hwnd = e.hwnd;
            self.editor_closed(hwnd);
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
        }
    }

    fn editor_mouse(&mut self, x: i32, y: i32, click: bool) {
        let Some(e) = self.editor.as_mut() else { return };
        let (lx, ly) = (x as f32 / e.s, y as f32 / e.s);
        let hit = e.hit(lx, ly, &self.hub.notes);
        if !e.tracking {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: e.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            e.tracking = true;
        }
        let hover_changed = hit != e.hover;
        e.hover = hit;
        if !click {
            if hover_changed {
                self.editor_redraw();
            }
            return;
        }
        use editor::Hit;
        let n = &mut self.hub.notes;
        let swap_edit = |ed: &mut Option<usize>, a: usize, b: usize| {
            if *ed == Some(a) {
                *ed = Some(b)
            } else if *ed == Some(b) {
                *ed = Some(a)
            }
        };
        let mut new_text: Option<String> = None;
        let mut changed = true;
        match hit {
            Hit::Add => return self.editor_submit(),
            Hit::None => changed = false,
            Hit::Toggle(i) => n[i].0 = !n[i].0,
            Hit::Edit(i) | Hit::Row(i) => {
                e.editing = Some(i);
                new_text = Some(n[i].1.clone());
                changed = false;
            }
            Hit::Up(i) => {
                n.swap(i, i - 1);
                swap_edit(&mut e.editing, i, i - 1);
            }
            Hit::Down(i) => {
                n.swap(i, i + 1);
                swap_edit(&mut e.editing, i, i + 1);
            }
            Hit::Delete(i) => {
                n.remove(i);
                match e.editing {
                    Some(j) if j == i => {
                        e.editing = None;
                        new_text = Some(String::new());
                    }
                    Some(j) if j > i => e.editing = Some(j - 1),
                    _ => {}
                }
            }
            Hit::Clear => {
                n.retain(|x| !x.0);
                e.editing = None;
                new_text = Some(String::new());
            }
        }
        if let Some(t) = new_text {
            self.editor_set_text(&t);
        }
        if !changed {
            self.editor_redraw();
            return;
        }
        if let Some(e) = self.editor.as_mut() {
            e.scroll = e.scroll.min(e.max_scroll(self.hub.notes.len()));
            e.hover = e.hit(lx, ly, &self.hub.notes);
        }
        self.notes_changed();
    }

    fn editor_wheel(&mut self, delta: i16) {
        let Some(e) = self.editor.as_mut() else { return };
        let max = e.max_scroll(self.hub.notes.len());
        e.scroll = (e.scroll - delta as f32 / 120.0 * 44.0 * 1.5).clamp(0.0, max);
        self.editor_redraw();
    }

    fn editor_resized(&mut self) {
        if let Some(e) = self.editor.as_mut() {
            e.place_edit();
            e.scroll = e.scroll.min(e.max_scroll(self.hub.notes.len()));
        }
        self.editor_redraw();
    }

    fn editor_dpi(&mut self, dpi: u32, suggested: &RECT) {
        let Some(e) = self.editor.as_mut() else { return };
        e.s = dpi as f32 / 96.0;
        unsafe {
            let font = make_font(e.s);
            SendMessageW(e.edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            let _ = DeleteObject(e.font);
            e.font = font;
            let _ = SetWindowPos(
                e.hwnd,
                None,
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.editor_resized();
    }

    // ---------------------------------------------------------- settings panel

    fn settings_redraw(&self) {
        if let Some(s) = &self.settings {
            unsafe {
                let _ = InvalidateRect(s.hwnd, None, false);
            }
        }
    }

    fn open_settings(&mut self) {
        unsafe {
            if let Some(s) = &self.settings {
                let _ = ShowWindow(s.hwnd, SW_RESTORE);
                let _ = SetForegroundWindow(s.hwnd);
                return;
            }
            let th = editor::theme(self.dark, self.accent);
            let Some((hwnd, s)) = create_panel(self.hinst, w!("FrostSettings"), w!("Frost Widgets Settings"), settings::W0, settings::H0, self.dark, th.bg)
            else {
                return;
            };
            let Some((edit, font)) = make_edit(hwnd, self.hinst, s) else {
                let _ = DestroyWindow(hwnd);
                return;
            };
            let city = util::wide(&self.cfg.city);
            let _ = SetWindowTextW(edit, PCWSTR(city.as_ptr()));
            let _ = ShowWindow(edit, SW_HIDE);
            let brush = CreateSolidBrush(editor::colorref(th.bg));
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, SETTINGS_TAG);
            self.set_window_icons(hwnd);
            if self.big_icon.is_invalid() {
                self.big_icon = load_app_icon(self.hinst, (64.0 * s) as i32);
            }
            self.mem_mb = private_mem_mb();
            self.settings = Some(settings::Settings::new(hwnd, edit, s, font, brush, self.cfg.glass));
            let _ = ShowWindow(hwnd, SW_SHOW);
            let _ = SetForegroundWindow(hwnd);
        }
    }

    fn set_window_icons(&self, hwnd: HWND) {
        unsafe {
            let small = load_app_icon(self.hinst, GetSystemMetrics(SM_CXSMICON));
            let big = load_app_icon(self.hinst, GetSystemMetrics(SM_CXICON));
            SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_SMALL as usize), LPARAM(small.0 as isize));
            SendMessageW(hwnd, WM_SETICON, WPARAM(ICON_BIG as usize), LPARAM(big.0 as isize));
        }
    }

    fn settings_closed(&mut self, hwnd: HWND) {
        if self.settings.as_ref().map(|s| s.hwnd) == Some(hwnd) {
            if let Some(s) = self.settings.take() {
                unsafe {
                    let _ = DeleteObject(s.font);
                    let _ = DeleteObject(s.brush);
                }
            }
            if !self.big_icon.is_invalid() {
                unsafe {
                    let _ = DestroyIcon(self.big_icon);
                }
                self.big_icon = HICON::default();
            }
            trim_memory();
        }
    }

    fn settings_paint(&mut self) {
        let th = editor::theme(self.dark, self.accent);
        let weather = match &self.hub.weather {
            Some(w) => format!("Now: {}, {}\u{B0} and {}", w.place, w.temp.round() as i32, sys::condition(w.code).to_lowercase()),
            None if self.hub.weather_failed && self.hub.weather_msg == "City not found" => format!("Couldn't find \"{}\". Check the spelling.", self.cfg.city),
            None if self.hub.weather_failed => "Couldn't reach the weather service".into(),
            None => "Loading\u{2026}".into(),
        };
        let mut on = [false; KINDS.len()];
        for (i, w) in self.ws.iter().enumerate() {
            on[i] = w.alive();
        }
        let gpu_name = self.gpu_adapter().map(|a| a.name).unwrap_or_default();
        let gpu_count = self.adapters().len();
        let photo_dir = self.photo_dir().display().to_string();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.display().to_string()))
            .unwrap_or_default();
        let view = settings::View {
            cfg: &self.cfg,
            autostart: util::reg_string(HKEY_CURRENT_USER, RUN_KEY, APP_NAME).is_some(),
            widgets_on: on,
            weather,
            mem_mb: self.mem_mb,
            dir: self.dir.display().to_string(),
            exe_dir,
            icon: self.big_icon,
            gpu_name,
            gpu_count,
            photo_dir,
        };
        let Some(st) = self.settings.as_mut() else { return };
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(st.hwnd, &mut ps);
            st.paint(&mut self.gfx, self.screen, hdc, &view, &th);
            let _ = EndPaint(st.hwnd, &ps);
        }
    }

    /// ev: 0 = move, 1 = button down, 2 = button up
    fn settings_mouse(&mut self, x: i32, y: i32, ev: u8) {
        let Some(st) = self.settings.as_mut() else { return };
        let (lx, ly) = (x as f32 / st.s, y as f32 / st.s);
        if st.dragging {
            st.glass_preview = st.glass_at(lx);
            if ev == 2 {
                st.dragging = false;
                let g = st.glass_preview;
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.cfg.glass = g;
                self.save_cfg();
                self.apply_look();
            }
            self.settings_redraw();
            return;
        }
        if !st.tracking {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: st.hwnd,
                dwHoverTime: 0,
            };
            unsafe {
                let _ = TrackMouseEvent(&mut tme);
            }
            st.tracking = true;
        }
        let hit = st.hit(lx, ly);
        let changed = hit != st.hover;
        st.hover = hit;
        if ev == 1 {
            match hit {
                Some(settings::Act::Glass) => {
                    st.dragging = true;
                    st.glass_preview = st.glass_at(lx);
                    unsafe {
                        SetCapture(st.hwnd);
                    }
                }
                Some(a) => self.settings_act(a),
                None => {}
            }
            self.settings_redraw();
        } else if changed {
            self.settings_redraw();
        }
    }

    /// Outline color from the settings (white when it can't be read).
    fn outline_rgb(&self) -> u32 {
        gfx::Color::parse(&self.cfg.island_outline_color).map(|c| c.rgb()).unwrap_or(0xFFFFFF)
    }

    fn set_outline_color(&mut self, rgb: u32) {
        self.cfg.island_outline_color = format!("#{rgb:06X}");
        // Picking a color means you want to see it.
        self.cfg.island_outline = true;
        self.outline_changed();
        self.settings_redraw();
    }

    fn outline_changed(&mut self) {
        self.save_cfg();
        self.sync_cfg_to_hub();
        self.refresh(true);
    }

    fn settings_act(&mut self, a: settings::Act) {
        use settings::Act;
        match a {
            Act::Tab(t) => {
                if let Some(st) = self.settings.as_mut() {
                    st.tab = t;
                }
                if t == settings::TAB_ABOUT {
                    self.mem_mb = private_mem_mb();
                }
            }
            Act::Autostart => self.command(22, usize::MAX),
            Act::Locked => self.command(20, usize::MAX),
            Act::EditReminders => self.open_editor(),
            Act::ResetLayout => self.command(21, usize::MAX),
            Act::OpenFolder => {
                let _ = std::process::Command::new("explorer.exe").arg(&self.dir).spawn();
            }
            Act::Theme(t) => self.command(30 + t as u32, usize::MAX),
            Act::Accent(hex) => {
                self.cfg.accent = format!("#{hex:06X}");
                self.save_cfg();
                self.apply_look();
            }
            Act::Glass => {}
            Act::Scale(i) => self.set_scale(settings::SCALES[i as usize].0),
            Act::Widget(i) => self.command(i as u32 + 1, usize::MAX),
            Act::SaveCity => {
                let city = self
                    .settings
                    .as_ref()
                    .map(|s| unsafe {
                        let mut buf = [0u16; 256];
                        let n = GetWindowTextW(s.edit, &mut buf).max(0) as usize;
                        String::from_utf16_lossy(&buf[..n]).trim().to_string()
                    })
                    .unwrap_or_default();
                if !city.is_empty() && city != self.cfg.city {
                    self.cfg.city = city;
                    self.save_cfg();
                    self.reset_weather();
                }
            }
            Act::Units(f) => {
                self.cfg.units = if f { "imperial" } else { "metric" }.into();
                self.save_cfg();
                self.reset_weather();
            }
            Act::H24 => {
                self.cfg.clock_24h = !self.cfg.clock_24h;
                self.save_cfg();
                self.sync_cfg_to_hub();
                self.refresh(true);
            }
            Act::Seconds => {
                self.cfg.analog_seconds = !self.cfg.analog_seconds;
                self.save_cfg();
                self.sync_cfg_to_hub();
                self.refresh(true);
                self.schedule_tick();
            }
            Act::Monday(m) => {
                self.cfg.week_start = if m { "monday" } else { "sunday" }.into();
                self.save_cfg();
                self.sync_cfg_to_hub();
                self.refresh(true);
            }
            Act::RefreshWeather => self.command(25, usize::MAX),
            Act::MediaStyle(style) => self.set_media_style(style),
            Act::TaskbarVis => {
                self.cfg.taskbar_vis = !self.cfg.taskbar_vis;
                self.save_cfg();
                self.sync_cfg_to_hub();
                self.refresh(true);
            }
            Act::GpuOn => {
                self.cfg.gpu_on = !self.cfg.gpu_on;
                self.save_cfg();
                self.on_stats();
            }
            Act::GpuNext => {
                let list = self.adapters().to_vec();
                if list.len() > 1 {
                    let cur = list.iter().position(|a| a.name == self.gpu_adapter().map(|a| a.name).unwrap_or_default()).unwrap_or(0);
                    self.cfg.gpu = list[(cur + 1) % list.len()].name.clone();
                    self.save_cfg();
                    self.on_stats();
                }
            }
            Act::PhotoDir => unsafe {
                let _ = PostMessageW(self.main, WM_APP_PICKDIR, WPARAM(0), LPARAM(0));
            },
            Act::PhotoEvery(m) => {
                self.cfg.photo_every = m;
                self.save_cfg();
                self.schedule_photo();
            }
            Act::IslandOutline => {
                self.cfg.island_outline = !self.cfg.island_outline;
                self.outline_changed();
            }
            Act::OutlineColor(hex) => self.set_outline_color(hex),
            Act::OutlineCustom => unsafe {
                let _ = PostMessageW(self.main, WM_APP_PICKCOLOR, WPARAM(0), LPARAM(0));
            },
            Act::IslandTop => {
                self.cfg.island_top = !self.cfg.island_top;
                self.save_cfg();
                self.sync_cfg_to_hub();
                self.restack_media();
            }
            Act::MsgPreview => {
                self.cfg.msg_preview = !self.cfg.msg_preview;
                self.save_cfg();
                self.sync_cfg_to_hub();
                self.refresh(false);
            }
            Act::Visualizer => {
                self.cfg.visualizer = !self.cfg.visualizer;
                self.save_cfg();
                self.sync_cfg_to_hub();
                self.refresh(true);
            }
            Act::PetOn => self.pet_command(200),
            Act::PetMode(i) => self.pet_command(210 + i as u32),
            Act::PetSpeed(i) => self.pet_command(220 + i as u32),
            Act::PetSize(i) => self.pet_command(230 + i as u32),
            Act::OpenApps => unsafe {
                ShellExecuteW(None, w!("open"), w!("ms-settings:appsfeatures"), None, None, SW_SHOWNORMAL);
            },
        }
        self.settings_redraw();
    }

    fn reset_weather(&mut self) {
        self.hub.weather = None;
        self.hub.weather_failed = false;
        self.sync_cfg_to_hub();
        self.weather_busy = false;
        self.start_weather();
        self.refresh(true);
    }

    /// Change widget size, keeping the arrangement (positions scale around the top-left widget).
    fn set_scale(&mut self, new: f32) {
        let old = self.cfg.scale.max(0.1);
        if (new - old).abs() < 0.001 {
            return;
        }
        let pts: Vec<(i32, i32)> = KINDS.iter().filter_map(|k| {
            let p = self.cfg.pos(k.key());
            Some((p.x?, p.y?))
        }).collect();
        if let (Some(ax), Some(ay)) = (pts.iter().map(|p| p.0).min(), pts.iter().map(|p| p.1).min()) {
            for k in KINDS {
                let p = self.cfg.pos_mut(k.key());
                if let (Some(x), Some(y)) = (p.x, p.y) {
                    p.x = Some(ax + ((x - ax) as f32 * new / old).round() as i32);
                    p.y = Some(ay + ((y - ay) as f32 * new / old).round() as i32);
                }
            }
        }
        self.cfg.scale = new;
        self.save_cfg();
        self.rebuild_all();
    }

    fn settings_dpi(&mut self, dpi: u32, r: &RECT) {
        let Some(st) = self.settings.as_mut() else { return };
        st.s = dpi as f32 / 96.0;
        unsafe {
            let font = make_font(st.s);
            SendMessageW(st.edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
            let _ = DeleteObject(st.font);
            st.font = font;
            let _ = SetWindowPos(st.hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        self.settings_redraw();
    }

    /// Re-color open panels after a theme or accent change.
    fn restyle_panels(&mut self) {
        let th = editor::theme(self.dark, self.accent);
        unsafe {
            if let Some(e) = self.editor.as_mut() {
                style_window(e.hwnd, self.dark, th.bg);
                let _ = DeleteObject(e.brush);
                e.brush = CreateSolidBrush(editor::colorref(th.input));
                let _ = InvalidateRect(e.hwnd, None, false);
                let _ = InvalidateRect(e.edit, None, true);
            }
            if let Some(s) = self.settings.as_mut() {
                style_window(s.hwnd, self.dark, th.bg);
                let _ = DeleteObject(s.brush);
                s.brush = CreateSolidBrush(editor::colorref(th.bg));
                let _ = InvalidateRect(s.hwnd, None, false);
                let _ = InvalidateRect(s.edit, None, true);
            }
        }
    }

    fn add_tray(&self) {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.main,
            uID: 1,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
            uCallbackMessage: WM_APP_TRAY,
            hIcon: self.tray_icon,
            ..Default::default()
        };
        for (d, s) in nid.szTip.iter_mut().zip("Frost Widgets".encode_utf16()) {
            *d = s;
        }
        unsafe {
            let _ = Shell_NotifyIconW(NIM_ADD, &nid);
        }
    }

    fn remove_tray(&self) {
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.main,
            uID: 1,
            ..Default::default()
        };
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
        }
    }

    fn build_menu(&self, idx: usize) -> HMENU {
        unsafe {
            let menu = CreatePopupMenu().unwrap_or_default();
            let item = |m: HMENU, id: usize, text: &str, checked: bool| {
                let t = util::wide(text);
                let f = if checked { MF_STRING | MF_CHECKED } else { MF_STRING };
                let _ = AppendMenuW(m, f, id, PCWSTR(t.as_ptr()));
            };
            let sep = |m: HMENU| {
                let _ = AppendMenuW(m, MF_SEPARATOR, 0, PCWSTR::null());
            };
            if idx < KINDS.len() {
                item(menu, 40, &format!("Hide {}", KINDS[idx].name().to_lowercase()), false);
                sep(menu);
            }
            item(menu, 26, "Settings\u{2026}", false);
            item(menu, 24, "Edit reminders\u{2026}", false);
            sep(menu);
            let wm = CreatePopupMenu().unwrap_or_default();
            for (i, k) in KINDS.iter().enumerate() {
                item(wm, 1 + i, k.name(), self.ws[i].alive());
            }
            let _ = AppendMenuW(menu, MF_POPUP, wm.0 as usize, w!("Widgets"));
            item(menu, 200, "Desktop pet", self.cfg.pet_on);
            item(menu, 20, "Lock positions", self.cfg.locked);
            sep(menu);
            item(menu, 99, "Exit Frost Widgets", false);
            menu
        }
    }

    fn command(&mut self, cmd: u32, idx: usize) {
        match cmd {
            1..=11 => {
                let i = cmd as usize - 1;
                let on = !self.ws[i].alive();
                self.cfg.pos_mut(KINDS[i].key()).enabled = on;
                if on {
                    self.create_widget(i, true);
                } else {
                    self.destroy_widget(i);
                }
                self.save_cfg();
                self.schedule_tick();
            }
            40 if idx < KINDS.len() => self.command(idx as u32 + 1, usize::MAX),
            200..=239 => self.pet_command(cmd),
            20 => {
                self.cfg.locked = !self.cfg.locked;
                self.save_cfg();
            }
            21 => {
                for k in KINDS {
                    let p = self.cfg.pos_mut(k.key());
                    p.x = None;
                    p.y = None;
                }
                (self.cfg.island_x, self.cfg.island_y) = (None, None);
                self.rebuild_all();
                self.save_cfg();
            }
            22 => {
                if util::reg_string(HKEY_CURRENT_USER, RUN_KEY, APP_NAME).is_some() {
                    util::reg_delete(HKEY_CURRENT_USER, RUN_KEY, APP_NAME);
                } else if let Ok(exe) = std::env::current_exe() {
                    util::reg_set_string(HKEY_CURRENT_USER, RUN_KEY, APP_NAME, &format!("\"{}\"", exe.display()));
                }
            }
            23 => {
                let _ = std::process::Command::new("notepad.exe").arg(self.dir.join("config.toml")).spawn();
            }
            24 => self.open_editor(),
            26 => self.open_settings(),
            25 => {
                self.hub.weather_failed = false;
                self.start_weather();
            }
            30..=32 => {
                self.cfg.theme = ["auto", "light", "dark"][cmd as usize - 30].into();
                self.save_cfg();
                self.apply_look();
            }
            99 => {
                self.remove_tray();
                unsafe { PostQuitMessage(0) };
            }
            _ => {}
        }
    }
}

fn trim_memory() {
    unsafe {
        let _ = K32EmptyWorkingSet(GetCurrentProcess());
    }
}

// ---------------------------------------------------------------- window procs

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let main = main_hwnd();
    if hwnd == main {
        if let Some(r) = main_proc(hwnd, msg, wp, lp) {
            return r;
        }
    } else {
        let idx = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
        if idx == SETTINGS_TAG {
            if let Some(r) = settings_proc(hwnd, msg, wp, lp) {
                return r;
            }
        } else if idx == EDITOR_TAG {
            if let Some(r) = editor_proc(hwnd, msg, wp, lp) {
                return r;
            }
        } else if idx == PET_TAG {
            if let Some(r) = pet_proc(hwnd, msg, wp, lp) {
                return r;
            }
        } else if idx > 0 {
            if let Some(r) = widget_proc(hwnd, idx as usize - 1, msg, wp, lp) {
                return r;
            }
        }
    }
    DefWindowProcW(hwnd, msg, wp, lp)
}

/// Folder picker. Returns None when cancelled.
unsafe fn pick_folder(owner: HWND, start: &std::path::Path) -> Option<PathBuf> {
    let dlg: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
    let opts = dlg.GetOptions().ok()?;
    let _ = dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
    let _ = dlg.SetTitle(w!("Choose a photos folder"));
    if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start.as_os_str()), None) {
        let _ = dlg.SetFolder(&item);
    }
    dlg.Show(owner).ok()?;
    let p = dlg.GetResult().ok()?.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as *const _));
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// Windows color picker, starting on `rgb` (0xRRGGBB). Remembers its custom colors
/// for the session.
unsafe fn pick_color(owner: HWND, rgb: u32) -> Option<u32> {
    use windows::Win32::UI::Controls::Dialogs::{ChooseColorW, CC_ANYCOLOR, CC_FULLOPEN, CC_RGBINIT, CHOOSECOLORW};
    static mut CUSTOM: [COLORREF; 16] = [COLORREF(0x00FF_FFFF); 16];
    // COLORREF is 0x00BBGGRR.
    let swap = |c: u32| ((c & 0xFF) << 16) | (c & 0xFF00) | ((c >> 16) & 0xFF);
    let mut cc = CHOOSECOLORW {
        lStructSize: std::mem::size_of::<CHOOSECOLORW>() as u32,
        hwndOwner: owner,
        rgbResult: COLORREF(swap(rgb)),
        lpCustColors: std::ptr::addr_of_mut!(CUSTOM) as *mut COLORREF,
        Flags: CC_RGBINIT | CC_FULLOPEN | CC_ANYCOLOR,
        ..Default::default()
    };
    ChooseColorW(&mut cc).as_bool().then(|| swap(cc.rgbResult.0))
}

/// The desktop window (Progman). Windows it owns stay visible on "Show desktop".
fn desktop_owner() -> HWND {
    unsafe { FindWindowW(w!("Progman"), None).unwrap_or_default() }
}

fn xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe fn widget_proc(hwnd: HWND, i: usize, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
        WM_WINDOWPOSCHANGING => {
            // Keep widgets on the desktop layer, behind normal windows.
            // Only touch z-order changes; plain moves (dragging) pass straight through.
            let wpos = &mut *(lp.0 as *mut WINDOWPOS);
            if !DESK_FRONT.load(Ordering::Relaxed) && !island_top(i) && (wpos.flags & SWP_NOZORDER).0 == 0 {
                wpos.hwndInsertAfter = HWND_BOTTOM;
            }
            None
        }
        WM_NCHITTEST => {
            let (x, y) = xy(lp);
            with_app(|a| a.hit_test(i, x, y))
        }
        WM_LBUTTONDOWN => {
            let (x, y) = xy(lp);
            with_app(|a| a.mouse_down(i, x, y)).map(|_| LRESULT(0))
        }
        WM_MOUSEMOVE => {
            let (x, y) = xy(lp);
            with_app(|a| {
                if i == MEDIA {
                    a.island_hover();
                }
                a.mouse_move(i, x, y)
            })
            .map(|_| LRESULT(0))
        }
        WM_NCMOUSEMOVE if i == MEDIA => {
            with_app(|a| a.island_hover());
            None
        }
        WM_LBUTTONUP => {
            let (x, y) = xy(lp);
            with_app(|a| a.mouse_up(i, x, y)).map(|_| LRESULT(0))
        }
        WM_CAPTURECHANGED => with_app(|a| a.cancel_seek()).map(|_| LRESULT(0)),
        WM_MOUSELEAVE_ => with_app(|a| a.mouse_leave()).map(|_| LRESULT(0)),
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            let mut pt = POINT::default();
            let _ = GetCursorPos(&mut pt);
            let hand = with_app(|a| {
                let (x, y) = a.to_logical(i, pt.x, pt.y);
                widgets::hit(a.ws[i].kind, x, y, &a.hub).is_some()
            })
            .unwrap_or(false);
            if hand {
                SetCursor(LoadCursorW(None, IDC_HAND).unwrap_or_default());
                Some(LRESULT(1))
            } else {
                None
            }
        }
        WM_CONTEXTMENU => {
            let _ = PostMessageW(main_hwnd(), WM_APP_MENU, WPARAM(i), LPARAM(0));
            Some(LRESULT(0))
        }
        WM_ENTERSIZEMOVE => with_app(|a| a.begin_move(i)).map(|_| LRESULT(0)),
        WM_MOVE => with_app(|a| a.on_move(i)).map(|_| LRESULT(0)),
        WM_EXITSIZEMOVE => with_app(|a| a.end_move(i)).map(|_| LRESULT(0)),
        WM_DPICHANGED => {
            let dpi = (wp.0 & 0xFFFF) as u32;
            with_app(|a| a.on_dpi(i, dpi)).map(|_| LRESULT(0))
        }
        WM_SYSCOMMAND if (wp.0 & 0xFFF0) as u32 == SC_KEYMENU => Some(LRESULT(0)),
        _ => {
            let _ = hwnd;
            None
        }
    }
}

const EDITOR_TAG: isize = 1000;
const SETTINGS_TAG: isize = 1001;
const PET_TAG: isize = 1002;
/// Menu index meaning "the pet's own menu".
const PET_MENU: usize = usize::MAX - 1;

unsafe fn pet_proc(hwnd: HWND, msg: u32, _wp: WPARAM, _lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_MOUSEACTIVATE => Some(LRESULT(MA_NOACTIVATE as isize)),
        WM_LBUTTONDOWN => {
            SetCapture(hwnd);
            with_app(|a| a.pet_press());
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            with_app(|a| a.pet_move());
            Some(LRESULT(0))
        }
        WM_LBUTTONUP => {
            with_app(|a| a.pet_release());
            let _ = ReleaseCapture();
            Some(LRESULT(0))
        }
        WM_CAPTURECHANGED => {
            with_app(|a| a.pet_release());
            Some(LRESULT(0))
        }
        WM_RBUTTONUP => {
            let _ = PostMessageW(main_hwnd(), WM_APP_MENU, WPARAM(PET_MENU), LPARAM(0));
            Some(LRESULT(0))
        }
        WM_SETCURSOR => {
            if let Ok(c) = LoadCursorW(None, IDC_HAND) {
                SetCursor(c);
            }
            Some(LRESULT(1))
        }
        _ => None,
    }
}
const WM_MOUSELEAVE_: u32 = 0x02A3;
const WM_APP_ED_SUBMIT: u32 = WM_APP + 10;
const WM_APP_ED_CANCEL: u32 = WM_APP + 11;
const EM_SETSEL_: u32 = 0x00B1;
const EM_SETMARGINS_: u32 = 0x00D3;
const EM_LIMITTEXT_: u32 = 0x00C5;
static OLD_EDIT_PROC: AtomicIsize = AtomicIsize::new(0);

unsafe fn make_font(s: f32) -> HFONT {
    CreateFontW(
        -(15.0 * s).round() as i32,
        0,
        0,
        0,
        400,
        0,
        0,
        0,
        DEFAULT_CHARSET.0 as u32,
        OUT_DEFAULT_PRECIS.0 as u32,
        CLIP_DEFAULT_PRECIS.0 as u32,
        CLEARTYPE_QUALITY.0 as u32,
        0,
        w!("Segoe UI"),
    )
}

/// Title bar that blends with the panel background (Windows 11; ignored elsewhere).
unsafe fn style_window(hwnd: HWND, dark: bool, bg: Color) {
    let d = BOOL::from(dark);
    let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), &d as *const _ as *const _, 4);
    let cap = editor::colorref(bg);
    let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(35), &cap as *const _ as *const _, 4);
}

/// A centered, resizable panel window sized in logical px. Returns it and its scale.
unsafe fn create_panel(hinst: HINSTANCE, class: PCWSTR, title: PCWSTR, w0: f32, h0: f32, dark: bool, bg: Color) -> Option<(HWND, f32)> {
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
    let (_, work) = monitor_rect(hmon);
    let dpi = monitor_dpi(hmon);
    let s = dpi as f32 / 96.0;
    let style = (WS_OVERLAPPEDWINDOW & !WS_MAXIMIZEBOX) | WS_CLIPCHILDREN;
    let mut r = RECT { left: 0, top: 0, right: (w0 * s) as i32, bottom: (h0 * s) as i32 };
    let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE::default(), dpi);
    let (ww, wh) = (r.right - r.left, r.bottom - r.top);
    let x = work.left + ((work.right - work.left) - ww).max(0) / 2;
    let y = work.top + ((work.bottom - work.top) - wh).max(0) / 2;
    let hwnd = CreateWindowExW(WINDOW_EX_STYLE::default(), class, title, style, x, y, ww, wh, None, None, hinst, None).ok()?;
    style_window(hwnd, dark, bg);
    Some((hwnd, GetDpiForWindow(hwnd) as f32 / 96.0))
}

/// Native single-line text box with our key handling and font.
unsafe fn make_edit(parent: HWND, hinst: HINSTANCE, s: f32) -> Option<(HWND, HFONT)> {
    let edit = CreateWindowExW(
        WINDOW_EX_STYLE::default(),
        w!("EDIT"),
        w!(""),
        WS_CHILD | WS_VISIBLE | WINDOW_STYLE(ES_AUTOHSCROLL as u32),
        0,
        0,
        10,
        10,
        parent,
        HMENU(1 as *mut _),
        hinst,
        None,
    )
    .ok()?;
    let old = SetWindowLongPtrW(edit, GWLP_WNDPROC, edit_subclass as *const () as isize);
    OLD_EDIT_PROC.store(old, Ordering::Relaxed);
    let font = make_font(s);
    SendMessageW(edit, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    SendMessageW(edit, EM_SETMARGINS_, WPARAM(3), LPARAM(0));
    SendMessageW(edit, EM_LIMITTEXT_, WPARAM(300), LPARAM(0));
    Some((edit, font))
}

/// The app icon embedded in the exe (resource 1), or a drawn fallback in dev builds.
unsafe fn load_app_icon(hinst: HINSTANCE, size: i32) -> HICON {
    let size = size.max(16);
    match LoadImageW(hinst, PCWSTR(1 as *const u16), IMAGE_ICON, size, size, LR_DEFAULTCOLOR) {
        Ok(h) if !h.is_invalid() => HICON(h.0),
        _ => gfx::make_icon(size),
    }
}

/// Private working set in MB (the "Memory" column in Task Manager).
fn private_mem_mb() -> f32 {
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_ws: usize,
        ws: usize,
        a: usize,
        b: usize,
        c: usize,
        d: usize,
        pagefile: usize,
        peak_pagefile: usize,
        private: usize,
        private_ws: usize,
        shared_commit: u64,
    }
    let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
    unsafe {
        let _ = windows::Win32::System::ProcessStatus::K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut c as *mut Counters as *mut _,
            c.cb,
        );
    }
    let bytes = if c.private_ws > 0 { c.private_ws } else { c.ws };
    bytes as f32 / (1024.0 * 1024.0)
}

unsafe fn settings_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_PAINT => with_app(|a| a.settings_paint()).map(|_| LRESULT(0)),
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_SIZE => with_app(|a| a.settings_redraw()).map(|_| LRESULT(0)),
        WM_MOUSEMOVE | WM_LBUTTONDOWN | WM_LBUTTONUP => {
            let (x, y) = xy(lp);
            let ev = match msg {
                WM_LBUTTONDOWN => 1,
                WM_LBUTTONUP => 2,
                _ => 0,
            };
            with_app(|a| a.settings_mouse(x, y, ev)).map(|_| LRESULT(0))
        }
        WM_MOUSELEAVE_ => with_app(|a| {
            if let Some(s) = a.settings.as_mut() {
                s.tracking = false;
                s.hover = None;
            }
            a.settings_redraw();
        })
        .map(|_| LRESULT(0)),
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            let hand = with_app(|a| a.settings.as_ref().map(|s| s.hover.is_some()).unwrap_or(false)).unwrap_or(false);
            if hand {
                SetCursor(LoadCursorW(None, IDC_HAND).unwrap_or_default());
                Some(LRESULT(1))
            } else {
                None
            }
        }
        WM_CTLCOLOREDIT => with_app(|a| {
            let th = editor::theme(a.dark, a.accent);
            let hdc = HDC(wp.0 as *mut _);
            SetTextColor(hdc, editor::colorref(th.fg));
            SetBkColor(hdc, editor::colorref(th.bg));
            LRESULT(a.settings.as_ref().map(|s| s.brush.0 as isize).unwrap_or(0))
        }),
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp.0 as *mut MINMAXINFO);
            let s = GetDpiForWindow(hwnd) as f32 / 96.0;
            mmi.ptMinTrackSize = POINT { x: (680.0 * s) as i32, y: (560.0 * s) as i32 };
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            let r = *(lp.0 as *const RECT);
            let dpi = (wp.0 & 0xFFFF) as u32;
            with_app(|a| a.settings_dpi(dpi, &r)).map(|_| LRESULT(0))
        }
        WM_CAPTURECHANGED => with_app(|a| {
            if let Some(s) = a.settings.as_mut() {
                if s.dragging {
                    s.dragging = false;
                    let g = s.glass_preview;
                    a.cfg.glass = g;
                    a.save_cfg();
                    a.apply_look();
                }
            }
        })
        .map(|_| LRESULT(0)),
        WM_APP_ED_SUBMIT => with_app(|a| a.settings_act(settings::Act::SaveCity)).map(|_| LRESULT(0)),
        WM_APP_ED_CANCEL => with_app(|a| {
            if let Some(s) = &a.settings {
                let c = util::wide(&a.cfg.city);
                let _ = SetWindowTextW(s.edit, PCWSTR(c.as_ptr()));
            }
        })
        .map(|_| LRESULT(0)),
        WM_DESTROY => {
            with_app(|a| a.settings_closed(hwnd));
            Some(LRESULT(0))
        }
        _ => None,
    }
}

/// Text box: Enter saves, Esc cancels, Ctrl+A selects all, placeholder when empty.
unsafe extern "system" fn edit_subclass(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let old: WNDPROC = std::mem::transmute(OLD_EDIT_PROC.load(Ordering::Relaxed));
    match msg {
        WM_KEYDOWN if wp.0 == 0x0D => {
            let _ = PostMessageW(GetParent(hwnd).unwrap_or_default(), WM_APP_ED_SUBMIT, WPARAM(0), LPARAM(0));
            return LRESULT(0);
        }
        WM_KEYDOWN if wp.0 == 0x1B => {
            let _ = PostMessageW(GetParent(hwnd).unwrap_or_default(), WM_APP_ED_CANCEL, WPARAM(0), LPARAM(0));
            return LRESULT(0);
        }
        WM_KEYDOWN if wp.0 == 'A' as usize && GetKeyState(0x11) < 0 => {
            SendMessageW(hwnd, EM_SETSEL_, WPARAM(0), LPARAM(-1));
            return LRESULT(0);
        }
        WM_CHAR if matches!(wp.0, 0x0D | 0x1B | 0x01) => return LRESULT(0),
        WM_PAINT => {
            let r = CallWindowProcW(old, hwnd, msg, wp, lp);
            if GetWindowTextLengthW(hwnd) == 0 {
                let dc = GetDC(hwnd);
                let font = HFONT(SendMessageW(hwnd, WM_GETFONT, WPARAM(0), LPARAM(0)).0 as *mut _);
                let prev = SelectObject(dc, font);
                SetBkMode(dc, TRANSPARENT);
                SetTextColor(dc, COLORREF(0x00909098));
                let parent = GetParent(hwnd).unwrap_or_default();
                let hint = if util::class_name(parent) == "FrostSettings" { "City name" } else { "Add a reminder\u{2026}" };
                let mut t: Vec<u16> = hint.encode_utf16().collect();
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                rc.left += 3;
                DrawTextW(dc, &mut t, &mut rc, DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_NOPREFIX);
                SelectObject(dc, prev);
                ReleaseDC(hwnd, dc);
            }
            return r;
        }
        _ => {}
    }
    CallWindowProcW(old, hwnd, msg, wp, lp)
}

unsafe fn editor_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_PAINT => with_app(|a| a.editor_paint()).map(|_| LRESULT(0)),
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_SIZE => with_app(|a| a.editor_resized()).map(|_| LRESULT(0)),
        WM_MOUSEMOVE => {
            let (x, y) = xy(lp);
            with_app(|a| a.editor_mouse(x, y, false)).map(|_| LRESULT(0))
        }
        WM_LBUTTONDOWN => {
            let (x, y) = xy(lp);
            with_app(|a| a.editor_mouse(x, y, true)).map(|_| LRESULT(0))
        }
        WM_MOUSELEAVE_ => with_app(|a| {
            if let Some(e) = a.editor.as_mut() {
                e.tracking = false;
                e.hover = editor::Hit::None;
            }
            a.editor_redraw();
        })
        .map(|_| LRESULT(0)),
        WM_MOUSEWHEEL => {
            let delta = ((wp.0 >> 16) & 0xFFFF) as i16;
            with_app(|a| a.editor_wheel(delta)).map(|_| LRESULT(0))
        }
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            let hand = with_app(|a| a.editor.as_ref().map(|e| e.hover.clickable()).unwrap_or(false)).unwrap_or(false);
            if hand {
                SetCursor(LoadCursorW(None, IDC_HAND).unwrap_or_default());
                Some(LRESULT(1))
            } else {
                None
            }
        }
        WM_CTLCOLOREDIT => with_app(|a| {
            let th = editor::theme(a.dark, a.accent);
            let hdc = HDC(wp.0 as *mut _);
            SetTextColor(hdc, editor::colorref(th.fg));
            SetBkColor(hdc, editor::colorref(th.input));
            LRESULT(a.editor.as_ref().map(|e| e.brush.0 as isize).unwrap_or(0))
        }),
        WM_GETMINMAXINFO => {
            let mmi = &mut *(lp.0 as *mut MINMAXINFO);
            let s = GetDpiForWindow(hwnd) as f32 / 96.0;
            mmi.ptMinTrackSize = POINT { x: (360.0 * s) as i32, y: (380.0 * s) as i32 };
            Some(LRESULT(0))
        }
        WM_DPICHANGED => {
            let r = *(lp.0 as *const RECT);
            let dpi = (wp.0 & 0xFFFF) as u32;
            with_app(|a| a.editor_dpi(dpi, &r)).map(|_| LRESULT(0))
        }
        WM_APP_ED_SUBMIT => with_app(|a| a.editor_submit()).map(|_| LRESULT(0)),
        WM_APP_ED_CANCEL => with_app(|a| a.editor_cancel()).map(|_| LRESULT(0)),
        WM_ACTIVATE if (wp.0 & 0xFFFF) != 0 => {
            if let Some(edit) = with_app(|a| a.editor.as_ref().map(|e| e.edit)).flatten() {
                let _ = SetFocus(edit);
            }
            Some(LRESULT(0))
        }
        WM_DESTROY => {
            with_app(|a| a.editor_closed(hwnd));
            Some(LRESULT(0))
        }
        _ => None,
    }
}

const WM_WTSSESSION_CHANGE_: u32 = 0x02B1;
const WTS_SESSION_LOCK_: usize = 7;
const WTS_SESSION_UNLOCK_: usize = 8;
const PBT_POWERSETTINGCHANGE_: usize = 0x8013;
const PBT_APMRESUMEAUTOMATIC_: usize = 0x12;

unsafe fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_TIMER => {
            match wp.0 {
                T_TICK => with_app(|a| a.on_tick()),
                T_STATS => with_app(|a| a.on_stats()),
                T_WEATHER => with_app(|a| a.start_weather()),
                T_DRAG => with_app(|a| a.drag_tick()),
                T_ANIM => with_app(|a| a.island_frame()),
                T_HOVER => with_app(|a| a.island_hover_check()),
                T_HOLD => with_app(|a| a.island_hold_done()),
                T_PHOTO => with_app(|a| a.next_photo()),
                T_PET => with_app(|a| a.pet_frame()),
                T_FS => with_app(|a| {
                    if (island_top(MEDIA) && a.ws[MEDIA].alive()) || a.dpet.is_some() {
                        a.tb_follow(false);
                        // A taskbar click doesn't always change the foreground window.
                        a.tb_raise();
                        a.refresh(false);
                    } else {
                        let _ = KillTimer(a.main, T_FS);
                    }
                }),
                T_TBAR => {
                    let _ = KillTimer(hwnd, T_TBAR);
                    with_app(|a| a.tb_follow(true))
                }
                T_RELOAD => {
                    let _ = KillTimer(hwnd, T_RELOAD);
                    with_app(|a| a.reload_files())
                }
                T_WALL => {
                    let _ = KillTimer(hwnd, T_WALL);
                    with_app(|a| {
                        a.sync_cfg_to_hub();
                        a.reload_wallpaper();
                        a.reattach();
                        a.ensure_onscreen();
                        a.pet_sync();
                    })
                }
                _ => None,
            };
            Some(LRESULT(0))
        }
        WM_APP_MEDIA => with_app(|a| a.on_media()).map(|_| LRESULT(0)),
        WM_APP_WEATHER => {
            let r = Box::from_raw(wp.0 as *mut sys::WeatherResult);
            with_app(move |a| a.on_weather(*r));
            Some(LRESULT(0))
        }
        WM_APP_FG => with_app(|a| a.on_foreground()).map(|_| LRESULT(0)),
        WM_APP_VIS => with_app(|a| a.on_vis()).map(|_| LRESULT(0)),
        WM_APP_PHOTO => {
            let r = Box::from_raw(wp.0 as *mut photos::Loaded);
            with_app(|a| a.on_photo(*r)).map(|_| LRESULT(0))
        }
        WM_APP_MSGS => with_app(|a| a.on_msgs()).map(|_| LRESULT(0)),
        WM_APP_PICKDIR => {
            // The folder dialog runs its own message loop: run it without holding app state.
            if let Some((owner, cur)) = with_app(|a| (a.settings.as_ref().map(|s| s.hwnd).unwrap_or(a.main), a.photo_dir())) {
                if let Some(dir) = pick_folder(owner, &cur) {
                    with_app(|a| a.set_photo_dir(dir));
                }
            }
            Some(LRESULT(0))
        }
        WM_APP_PICKCOLOR => {
            // Same as the folder dialog: its message loop must not run inside app state.
            let cur = with_app(|a| (a.settings.as_ref().map(|s| s.hwnd).unwrap_or(a.main), a.outline_rgb()));
            if let Some((owner, rgb)) = cur {
                if let Some(hex) = pick_color(owner, rgb) {
                    with_app(|a| a.set_outline_color(hex));
                }
            }
            Some(LRESULT(0))
        }
        WM_APP_TRAY => {
            let m = (lp.0 & 0xFFFF) as u32;
            if m == WM_RBUTTONUP {
                let _ = PostMessageW(hwnd, WM_APP_MENU, WPARAM(usize::MAX), LPARAM(0));
            } else if m == WM_LBUTTONUP {
                with_app(|a| a.open_settings());
            }
            Some(LRESULT(0))
        }
        WM_APP_MENU => {
            let idx = wp.0;
            // Build the menu, then run the modal menu loop without holding app state.
            if let Some(menu) = with_app(|a| if idx == PET_MENU { a.pet_menu() } else { a.build_menu(idx) }) {
                let mut pt = POINT::default();
                let _ = GetCursorPos(&mut pt);
                let _ = SetForegroundWindow(hwnd);
                let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_NONOTIFY, pt.x, pt.y, 0, hwnd, None);
                let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
                let _ = DestroyMenu(menu);
                if cmd.0 != 0 {
                    with_app(|a| a.command(cmd.0 as u32, idx));
                }
            }
            Some(LRESULT(0))
        }
        WM_SETTINGCHANGE => {
            let wallpaper = wp.0 as u32 == SPI_SETDESKWALLPAPER.0;
            let mut theme = false;
            if lp.0 != 0 {
                let s = PCWSTR(lp.0 as *const u16).to_string().unwrap_or_default();
                theme = s == "ImmersiveColorSet";
            }
            if wallpaper || theme {
                SetTimer(hwnd, T_WALL, 600, None);
            }
            // Taskbar alignment, Widgets button and the like are all settings changes.
            with_app(|a| a.tb_soon());
            if wp.0 as u32 == SPI_SETWORKAREA.0 {
                // Taskbar moved or resized: walk along its new edge.
                with_app(|a| {
                    a.pet_bounds();
                    a.pet_draw();
                });
            }
            Some(LRESULT(0))
        }
        WM_DISPLAYCHANGE => {
            SetTimer(hwnd, T_WALL, 800, None);
            with_app(|a| a.tb_soon());
            Some(LRESULT(0))
        }
        WM_POWERBROADCAST => {
            if wp.0 == PBT_POWERSETTINGCHANGE_ {
                let s = &*(lp.0 as *const POWERBROADCAST_SETTING);
                if s.PowerSetting == GUID_CONSOLE_DISPLAY_STATE {
                    let off = s.Data[0] == 0;
                    with_app(|a| {
                        if a.display_off != off {
                            a.display_off = off;
                            a.set_paused();
                        }
                    });
                }
            } else if wp.0 == PBT_APMRESUMEAUTOMATIC_ {
                SetTimer(hwnd, T_WEATHER, 15_000, None);
            }
            Some(LRESULT(1))
        }
        WM_WTSSESSION_CHANGE_ => {
            let lock = match wp.0 {
                WTS_SESSION_LOCK_ => Some(true),
                WTS_SESSION_UNLOCK_ => Some(false),
                _ => None,
            };
            if let Some(l) = lock {
                with_app(|a| {
                    a.locked_session = l;
                    a.set_paused();
                });
            }
            Some(LRESULT(0))
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            Some(LRESULT(0))
        }
        m => {
            let tc = with_app(|a| a.taskbar_created).unwrap_or(0);
            if tc != 0 && m == tc {
                with_app(|a| {
                    a.add_tray();
                    a.reattach();
                    // New taskbar: get back on top of it, in the right place.
                    if a.hub.taskbar && a.ws[MEDIA].alive() {
                        a.tb_raise();
                    }
                    a.tb_soon();
                });
                return Some(LRESULT(0));
            }
            None
        }
    }
}

unsafe extern "system" fn win_event(_: HWINEVENTHOOK, _: u32, _: HWND, _: i32, _: i32, _: u32, _: u32) {
    let _ = PostMessageW(main_hwnd(), WM_APP_FG, WPARAM(0), LPARAM(0));
}

// ---------------------------------------------------------------- entry

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let snapshot = args.iter().position(|a| a == "--snapshot").and_then(|i| args.get(i + 1).cloned());
    unsafe {
        if snapshot.is_none() && !args.iter().any(|a| a == "--media-test") {
            let _m = CreateMutexW(None, true, w!("Local\\FrostWidgetsSingleton"));
            if GetLastError() == ERROR_ALREADY_EXISTS {
                return;
            }
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        run(snapshot);
    }
}

unsafe fn run(snapshot: Option<String>) {
    let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
    let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
    for name in [w!("FrostWidget"), w!("FrostHost"), w!("FrostEditor"), w!("FrostSettings"), w!("FrostPet")] {
        let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: name, hCursor: cursor, ..Default::default() };
        RegisterClassW(&wc);
    }
    let Ok(main) = CreateWindowExW(WS_EX_TOOLWINDOW, w!("FrostHost"), w!("Frost Widgets"), WS_POPUP, 0, 0, 0, 0, None, None, hinst, None)
    else {
        return;
    };
    MAIN.store(main.0 as isize, Ordering::Relaxed);

    let dir = config::dir();
    let cfg = config::load(&dir);
    let notes = config::load_notes(&dir);
    let Ok(gfx) = Gfx::new() else { return };
    let (now, now_ft) = local_now();
    let mut app = App {
        hinst,
        main,
        gfx,
        screen: GetDC(None),
        cfg_time: config::mtime(&dir.join("config.toml")),
        notes_time: config::mtime(&dir.join("notes.txt")),
        dir,
        cfg,
        hub: Hub {
            now,
            now_ft,
            cpu: 0.0,
            ram: 0.0,
            disk: 0.0,
            down: 0.0,
            up: 0.0,
            bat: Default::default(),
            weather: None,
            weather_failed: false,
            weather_msg: "",
            imperial: false,
            vis_on: true,
            vis: [0.0; vis::BANDS],
            seek: None,
            bar_hover: false,
            island: false,
            isl_t: 0.0,
            isl_cw: 200.0,
            outline: None,
            taskbar: false,
            tb_vis: true,
            tb_hot: 0,
            tb_right: false,
            gpu: None,
            photo_id: 0,
            photo_msg: "",
            msgs: Vec::new(),
            msg_status: "",
            msg_preview: true,
            media: Default::default(),
            notes,
            city: String::new(),
            h24: true,
            seconds: true,
            monday: false,
        },
        stats: sys::Stats::new(),
        ws: KINDS.iter().map(|&k| W::new(k)).collect(),
        walls: Vec::new(),
        media: media::start(main.0 as isize, WM_APP_MEDIA),
        geo: None,
        weather_busy: false,
        dark: false,
        accent: Color::hex(0xFF6A88),
        display_off: false,
        locked_session: false,
        moving: None,
        drag_dirty: false,
        editor: None,
        settings: None,
        big_icon: HICON::default(),
        mem_mb: 0.0,
        vis: vis::start(main.0 as isize, WM_APP_VIS),
        seeking: None,
        mouse_tracking: false,
        weather_fails: 0,
        tray_icon: HICON::default(),
        taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        isl: IslandAnim::default(),
        gpu: None,
        gpus: None,
        photos: None,
        photo: None,
        photo_seq: 0,
        island_shown: true,
        tb_probe: Default::default(),
        tb_probe_at: None,
        tb_centered: true,
        tb_setting: true,
        msgs: msgs::start(main.0 as isize, WM_APP_MSGS),
        dpet: None,
    };
    app.sync_cfg_to_hub();
    app.layout_defaults();
    app.save_cfg();
    app.on_stats();

    if let Some(out) = snapshot {
        snapshot_mode(&mut app, &out);
        return;
    }
    if let Some(out) = std::env::args().skip_while(|a| a != "--media-test").nth(1) {
        media_test(&mut app, &out);
        return;
    }

    app.start_weather();
    for (i, k) in KINDS.iter().enumerate() {
        if app.cfg.pos(k.key()).enabled {
            app.create_widget(i, true);
        }
    }
    app.pet_sync();
    app.tray_icon = load_app_icon(hinst, GetSystemMetrics(SM_CXSMICON));
    app.add_tray();
    app.set_stats_timer(true);
    let _ = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(win_event), 0, 0, WINEVENT_OUTOFCONTEXT);
    let _ = RegisterPowerSettingNotification(HANDLE(main.0), &GUID_CONSOLE_DISPLAY_STATE, DEVICE_NOTIFY_WINDOW_HANDLE);
    let _ = WTSRegisterSessionNotification(main, NOTIFY_FOR_THIS_SESSION);
    let dirw = util::wide(&app.dir.to_string_lossy());
    let change = FindFirstChangeNotificationW(PCWSTR(dirw.as_ptr()), false, FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME).ok();

    APP.with(|a| *a.borrow_mut() = Some(app));
    with_app(|a| a.schedule_tick());
    trim_memory();

    // Sleep until a window message or a settings-file change arrives.
    let handles: Vec<HANDLE> = change.into_iter().collect();
    let mut msg = MSG::default();
    loop {
        let r = MsgWaitForMultipleObjectsEx(Some(&handles), INFINITE, QS_ALLINPUT, MWMO_INPUTAVAILABLE);
        if !handles.is_empty() && r == WAIT_OBJECT_0 {
            let _ = FindNextChangeNotification(handles[0]);
            SetTimer(main, T_RELOAD, 400, None);
        }
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            if msg.message == WM_QUIT {
                with_app(|a| a.remove_tray());
                return;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// `frost.exe --snapshot out.png`: render every widget over the real wallpaper
/// into one PNG (for previews), then exit.
unsafe fn snapshot_mode(app: &mut App, out: &str) {
    let r = sys::fetch_weather(&app.cfg.city, app.hub.imperial, None);
    app.geo = r.geo.clone();
    app.hub.weather = r.weather;
    app.sync_cfg_to_hub();
    std::thread::sleep(std::time::Duration::from_millis(1500));
    app.hub.media = app.media.state.lock().map(|g| g.clone()).unwrap_or_default();
    if !app.hub.media.has {
        // Nothing playing: preview with sample content so the layout can be checked.
        let n = 256u32;
        let mut px = vec![0u8; (n * n * 4) as usize];
        for y in 0..n {
            for x in 0..n {
                let t = (x + y) as f32 / (2 * n) as f32;
                let o = ((y * n + x) * 4) as usize;
                px[o] = (255.0 * (0.53 + 0.47 * t)) as u8;
                px[o + 1] = (255.0 * (0.42 - 0.06 * t)) as u8;
                px[o + 2] = (255.0 * (1.0 - 0.58 * t)) as u8;
                px[o + 3] = 255;
            }
        }
        let (_, now_ft) = local_now();
        app.hub.media = media::MediaState {
            has: true,
            title: "Midnight Drive".into(),
            artist: "Neon Coast".into(),
            app: "Spotify".into(),
            playing: true,
            pos: 83.0,
            dur: 214.0,
            updated: now_ft,
            start: 0,
            can_seek: true,
            art: Some(std::sync::Arc::new(media::Art { w: n, h: n, px, tint: 0xFF6AB0 })),
            art_id: 7,
        };
        app.hub.bar_hover = true;
    }
    for (i, v) in app.hub.vis.iter_mut().enumerate() {
        *v = (0.35 + 0.6 * ((i as f32 * 0.9).sin() * 0.5 + 0.5) * (1.0 - i as f32 / 22.0)).clamp(0.0, 1.0);
    }    std::thread::sleep(std::time::Duration::from_millis(900));
    // Photos from the output folder (not the user's pictures); sample messages.
    if let Some(d) = std::path::Path::new(out).parent() {
        app.cfg.photo_dir = d.display().to_string();
    }
    let (_, nft) = local_now();
    let m = |app: &str, t: &str, b: &str, mins: i64| msgs::Msg { id: 0, app: app.into(), title: t.into(), body: b.into(), time: nft - mins * 600_000_000 };
    app.hub.msgs = vec![
        m("WhatsApp", "Amaya", "Are we still on for dinner tonight?", 0),
        m("Telegram", "Design team", "Kasun: pushed the new mockups, take a look", 12),
        m("Outlook", "Weekly report", "Your summary for this week is ready to review", 190),
    ];
    app.cfg.gpu_on = true;
    app.on_stats();
    for i in 0..KINDS.len() {
        app.create_widget(i, false);
    }
    // Wait for the photo loader's answer.
    let t0 = std::time::Instant::now();
    let mut msg = MSG::default();
    while t0.elapsed().as_secs() < 8 && app.hub.photo_id == 0 && app.hub.photo_msg.is_empty() {
        if PeekMessageW(&mut msg, None, WM_APP_PHOTO, WM_APP_PHOTO, PM_REMOVE).as_bool() {
            let r = Box::from_raw(msg.wParam.0 as *mut photos::Loaded);
            app.on_photo(*r);
        } else {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(1100));
    app.on_stats();
    for i in 0..KINDS.len() {
        app.draw(i);
    }
    // Bounding box of all windows.
    let (mut l, mut t, mut r_, mut b) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for w in &app.ws {
        let (ww, wh, m) = w.dims();
        l = l.min(w.gx - m);
        t = t.min(w.gy - m);
        r_ = r_.max(w.gx - m + ww);
        b = b.max(w.gy - m + wh);
    }
    let (cw, ch) = ((r_ - l) as usize, (b - t) as usize);
    // Sharp wallpaper as the backdrop.
    let hmon = MonitorFromPoint(POINT { x: l + 1, y: t + 1 }, MONITOR_DEFAULTTONEAREST);
    let (mon, _) = monitor_rect(hmon);
    let (mw, mh) = ((mon.right - mon.left) as usize, (mon.bottom - mon.top) as usize);
    let mut wall = vec![40u8; mw * mh * 3];
    let mut wpbuf = [0u16; 520];
    let _ = SystemParametersInfoW(SPI_GETDESKWALLPAPER, 520, Some(wpbuf.as_mut_ptr() as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    let n = wpbuf.iter().position(|&c| c == 0).unwrap_or(0);
    let mut path = String::from_utf16_lossy(&wpbuf[..n]);
    if path.is_empty() || !std::path::Path::new(&path).exists() {
        path = std::env::var("APPDATA").unwrap_or_default() + "\\Microsoft\\Windows\\Themes\\TranscodedWallpaper";
    }
    let _ = gfx::decode_wall(&app.gfx.wic, &path, mw, mh, 10, &mut wall);
    let mut img = vec![0u8; cw * ch * 4];
    for y in 0..ch {
        for x in 0..cw {
            let (sx, sy) = ((l - mon.left) as usize + x, (t - mon.top) as usize + y);
            let s = if sx < mw && sy < mh { (sy * mw + sx) * 3 } else { 0 };
            let d = (y * cw + x) * 4;
            img[d] = wall[s + 2];
            img[d + 1] = wall[s + 1];
            img[d + 2] = wall[s];
            img[d + 3] = 255;
        }
    }
    for w in &mut app.ws {
        let (_, _, m) = w.dims();
        let (ox, oy) = ((w.gx - m - l) as usize, (w.gy - m - t) as usize);
        let Some(s) = w.surf.as_mut() else { continue };
        let (sw, sh) = (s.w as usize, s.h as usize);
        let px = s.pixels();
        for y in 0..sh {
            for x in 0..sw {
                let p = px[y * sw + x];
                let a = (p >> 24) as f32 / 255.0;
                let d = ((oy + y) * cw + ox + x) * 4;
                for (c, sh_) in [(0usize, 0u32), (1, 8), (2, 16)] {
                    let src = ((p >> sh_) & 255) as f32;
                    img[d + c] = (src + img[d + c] as f32 * (1.0 - a)).min(255.0) as u8;
                }
            }
        }
    }
    let _ = gfx::save_png(&app.gfx.wic, out, cw as u32, ch as u32, &mut img);

    // Also render the reminders editor (with a hovered row) next to it.
    app.open_editor();
    let th = editor::theme(app.dark, app.accent);
    if let Some(e) = app.editor.as_mut() {
        e.hover = editor::Hit::Row(1);
        let dc = GetDC(e.hwnd);
        e.paint(&mut app.gfx, app.screen, dc, &app.hub.notes, &th);
        ReleaseDC(e.hwnd, dc);
        if let Some(s) = e.surf.as_mut() {
            let (w, h) = (s.w as u32, s.h as u32);
            let mut bytes: Vec<u8> = s.pixels().iter().flat_map(|p| (p | 0xFF00_0000).to_le_bytes()).collect();
            let _ = gfx::save_png(&app.gfx.wic, &out.replace(".png", "-editor.png"), w, h, &mut bytes);
        }
        let _ = DestroyWindow(e.hwnd);
    }

    // And every settings tab.
    app.open_settings();
    for tab in 0..settings::TAB_COUNT {
        let Some(st) = app.settings.as_mut() else { break };
        st.tab = tab;
        st.hover = None;
        let hwnd = st.hwnd;
        let dc = GetDC(hwnd);
        // settings_paint uses BeginPaint; paint directly instead.
        let th = editor::theme(app.dark, app.accent);
        let mut on = [false; KINDS.len()];
        for (i, w) in app.ws.iter().enumerate() {
            on[i] = w.alive();
        }
        let gpu_name = app.gpu_adapter().map(|a| a.name).unwrap_or_default();
        let gpu_count = app.adapters().len();
        let photo_dir = app.photo_dir().display().to_string();
        let view = settings::View {
            cfg: &app.cfg,
            autostart: util::reg_string(HKEY_CURRENT_USER, RUN_KEY, APP_NAME).is_some(),
            widgets_on: on,
            weather: "Now: Colombo, 25\u{B0} and drizzle".into(),
            mem_mb: private_mem_mb(),
            dir: app.dir.display().to_string(),
            exe_dir: "C:\\Users\\you\\AppData\\Local\\Programs\\Frost Widgets".into(),
            icon: app.big_icon,
            gpu_name,
            gpu_count,
            photo_dir,
        };
        let st = app.settings.as_mut().unwrap();
        st.paint(&mut app.gfx, app.screen, dc, &view, &th);
        ReleaseDC(hwnd, dc);
        if let Some(s) = st.surf.as_mut() {
            let (w, h) = (s.w as u32, s.h as u32);
            let mut bytes: Vec<u8> = s.pixels().iter().flat_map(|p| (p | 0xFF00_0000).to_le_bytes()).collect();
            let _ = gfx::save_png(&app.gfx.wic, &out.replace(".png", &format!("-settings{tab}.png")), w, h, &mut bytes);
        }
    }
    if let Some(st) = &app.settings {
        let _ = DestroyWindow(st.hwnd);
    }

    // Desktop pet: every pose in a row, standing on a taskbar strip.
    {
        use desk_pet::{posed, Act, GH, GW};
        let poses = [
            posed(Act::Walk, 0, 1, false),
            posed(Act::Walk, 1, 1, false),
            posed(Act::Walk, 2, -1, false),
            posed(Act::Sit, 0, 1, false),
            posed(Act::Sit, 0, 1, true),
            posed(Act::Laptop, 0, 1, false),
            posed(Act::Laptop, 1, 1, false),
            posed(Act::Laptop, 0, 1, true),
            posed(Act::Sleep, 0, 1, false),
            posed(Act::Sleep, 1, -1, false),
            posed(Act::Hop, 3, 1, false),
            posed(Act::Held, 0, 1, false),
        ];
        let s = 5usize;
        let (fw, fh) = (GW * s, GH * s);
        let pad = 8usize;
        let (w, h) = (poses.len() * (fw + pad) + pad, fh + 40);
        let ground = pad + (desk_pet::GROUND_ROWS as usize) * s;
        let mut img = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let c: [u8; 3] = if y >= ground { [0x20, 0x20, 0x24] } else { [0x7A, 0x8C, 0xA8] };
                let d = (y * w + x) * 4;
                img[d] = c[2];
                img[d + 1] = c[1];
                img[d + 2] = c[0];
                img[d + 3] = 255;
            }
        }
        let mut buf = vec![0u32; GW * GH];
        for (k, p) in poses.iter().enumerate() {
            p.render(&mut buf);
            let ox = pad + k * (fw + pad);
            for gy in 0..GH {
                for gx in 0..GW {
                    let v = buf[gy * GW + gx];
                    let a = (v >> 24) as f32 / 255.0;
                    if a == 0.0 {
                        continue;
                    }
                    for yy in 0..s {
                        for xx in 0..s {
                            let d = ((pad + gy * s + yy) * w + ox + gx * s + xx) * 4;
                            for (c, sh) in [(0usize, 0u32), (1, 8), (2, 16)] {
                                let src = ((v >> sh) & 255) as f32;
                                img[d + c] = (src + img[d + c] as f32 * (1.0 - a)).min(255.0) as u8;
                            }
                        }
                    }
                }
            }
        }
        let _ = gfx::save_png(&app.gfx.wic, &out.replace(".png", "-pet.png"), w as u32, h as u32, &mut img);
    }

    // Island style: animation frames stacked top to bottom over a grey backdrop.
    app.hub.island = true;
    widgets::ISLAND.store(true, Ordering::Relaxed);
    app.destroy_widget(MEDIA);
    app.create_widget(MEDIA, false);
    let frames = [0.0f32, 0.3, 0.6, 0.85, 1.08, 1.0];
    let (ww, wh, _) = app.ws[MEDIA].dims();
    let (fw, fh) = (ww as usize, wh as usize);
    let mut img = vec![0u8; fw * fh * frames.len() * 4];
    for (n, t) in frames.iter().enumerate() {
        app.hub.isl_t = *t;
        app.draw(MEDIA);
        let Some(s) = app.ws[MEDIA].surf.as_mut() else { break };
        let px = s.pixels();
        for y in 0..fh {
            for x in 0..fw {
                let p = px[y * fw + x];
                let a = (p >> 24) as f32 / 255.0;
                let d = ((n * fh + y) * fw + x) * 4;
                let bg = [150.0, 140.0, 128.0];
                for (c, sh_) in [(0usize, 0u32), (1, 8), (2, 16)] {
                    img[d + c] = (((p >> sh_) & 255) as f32 + bg[c] * (1.0 - a)).min(255.0) as u8;
                }
                img[d + 3] = 255;
            }
        }
    }
    let _ = gfx::save_png(&app.gfx.wic, &out.replace(".png", "-island.png"), fw as u32, (fh * frames.len()) as u32, &mut img);

    // Outline on a black screen (the case it exists for): off, then white and pink,
    // compact and open.
    let looks = [(None, 0.0f32), (None, 1.0), (Some(0xFFFFFFu32), 0.0), (Some(0xFFFFFF), 1.0), (Some(0xFF6A88), 0.0), (Some(0xFF6A88), 1.0)];
    let mut img = vec![0u8; fw * fh * looks.len() * 4];
    for (n, (o, t)) in looks.iter().enumerate() {
        app.hub.outline = *o;
        app.hub.isl_t = *t;
        app.draw(MEDIA);
        let Some(s) = app.ws[MEDIA].surf.as_mut() else { break };
        let px = s.pixels();
        for y in 0..fh {
            for x in 0..fw {
                let p = px[y * fw + x];
                let d = ((n * fh + y) * fw + x) * 4;
                for (c, sh_) in [(0usize, 0u32), (1, 8), (2, 16)] {
                    img[d + c] = ((p >> sh_) & 255) as u8;
                }
                img[d + 3] = 255;
            }
        }
    }
    app.hub.outline = None;
    let _ = gfx::save_png(&app.gfx.wic, &out.replace(".png", "-island-outline.png"), fw as u32, (fh * looks.len()) as u32, &mut img);

    // Taskbar style over a taskbar-colored strip: plain, hovered, and without bars.
    app.hub.island = false;
    widgets::ISLAND.store(false, Ordering::Relaxed);
    app.hub.taskbar = true;
    widgets::TASKBAR.store(true, Ordering::Relaxed);
    app.destroy_widget(MEDIA);
    app.create_widget(MEDIA, false);
    let light = taskbar_light();
    let bg = if light { [238.0, 238.0, 238.0] } else { [32.0, 32.0, 32.0] };
    let looks = [(true, 0.0f32), (false, 0.0), (true, 0.45), (true, 1.06), (true, 1.0)];
    let zs = app.ws[MEDIA].s;
    let (ww, wh, _) = app.ws[MEDIA].dims();
    let (fw, fh) = (ww as usize, wh as usize);
    let mut img = vec![0u8; fw * fh * looks.len() * 4];
    // Mouse-catching pixels (alpha > 0) for each look and side: above the taskbar band,
    // and outside the shape it opens on (anything there would open it from afar).
    use std::fmt::Write as _;
    let mut hits = String::new();
    for (right, outline) in [(false, None), (true, None), (true, Some(0xFF6A88u32))] {
        app.hub.tb_right = right;
        app.hub.outline = outline;
        app.ws[MEDIA].bg_ok = false;
        for (vis_on, t) in looks {
            app.hub.tb_vis = vis_on;
            app.hub.isl_t = t;
            app.draw(MEDIA);
            let Some(s) = app.ws[MEDIA].surf.as_mut() else { break };
            let px = s.pixels();
            let (sx, sy, sw, sh, _) = widgets::tb_shape(t, right);
            let (mut above, mut outside, mut total) = (0, 0, 0);
            for y in 0..fh {
                for x in 0..fw {
                    if px[y * fw + x] >> 24 == 0 {
                        continue;
                    }
                    total += 1;
                    let (lx, ly) = (x as f32 / zs, y as f32 / zs);
                    if ly < widgets::TB_H - 48.0 {
                        above += 1;
                    }
                    if lx < sx - 1.0 || lx > sx + sw + 1.0 || ly < sy - 1.0 || ly > sy + sh + 1.0 {
                        outside += 1;
                    }
                }
            }
            let _ = writeln!(hits, "right={right} outline={} t={t:.2} vis={vis_on}: catching={total} above_taskbar={above} outside_shape={outside}", outline.is_some());
        }
    }
    app.hub.tb_right = false;
    app.ws[MEDIA].bg_ok = false;
    for (n, (vis_on, t)) in looks.iter().enumerate() {
        // Rows 2 and 5 (pill without bars, open card) show the outline.
        app.hub.outline = (n == 1 || n == 4).then_some(0xFF6A88);
        app.hub.tb_vis = *vis_on;
        app.hub.isl_t = *t;
        app.hub.tb_hot = if *t >= 1.0 { 2 } else { 0 };
        app.draw(MEDIA);
        let Some(s) = app.ws[MEDIA].surf.as_mut() else { break };
        let px = s.pixels();
        for y in 0..fh {
            for x in 0..fw {
                let p = px[y * fw + x];
                let a = (p >> 24) as f32 / 255.0;
                let d = ((n * fh + y) * fw + x) * 4;
                // Above the taskbar: a desktop-ish backdrop (BGR).
                let desk = [150.0, 118.0, 92.0];
                let under = if (y as f32) < (widgets::TB_H - 48.0) * zs { &desk } else { &bg };
                for (c, sh_) in [(0usize, 0u32), (1, 8), (2, 16)] {
                    img[d + c] = (((p >> sh_) & 255) as f32 + under[c] * (1.0 - a)).min(255.0) as u8;
                }
                img[d + 3] = 255;
            }
        }
    }
    let _ = gfx::save_png(&app.gfx.wic, &out.replace(".png", "-taskbar.png"), fw as u32, (fh * looks.len()) as u32, &mut img);
    let spot = app.tb_spot();
    let _ = std::fs::write(
        out.replace(".png", "-taskbar.txt"),
        format!("centered={} probe={:?} spot={:?}\n{hits}", app.tb_centered, app.tb_probe, spot),
    );
}

/// `frost.exe --media-test out.txt`: report what the media session API sees,
/// try a seek, and probe audio capture. For diagnostics only.
unsafe fn media_test(app: &mut App, out: &str) {
    use std::fmt::Write as _;
    let mut log = String::new();
    let dump = |app: &App, log: &mut String, label: &str| {
        let m = app.media.state.lock().map(|g| g.clone()).unwrap_or_default();
        let (_, ft) = local_now();
        let _ = writeln!(
            log,
            "{label}: has={} playing={} app={:?} title={:?} artist={:?} dur={:.1} pos_now={:.1} can_seek={} art={}",
            m.has, m.playing, m.app, m.title, m.artist, m.dur, m.position(ft), m.can_seek, m.art.is_some()
        );
    };
    std::thread::sleep(std::time::Duration::from_millis(2500));
    dump(app, &mut log, "initial");
    app.media.cmd(media::Cmd::Seek(20.0));
    std::thread::sleep(std::time::Duration::from_millis(2000));
    dump(app, &mut log, "after seek to 20s");
    app.media.cmd(media::Cmd::Toggle);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    dump(app, &mut log, "after play/pause toggle");
    app.media.cmd(media::Cmd::Toggle);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    dump(app, &mut log, "after second toggle");
    let _ = writeln!(log, "audio: {}", vis::probe(1500));

    // Visualizer cost: real capture thread + bar-strip repaints of a (hidden) widget for 10 s.
    let cpu_ms = || {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
        let t = |f: FILETIME| (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 10_000.0;
        t(k) + t(u)
    };
    app.hub.media = app.media.state.lock().map(|g| g.clone()).unwrap_or_default();
    app.cfg.visualizer = true;
    app.sync_cfg_to_hub();
    app.create_widget(MEDIA, false);
    app.vis.set_active(true);
    app.on_vis();
    let c0 = cpu_ms();
    let t0 = std::time::Instant::now();
    let mut frames = 0;
    while t0.elapsed().as_secs_f64() < 10.0 {
        std::thread::sleep(std::time::Duration::from_millis(33));
        app.on_vis();
        frames += 1;
    }
    let used = cpu_ms() - c0;
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
    let _ = writeln!(
        log,
        "visualizer: {frames} frames in 10 s, {used:.0} ms CPU = {:.2}% of one core, {:.3}% of the whole CPU ({cores} threads)",
        used / 100.0,
        used / 100.0 / cores
    );
    app.vis.set_active(false);
    let _ = std::fs::write(out, log);
}