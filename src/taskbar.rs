//! Where the taskbar style of Now playing goes.
//!
//! Icons centred (Windows 11 default): on the left of the taskbar, or just right of
//! whatever already sits there (the Widgets/weather button). Icons on the left
//! (Windows 10, or Windows 11 set to left): beside the tray, next to the clock.
//!
//! The Windows 11 taskbar is XAML with no child windows, so its pieces are read with
//! UI Automation. That costs a few cross-process calls, so it's done rarely and cached;
//! the cheap part (the taskbar's own rectangle) is checked every second.
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::util;

/// What UI Automation found on the taskbar (screen px).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Probe {
    /// The taskbar rectangle this was measured against.
    pub bar: RECT,
    /// Left edge of the Start button (first centred icon).
    pub start_left: Option<i32>,
    /// Right edge of anything left of the Start button (e.g. the Widgets button).
    pub left_end: Option<i32>,
    /// Left edge of the tray (hidden icons arrow, network, clock...).
    pub tray_left: Option<i32>,
}

pub fn tray_hwnd() -> HWND {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), None).unwrap_or_default() }
}

/// Taskbar rectangle on the main screen, if there is one.
pub fn bar_rect() -> Option<RECT> {
    let h = tray_hwnd();
    if h.0.is_null() {
        return None;
    }
    let mut r = RECT::default();
    unsafe { GetWindowRect(h, &mut r).ok()? };
    (r.right > r.left && r.bottom > r.top).then_some(r)
}

/// Does the alignment setting say the icons are centred? Windows 10 never; Windows 11
/// unless set to left. Cheap (one registry read), so it's checked every second.
pub fn centered_setting() -> bool {
    static WIN11: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let win11 = *WIN11.get_or_init(|| {
        util::reg_string(HKEY_LOCAL_MACHINE, "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", "CurrentBuildNumber")
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(22000)
            >= 22000
    });
    win11 && util::reg_dword(HKEY_CURRENT_USER, "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Advanced", "TaskbarAl") != Some(0)
}

/// Are the taskbar icons centred? Trust where the Start button really is (the setting
/// can lag, and taskbar tweakers move the icons without touching it); fall back to the
/// setting when Start wasn't found.
pub fn centered(p: &Probe) -> bool {
    let bh = p.bar.bottom - p.bar.top;
    match p.start_left {
        // Left-aligned, Start sits right at the taskbar's left edge.
        Some(s) if bh > 0 => s - p.bar.left > bh,
        _ => centered_setting(),
    }
}

/// Read the taskbar's buttons. Needs COM on this thread.
pub fn probe() -> Probe {
    let bar = bar_rect().unwrap_or_default();
    let mut p = Probe { bar, ..Default::default() };
    if bar.right <= bar.left {
        return p;
    }
    let _ = unsafe { read_buttons(&mut p) };
    if p.tray_left.is_none() {
        // Windows 10 (and a fallback): the classic tray window still exists.
        unsafe {
            if let Ok(t) = FindWindowExW(tray_hwnd(), None, w!("TrayNotifyWnd"), None) {
                let mut r = RECT::default();
                if GetWindowRect(t, &mut r).is_ok() && r.right > r.left {
                    p.tray_left = Some(r.left);
                }
            }
        }
    }
    p
}

unsafe fn read_buttons(p: &mut Probe) -> Result<()> {
    let uia: IUIAutomation = CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)?;
    let root = uia.ElementFromHandle(tray_hwnd())?;
    let all = root.FindAll(TreeScope_Descendants, &uia.CreateTrueCondition()?)?;
    let bar = p.bar;
    let bw = bar.right - bar.left;
    let mut items: Vec<(String, RECT)> = Vec::new();
    // Never measure ourselves (should our strip ever show up in the taskbar's tree),
    // or the strip would keep stepping right of itself.
    let me = GetCurrentProcessId() as i32;
    for i in 0..all.Length()? {
        let Ok(e) = all.GetElement(i) else { continue };
        let frost = e.CurrentClassName().map(|c| c.to_string().starts_with("Frost")).unwrap_or(false);
        if frost || e.CurrentProcessId().map(|p| p == me).unwrap_or(false) {
            continue;
        }
        let Ok(r) = e.CurrentBoundingRectangle() else { continue };
        if r.right <= r.left || r.bottom <= r.top || e.CurrentIsOffscreen().map(|b| b.as_bool()).unwrap_or(false) {
            continue;
        }
        let id = e.CurrentAutomationId().map(|s| s.to_string()).unwrap_or_default();
        items.push((id, r));
    }
    p.start_left = items.iter().find(|(id, _)| id == "StartButton").map(|(_, r)| r.left);
    // With icons on the left, the Widgets (weather) button moves beside the tray.
    let widgets_right = p.start_left.is_some_and(|s| s - bar.left <= bar.bottom - bar.top);
    p.tray_left = items
        .iter()
        .filter(|(id, _)| {
            matches!(id.as_str(), "SystemTrayIcon" | "NotifyItemIcon" | "SystemTrayButton") || (widgets_right && id == "WidgetsButton")
        })
        .map(|(_, r)| r.left)
        .min();
    if let Some(sl) = p.start_left {
        // Anything small sitting left of Start: the Widgets button, a pinned search box...
        p.left_end = items
            .iter()
            .filter(|(id, r)| id != "StartButton" && r.right <= sl + 2 && r.left >= bar.left && (r.right - r.left) < bw / 3)
            .map(|(_, r)| r.right)
            .max();
    }
    Ok(())
}

/// Top-left for a `w` x `h` strip on the taskbar, or None when there's no usable taskbar.
pub fn spot(p: &Probe, bar: RECT, centered: bool, w: i32, h: i32, gap: i32) -> Option<(i32, i32)> {
    let (bw, bh) = (bar.right - bar.left, bar.bottom - bar.top);
    if bw <= 0 || bh <= 0 || bh > bw {
        // Missing or vertical taskbar: not supported, fall back to the card's spot.
        return None;
    }
    let y = bar.top + (bh - h) / 2;
    // Measurements are from where the taskbar was when probed; follow it if it slid
    // (auto-hide) without a fresh probe.
    let dx = bar.left - p.bar.left;
    let x = if centered {
        let left = p.left_end.map(|e| e + dx + gap).unwrap_or(bar.left + gap);
        // Never cover the Start button and the icons after it.
        let limit = p.start_left.map(|s| s + dx - gap - w).unwrap_or(i32::MAX);
        left.min(limit).max(bar.left + gap)
    } else {
        let tray = p.tray_left.map(|t| t + dx).unwrap_or(bar.right - bw / 6);
        (tray - gap - w).max(bar.left + gap)
    };
    Some((x, y))
}
