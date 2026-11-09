//! Messages widget source: the notifications currently in Windows' notification
//! centre (WhatsApp, Telegram, Mail, Teams...), read with the system
//! notification listener. Runs on its own thread only while the widget is shown.
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::Thread;
use std::time::Duration;

use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::Management::*;
use windows::UI::Notifications::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Msg {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub body: String,
    /// FILETIME (100 ns since 1601, UTC).
    pub time: i64,
}

#[derive(Clone, Default, PartialEq)]
pub struct MsgState {
    pub list: Vec<Msg>,
    /// Why messages can't be shown ("" when fine).
    pub status: &'static str,
}

pub struct Msgs {
    pub state: Arc<Mutex<MsgState>>,
    active: Arc<AtomicBool>,
    thread: Option<Thread>,
}

impl Msgs {
    pub fn set_active(&self, on: bool) {
        if self.active.swap(on, Ordering::Relaxed) != on {
            if let Some(t) = &self.thread {
                t.unpark();
            }
        }
    }
}

const KEEP: usize = 12;

pub fn start(hwnd: isize, msg: u32) -> Msgs {
    let state = Arc::new(Mutex::new(MsgState::default()));
    let active = Arc::new(AtomicBool::new(false));
    let (s, a) = (state.clone(), active.clone());
    let thread = std::thread::Builder::new()
        .name("messages".into())
        .stack_size(256 * 1024)
        .spawn(move || run(s, a, hwnd, msg))
        .ok()
        .map(|h| h.thread().clone());
    Msgs { state, active, thread }
}

fn run(state: Arc<Mutex<MsgState>>, active: Arc<AtomicBool>, hwnd: isize, msg: u32) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let me = std::thread::current();
    let mut listener: Option<UserNotificationListener> = None;
    let mut pushed = false;
    let mut last_hash = 0u64;
    loop {
        while !active.load(Ordering::Relaxed) {
            std::thread::park();
        }
        if listener.is_none() {
            listener = UserNotificationListener::Current().ok();
            // Change events wake us instantly; without them we poll.
            if let Some(l) = &listener {
                let t = me.clone();
                pushed = l
                    .NotificationChanged(&TypedEventHandler::new(move |_, _| {
                        t.unpark();
                        Ok(())
                    }))
                    .is_ok();
            }
        }
        let st = match &listener {
            None => MsgState { list: Vec::new(), status: "Notifications unavailable" },
            Some(l) => match l.GetAccessStatus() {
                Ok(UserNotificationListenerAccessStatus::Allowed) => MsgState { list: fetch(l).unwrap_or_default(), status: "" },
                _ => MsgState { list: Vec::new(), status: "Notification access is off" },
            },
        };
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (&st.list, st.status).hash(&mut h);
        let hash = h.finish();
        if hash != last_hash {
            last_hash = hash;
            if let Ok(mut g) = state.lock() {
                *g = st;
            }
            unsafe {
                let _ = PostMessageW(HWND(hwnd as *mut _), msg, WPARAM(0), LPARAM(0));
            }
        }
        std::thread::park_timeout(Duration::from_secs(if pushed { 30 } else { 4 }));
    }
}

fn fetch(l: &UserNotificationListener) -> windows::core::Result<Vec<Msg>> {
    let list = l.GetNotificationsAsync(NotificationKinds::Toast)?.get()?;
    let generic = KnownNotificationBindings::ToastGeneric()?;
    let mut out = Vec::new();
    for n in list {
        let app = n.AppInfo().and_then(|a| a.DisplayInfo()).and_then(|d| d.DisplayName()).map(|s| s.to_string()).unwrap_or_default();
        let time = n.CreationTime().map(|t| t.UniversalTime).unwrap_or(0);
        let mut texts = Vec::new();
        if let Ok(b) = n.Notification().and_then(|x| x.Visual()).and_then(|v| v.GetBinding(&generic)) {
            if let Ok(els) = b.GetTextElements() {
                for e in els {
                    if let Ok(t) = e.Text() {
                        let t = t.to_string();
                        let t = t.trim();
                        if !t.is_empty() {
                            texts.push(t.replace(['\r', '\n'], " "));
                        }
                    }
                }
            }
        }
        if texts.is_empty() {
            continue;
        }
        let title = texts.remove(0);
        out.push(Msg { id: n.Id().unwrap_or(0), app, title, body: texts.join(" "), time });
    }
    out.sort_by(|a, b| b.time.cmp(&a.time));
    out.truncate(KEEP);
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore]
    fn live_read() {
        use windows::UI::Notifications::Management::*;
        unsafe { let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED); }
        let l = UserNotificationListener::Current().unwrap();
        println!("access={:?}", l.GetAccessStatus().map(|s| s.0));
        let ev = l.NotificationChanged(&windows::Foundation::TypedEventHandler::new(|_, _| Ok(()))).is_ok();
        println!("events={ev} count={:?}", super::fetch(&l).map(|v| v.len()));
    }
}
