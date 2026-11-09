#![windows_subsystem = "windows"]
#![allow(dead_code)]
//! Frost Widgets installer and uninstaller.
//! One small exe: the app is compressed inside it. Installs (with an admin prompt)
//! to C:\Program Files\Frost Widgets: frost.exe uses UI Access so its taskbar pill
//! stays visible over Start, and Windows only allows that for a signed exe in a
//! protected folder. Settings stay per user. Adds a Start
//! menu entry and an entry in Settings > Apps. The installed copy of this exe
//! becomes the uninstaller (`uninstall.exe --uninstall`).
//!
//! Flags: --silent (no UI), --uninstall

#[path = "../gfx.rs"]
mod gfx;
#[path = "../util.rs"]
mod util;

use std::cell::RefCell;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};

use windows::core::*;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Dwm::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use gfx::{Align, Color, Fam, Gfx, Surface, WHITE};

static PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));
const VERSION: &str = env!("CARGO_PKG_VERSION");
const UNINST_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\FrostWidgets";
const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const APP_NAME: &str = "FrostWidgets";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

// ------------------------------------------------------------------ install logic

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// Built as the UI Access edition (Program Files, admin prompt, signed app)?
const UIACCESS: bool = option_env!("FROST_UIACCESS").is_some();

fn install_dir() -> PathBuf {
    if !UIACCESS {
        // Normal edition: per user, no admin prompt.
        return known_folder(&FOLDERID_UserProgramFiles)
            .unwrap_or_else(|| PathBuf::from(std::env::var("LOCALAPPDATA").unwrap_or_default()).join("Programs"))
            .join("Frost Widgets");
    }
    known_folder(&FOLDERID_ProgramFiles)
        .unwrap_or_else(|| PathBuf::from(std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into())))
        .join("Frost Widgets")
}

fn start_menu_link() -> Option<PathBuf> {
    known_folder(&FOLDERID_Programs).map(|p| p.join("Frost Widgets.lnk"))
}

fn desktop_link() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop).map(|p| p.join("Frost Widgets.lnk"))
}

fn installed() -> bool {
    util::reg_string(HKEY_CURRENT_USER, UNINST_KEY, "InstallLocation")
        .map(|d| Path::new(&d).join("frost.exe").exists())
        .unwrap_or(false)
}

/// Ask a running copy to exit (it removes its tray icon), then make sure it's gone.
fn close_running() {
    unsafe {
        let Ok(h) = FindWindowW(w!("FrostHost"), None) else { return };
        if h.0.is_null() {
            return;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let proc = OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, false, pid);
        let _ = PostMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
        if let Ok(p) = proc {
            if WaitForSingleObject(p, 5000) != WAIT_OBJECT_0 {
                let _ = TerminateProcess(p, 0);
                let _ = WaitForSingleObject(p, 2000);
            }
            let _ = CloseHandle(p);
        }
    }
}

fn shortcut(lnk: &Path, target: &Path) -> Result<()> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let t = util::wide(&target.display().to_string());
        let dir = util::wide(&target.parent().unwrap_or(Path::new(".")).display().to_string());
        link.SetPath(PCWSTR(t.as_ptr()))?;
        link.SetWorkingDirectory(PCWSTR(dir.as_ptr()))?;
        link.SetIconLocation(PCWSTR(t.as_ptr()), 0)?;
        link.SetDescription(w!("Glass desktop widgets"))?;
        let file: IPersistFile = link.cast()?;
        let l = util::wide(&lnk.display().to_string());
        file.Save(PCWSTR(l.as_ptr()), true)?;
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Opts {
    autostart: bool,
    desktop: bool,
    launch: bool,
    remove_data: bool,
}

fn do_install(o: &Opts) -> std::result::Result<(), String> {
    if PAYLOAD.len() < 8 {
        return Err("This setup was built without the app inside it.".into());
    }
    let size = u32::from_le_bytes(PAYLOAD[..4].try_into().unwrap()) as usize;
    let exe_bytes = miniz_oxide::inflate::decompress_to_vec_with_limit(&PAYLOAD[4..], size + 1)
        .map_err(|_| "The installer data is damaged. Download the setup again.".to_string())?;

    close_running();
    let dir = install_dir();
    // An older per-user install (before UI Access) lives elsewhere: remove it once
    // the new copy is in place.
    let old = util::reg_string(HKEY_CURRENT_USER, UNINST_KEY, "InstallLocation").map(PathBuf::from).filter(|d| *d != dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Couldn't create {}: {e}", dir.display()))?;
    let exe = dir.join("frost.exe");
    let mut wrote = false;
    for _ in 0..10 {
        if std::fs::write(&exe, &exe_bytes).is_ok() {
            wrote = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    if !wrote {
        return Err("Couldn't replace frost.exe. Close Frost Widgets and try again.".into());
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let uninst = dir.join("uninstall.exe");
    if me != uninst {
        let _ = std::fs::copy(&me, &uninst);
    }

    if let Some(l) = start_menu_link() {
        let _ = shortcut(&l, &exe);
    }
    if let Some(l) = desktop_link() {
        if o.desktop {
            let _ = shortcut(&l, &exe);
        } else {
            let _ = std::fs::remove_file(l);
        }
    }
    let quoted = format!("\"{}\"", exe.display());
    if o.autostart {
        util::reg_set_string(HKEY_CURRENT_USER, RUN_KEY, APP_NAME, &quoted);
    } else {
        util::reg_delete(HKEY_CURRENT_USER, RUN_KEY, APP_NAME);
    }

    let st = unsafe { GetLocalTime() };
    let kb = (std::fs::metadata(&exe).map(|m| m.len()).unwrap_or(0) + std::fs::metadata(&uninst).map(|m| m.len()).unwrap_or(0)) / 1024;
    let set = |n: &str, v: &str| util::reg_set_string(HKEY_CURRENT_USER, UNINST_KEY, n, v);
    set("DisplayName", "Frost Widgets");
    set("DisplayVersion", VERSION);
    set("Publisher", "Frost Widgets");
    set("DisplayIcon", &format!("{},0", exe.display()));
    set("InstallLocation", &dir.display().to_string());
    set("UninstallString", &format!("\"{}\" --uninstall", uninst.display()));
    set("QuietUninstallString", &format!("\"{}\" --uninstall --silent", uninst.display()));
    set("InstallDate", &format!("{:04}{:02}{:02}", st.wYear, st.wMonth, st.wDay));
    util::reg_set_dword(HKEY_CURRENT_USER, UNINST_KEY, "EstimatedSize", kb as u32);
    util::reg_set_dword(HKEY_CURRENT_USER, UNINST_KEY, "NoModify", 1);
    util::reg_set_dword(HKEY_CURRENT_USER, UNINST_KEY, "NoRepair", 1);

    if let Some(old) = old {
        let _ = std::fs::remove_file(old.join("frost.exe"));
        let _ = std::fs::remove_file(old.join("uninstall.exe"));
        let _ = std::fs::remove_dir(&old);
    }

    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

/// Start the app as the signed-in user, not as admin: this installer runs elevated,
/// and Explorer launches it un-elevated (a UI Access exe also has to be started
/// through the shell, not CreateProcess).
fn launch_app() {
    let exe = install_dir().join("frost.exe");
    let _ = std::process::Command::new("explorer.exe").arg(&exe).spawn();
}

fn do_uninstall(o: &Opts) -> std::result::Result<(), String> {
    close_running();
    let dir = util::reg_string(HKEY_CURRENT_USER, UNINST_KEY, "InstallLocation").map(PathBuf::from).unwrap_or_else(install_dir);
    let _ = std::fs::remove_file(dir.join("frost.exe"));
    for l in [start_menu_link(), desktop_link()].into_iter().flatten() {
        let _ = std::fs::remove_file(l);
    }
    util::reg_delete(HKEY_CURRENT_USER, RUN_KEY, APP_NAME);
    util::reg_delete_tree(HKEY_CURRENT_USER, UNINST_KEY);
    if o.remove_data {
        if let Ok(a) = std::env::var("APPDATA") {
            let _ = std::fs::remove_dir_all(PathBuf::from(a).join("FrostWidgets"));
        }
    }
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
    Ok(())
}

/// The uninstaller can't delete itself while running; let cmd remove the folder after we exit.
fn schedule_folder_removal() {
    let dir = util::reg_string(HKEY_CURRENT_USER, UNINST_KEY, "InstallLocation").map(PathBuf::from).unwrap_or_else(install_dir);
    let me = std::env::current_exe().ok();
    if me.as_ref().and_then(|m| m.parent()) != Some(dir.as_path()) {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let cmd = format!("/c ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{}\"", dir.display());
    let _ = std::process::Command::new("cmd.exe")
        .raw_arg(cmd)
        .current_dir(std::env::temp_dir())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}

// ------------------------------------------------------------------ UI

#[derive(Clone, PartialEq)]
enum Stage {
    Ready,
    Working,
    Done,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Act {
    Autostart,
    Desktop,
    Launch,
    RemoveData,
    Primary,
    Cancel,
}

struct Ui {
    hwnd: HWND,
    gfx: Gfx,
    screen: HDC,
    surf: Option<Surface>,
    s: f32,
    uninstall: bool,
    update: bool,
    opts: Opts,
    stage: Stage,
    hover: Option<Act>,
    zones: Vec<(f32, f32, f32, f32, Act)>,
    dark: bool,
    icon: HICON,
}

const W0: f32 = 560.0;
const H0: f32 = 380.0;

thread_local! {
    static UI: RefCell<Option<Ui>> = const { RefCell::new(None) };
}

fn with_ui<R>(f: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    UI.with(|u| u.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

impl Ui {
    fn colors(&self) -> (Color, Color, Color, Color, Color) {
        // bg, card, fg, fg2, accent
        if self.dark {
            (Color::hex(0x1E1E22), Color::hex(0x2A2A30), WHITE, WHITE.alpha(0.6), Color::hex(0xFF6A88))
        } else {
            let fg = Color::hex(0x1D1D1F);
            (Color::hex(0xF6F6F8), WHITE, fg, fg.alpha(0.58), Color::hex(0xE84A6E))
        }
    }

    fn checkbox(&mut self, x: f32, y: f32, w: f32, label: &str, on: bool, act: Act) {
        let (_, _, fg, _, accent) = self.colors();
        let hov = self.hover == Some(act);
        if on {
            self.gfx.fill_rrect(x, y + 12.0, 20.0, 20.0, 5.0, if hov { accent.alpha(0.85) } else { accent });
            self.gfx.glyph('\u{E73E}', x + 10.0, y + 22.0, 12.0, WHITE);
        } else {
            self.gfx.fill_rrect(x, y + 12.0, 20.0, 20.0, 5.0, fg.alpha(if hov { 0.5 } else { 0.35 }));
            let (bg, card, ..) = self.colors();
            let _ = bg;
            self.gfx.fill_rrect(x + 1.5, y + 13.5, 17.0, 17.0, 4.0, card);
        }
        self.gfx.text(label, x + 32.0, y, w - 40.0, 44.0, 14.5, 400, Align::Left, Fam::Text, fg, true);
        self.zones.push((x - 4.0, y, w, 44.0, act));
    }

    fn button(&mut self, x: f32, y: f32, w: f32, label: &str, primary: bool, act: Act) {
        let (_, _, fg, _, accent) = self.colors();
        let hov = self.hover == Some(act);
        let danger = Color::hex(0xE5484D);
        let base = if primary { if self.uninstall { danger } else { accent } } else { fg.alpha(if hov { 0.14 } else { 0.08 }) };
        let fill = if primary && hov { base.alpha(0.85) } else { base };
        self.gfx.fill_rrect(x, y, w, 38.0, 9.0, fill);
        self.gfx.text(label, x, y, w, 38.0, 14.5, 600, Align::Center, Fam::Text, if primary { WHITE } else { fg }, false);
        self.zones.push((x, y, w, 38.0, act));
    }

    fn paint(&mut self, hdc: HDC) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        let (cw, ch) = (r.right.max(1), r.bottom.max(1));
        if self.surf.as_ref().map(|s| (s.w, s.h)) != Some((cw, ch)) {
            self.surf = Surface::new(self.screen, cw, ch);
        }
        let Some(dc) = self.surf.as_ref().map(|s| s.dc) else { return };
        let (w, h) = (cw as f32 / self.s, ch as f32 / self.s);
        let (bg, card, fg, fg2, accent) = self.colors();
        self.zones.clear();
        unsafe {
            if self.gfx.rt.BindDC(dc, &RECT { left: 0, top: 0, right: cw, bottom: ch }).is_err() {
                return;
            }
            self.gfx.rt.BeginDraw();
            self.gfx.rt.SetTransform(&Matrix3x2 { M11: self.s, M12: 0.0, M21: 0.0, M22: self.s, M31: 0.0, M32: 0.0 });
            self.gfx.fill_rrect(-1.0, -1.0, w + 2.0, h + 2.0, 0.0, bg);

            let title = if self.uninstall { "Uninstall Frost Widgets" } else { "Frost Widgets" };
            self.gfx.text(title, 124.0, 34.0, w - 150.0, 34.0, 26.0, 700, Align::Left, Fam::Display, fg, true);
            let sub = format!("Version {VERSION}  \u{B7}  Glass desktop widgets for Windows");
            self.gfx.text(&sub, 125.0, 68.0, w - 150.0, 20.0, 13.0, 400, Align::Left, Fam::Text, fg2, true);

            let (bx, by) = (w - 32.0, h - 32.0 - 38.0);
            match self.stage.clone() {
                Stage::Ready => {
                    if self.uninstall {
                        let t = "This removes the app, its shortcuts and its startup entry.";
                        self.gfx.text(t, 32.0, 130.0, w - 64.0, 22.0, 14.0, 400, Align::Left, Fam::Text, fg, true);
                        self.gfx.fill_rrect(32.0, 170.0, w - 64.0, 44.0, 10.0, card);
                        self.checkbox(48.0, 170.0, w - 80.0, "Also delete my settings and reminders", self.opts.remove_data, Act::RemoveData);
                        self.button(bx - 140.0, by, 140.0, "Uninstall", true, Act::Primary);
                    } else {
                        if self.update {
                            let t = "An earlier version is installed. It will be updated; your settings stay.";
                            self.gfx.text(t, 125.0, 90.0, w - 150.0, 20.0, 12.5, 600, Align::Left, Fam::Text, accent, true);
                        }
                        self.gfx.fill_rrect(32.0, 128.0, w - 64.0, 132.0, 10.0, card);
                        let o = self.opts;
                        self.checkbox(48.0, 128.0, w - 80.0, "Start Frost Widgets when I sign in to Windows", o.autostart, Act::Autostart);
                        self.checkbox(48.0, 172.0, w - 80.0, "Create a desktop shortcut", o.desktop, Act::Desktop);
                        self.checkbox(48.0, 216.0, w - 80.0, "Open Frost Widgets when setup finishes", o.launch, Act::Launch);
                        let loc = format!("Installs just for you, no admin rights needed: {}", install_dir().display());
                        self.gfx.text(&loc, 32.0, 268.0, w - 64.0, 20.0, 12.0, 400, Align::Left, Fam::Text, fg2, true);
                        self.button(bx - 140.0, by, 140.0, if self.update { "Update" } else { "Install" }, true, Act::Primary);
                    }
                    self.button(bx - 140.0 - 12.0 - 110.0, by, 110.0, "Cancel", false, Act::Cancel);
                }
                Stage::Working => {
                    let t = if self.uninstall { "Removing Frost Widgets\u{2026}" } else { "Installing Frost Widgets\u{2026}" };
                    self.gfx.text(t, 32.0, 150.0, w - 64.0, 30.0, 17.0, 600, Align::Left, Fam::Text, fg, false);
                    self.gfx.fill_rrect(32.0, 192.0, w - 64.0, 6.0, 3.0, fg.alpha(0.12));
                    self.gfx.fill_rrect(32.0, 192.0, (w - 64.0) * 0.6, 6.0, 3.0, accent);
                }
                Stage::Done => {
                    let green = Color::hex(0x30D158);
                    self.gfx.fill_circle(52.0, 160.0, 18.0, green);
                    self.gfx.glyph('\u{E73E}', 52.0, 160.0, 16.0, WHITE);
                    let (t1, t2) = if self.uninstall {
                        ("Frost Widgets was removed", "Thanks for trying it.")
                    } else {
                        ("Frost Widgets is installed", "Find it in the Start menu. Right-click any widget or the tray icon for settings.")
                    };
                    self.gfx.text(t1, 84.0, 146.0, w - 116.0, 28.0, 18.0, 700, Align::Left, Fam::Text, fg, true);
                    self.gfx.text(t2, 84.0, 176.0, w - 116.0, 20.0, 13.0, 400, Align::Left, Fam::Text, fg2, true);
                    self.button(bx - 140.0, by, 140.0, if self.uninstall { "Close" } else { "Finish" }, true, Act::Primary);
                }
                Stage::Failed(msg) => {
                    let red = Color::hex(0xE5484D);
                    self.gfx.fill_circle(52.0, 160.0, 18.0, red);
                    self.gfx.glyph('\u{E711}', 52.0, 160.0, 14.0, WHITE);
                    self.gfx.text("Something went wrong", 84.0, 146.0, w - 116.0, 28.0, 18.0, 700, Align::Left, Fam::Text, fg, true);
                    self.gfx.text(&msg, 84.0, 176.0, w - 116.0, 20.0, 13.0, 400, Align::Left, Fam::Text, fg2, true);
                    self.button(bx - 140.0, by, 140.0, "Close", true, Act::Primary);
                }
            }
            let _ = self.gfx.rt.EndDraw(None, None);
            let s = self.s;
            let _ = DrawIconEx(dc, (32.0 * s) as i32, (28.0 * s) as i32, self.icon, (72.0 * s) as i32, (72.0 * s) as i32, 0, None, DI_NORMAL);
            let _ = BitBlt(hdc, 0, 0, cw, ch, dc, 0, 0, SRCCOPY);
        }
    }

    fn hit(&self, x: f32, y: f32) -> Option<Act> {
        self.zones.iter().rev().find(|z| x >= z.0 && x <= z.0 + z.2 && y >= z.1 && y <= z.1 + z.3).map(|z| z.4)
    }

    fn redraw(&self) {
        unsafe {
            let _ = InvalidateRect(self.hwnd, None, false);
        }
    }
}

/// Runs the install/uninstall after the "working" screen has been painted.
const WM_APP_RUN: u32 = WM_APP + 1;

fn click(act: Act) {
    let mut run = false;
    let mut close = false;
    with_ui(|u| {
        match act {
            Act::Autostart => u.opts.autostart = !u.opts.autostart,
            Act::Desktop => u.opts.desktop = !u.opts.desktop,
            Act::Launch => u.opts.launch = !u.opts.launch,
            Act::RemoveData => u.opts.remove_data = !u.opts.remove_data,
            Act::Cancel => close = true,
            Act::Primary => match u.stage {
                Stage::Ready => {
                    u.stage = Stage::Working;
                    run = true;
                }
                Stage::Done => {
                    if !u.uninstall && u.opts.launch {
                        launch_app();
                    }
                    if u.uninstall {
                        schedule_folder_removal();
                    }
                    close = true;
                }
                _ => close = true,
            },
        }
        u.redraw();
        if run {
            unsafe {
                let _ = PostMessageW(u.hwnd, WM_APP_RUN, WPARAM(0), LPARAM(0));
            }
        }
        if close {
            unsafe {
                let _ = DestroyWindow(u.hwnd);
            }
        }
    });
}

fn run_job() {
    let job = with_ui(|u| (u.uninstall, u.opts));
    let Some((uninstall, opts)) = job else { return };
    let r = if uninstall { do_uninstall(&opts) } else { do_install(&opts) };
    with_ui(|u| {
        u.stage = match r {
            Ok(()) => Stage::Done,
            Err(e) => Stage::Failed(e),
        };
        u.redraw();
    });
}

fn xy(lp: LPARAM) -> (i32, i32) {
    ((lp.0 & 0xFFFF) as i16 as i32, ((lp.0 >> 16) & 0xFFFF) as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            with_ui(|u| u.paint(hdc));
            let _ = EndPaint(hwnd, &ps);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_MOUSEMOVE => {
            let (x, y) = xy(lp);
            with_ui(|u| {
                let h = u.hit(x as f32 / u.s, y as f32 / u.s);
                if h != u.hover {
                    u.hover = h;
                    u.redraw();
                }
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            let (x, y) = xy(lp);
            if let Some(Some(a)) = with_ui(|u| u.hit(x as f32 / u.s, y as f32 / u.s)) {
                click(a);
            }
            LRESULT(0)
        }
        WM_SETCURSOR if (lp.0 & 0xFFFF) as u32 == HTCLIENT => {
            if with_ui(|u| u.hover.is_some()).unwrap_or(false) {
                SetCursor(LoadCursorW(None, IDC_HAND).unwrap_or_default());
                return LRESULT(1);
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_APP_RUN => {
            run_job();
            LRESULT(0)
        }
        WM_CLOSE => {
            // Don't allow closing in the middle of installing.
            if with_ui(|u| u.stage == Stage::Working).unwrap_or(false) {
                return LRESULT(0);
            }
            if with_ui(|u| u.uninstall && u.stage == Stage::Done).unwrap_or(false) {
                schedule_folder_removal();
            }
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DPICHANGED => {
            let r = *(lp.0 as *const RECT);
            with_ui(|u| u.s = (wp.0 & 0xFFFF) as f32 / 96.0);
            let _ = SetWindowPos(hwnd, None, r.left, r.top, r.right - r.left, r.bottom - r.top, SWP_NOZORDER | SWP_NOACTIVATE);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().map(|a| a.to_ascii_lowercase()).collect();
    let has = |f: &str| args.iter().any(|a| a == f || a == &f.replace("--", "/"));
    let uninstall = has("--uninstall");
    let silent = has("--silent") || args.iter().any(|a| a == "/s");
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let mut opts = Opts { autostart: true, desktop: false, launch: true, remove_data: false };
    if installed() {
        // Updating: keep the choices made at first install.
        opts.autostart = util::reg_string(HKEY_CURRENT_USER, RUN_KEY, APP_NAME).is_some();
        opts.desktop = desktop_link().map(|l| l.exists()).unwrap_or(false);
    }

    if silent {
        if uninstall {
            let _ = do_uninstall(&opts);
            schedule_folder_removal();
        } else if do_install(&opts).is_ok() {
            launch_app();
        }
        return;
    }

    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).unwrap_or_default().into();
        let cursor = LoadCursorW(None, IDC_ARROW).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            lpszClassName: w!("FrostSetup"),
            hCursor: cursor,
            hIcon: LoadIconW(hinst, PCWSTR(1 as *const u16)).unwrap_or_default(),
            ..Default::default()
        };
        RegisterClassW(&wc);

        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let hmon = MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let s = dx as f32 / 96.0;
        let mut mi = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hmon, &mut mi);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let mut r = RECT { left: 0, top: 0, right: (W0 * s) as i32, bottom: (H0 * s) as i32 };
        let _ = AdjustWindowRectExForDpi(&mut r, style, false, WINDOW_EX_STYLE::default(), dx);
        let (ww, wh) = (r.right - r.left, r.bottom - r.top);
        let work = mi.rcWork;
        let x = work.left + ((work.right - work.left) - ww) / 2;
        let y = work.top + ((work.bottom - work.top) - wh) / 2;
        let title = if uninstall { w!("Uninstall Frost Widgets") } else { w!("Frost Widgets Setup") };
        let Ok(hwnd) = CreateWindowExW(WINDOW_EX_STYLE::default(), w!("FrostSetup"), title, style, x, y, ww, wh, None, None, hinst, None) else {
            return;
        };
        let dark = util::reg_dword(
            HKEY_CURRENT_USER,
            "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize",
            "AppsUseLightTheme",
        ) == Some(0);
        let d = BOOL::from(dark);
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(20), &d as *const _ as *const _, 4);
        let bgc: u32 = if dark { 0x0022_1E1E } else { 0x00F8_F6F6 };
        let _ = DwmSetWindowAttribute(hwnd, DWMWINDOWATTRIBUTE(35), &bgc as *const _ as *const _, 4);
        let s = GetDpiForWindow(hwnd) as f32 / 96.0;
        let Ok(gfx) = Gfx::new() else { return };
        let icon = match LoadImageW(hinst, PCWSTR(1 as *const u16), IMAGE_ICON, (72.0 * s) as i32, (72.0 * s) as i32, LR_DEFAULTCOLOR) {
            Ok(h) => HICON(h.0),
            Err(_) => gfx::make_icon((72.0 * s) as i32),
        };
        let ui = Ui {
            hwnd,
            gfx,
            screen: GetDC(None),
            surf: None,
            s,
            uninstall,
            update: !uninstall && installed(),
            opts,
            stage: Stage::Ready,
            hover: None,
            zones: Vec::new(),
            dark,
            icon,
        };
        UI.with(|u| *u.borrow_mut() = Some(ui));
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
