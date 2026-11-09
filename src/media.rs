//! Universal "now playing" via Windows' system media session API.
//! Works with any app that shows up in the Windows volume/media flyout:
//! Spotify, browsers (YouTube, SoundCloud, any site), VLC, Media Player, etc.
//! Runs on its own thread and is fully event-driven (no polling).
use std::hash::{Hash, Hasher};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Foundation::{EventRegistrationToken, TypedEventHandler};
use windows::Media::Control::*;
use windows::Storage::Streams::DataReader;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::WinRT::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

type Session = GlobalSystemMediaTransportControlsSession;

pub struct Art {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>, // premultiplied BGRA
    /// Vivid main color of the artwork (0xRRGGBB), used to tint the visualizer.
    pub tint: u32,
}

#[derive(Clone, Default)]
pub struct MediaState {
    pub has: bool,
    pub title: String,
    pub artist: String,
    pub app: String,
    pub playing: bool,
    pub pos: f64,
    pub dur: f64,
    pub updated: i64, // FILETIME (100ns since 1601, UTC)
    pub start: i64,   // timeline start in 100ns ticks (seek target offset)
    pub can_seek: bool,
    pub art: Option<Arc<Art>>,
    pub art_id: u64,
}

impl MediaState {
    /// Playback position now, extrapolated from the last reported position.
    pub fn position(&self, now_ft: i64) -> f64 {
        let mut p = self.pos;
        if self.playing && self.updated > 0 && now_ft > self.updated {
            p += (now_ft - self.updated) as f64 / 1e7;
        }
        if self.dur > 0.0 {
            p.clamp(0.0, self.dur)
        } else {
            p.max(0.0)
        }
    }
}

pub enum Cmd {
    Toggle,
    Next,
    Prev,
    Seek(f64), // seconds from the start of the track
}

enum Msg {
    Session,
    Refresh,
    Cmd(Cmd),
}

pub struct Media {
    tx: Sender<Msg>,
    pub state: Arc<Mutex<MediaState>>,
}

impl Media {
    pub fn cmd(&self, c: Cmd) {
        let _ = self.tx.send(Msg::Cmd(c));
    }
}

pub fn start(hwnd: isize, msg: u32) -> Media {
    let (tx, rx) = channel();
    let state = Arc::new(Mutex::new(MediaState::default()));
    let (st, t) = (state.clone(), tx.clone());
    std::thread::Builder::new()
        .name("media".into())
        .stack_size(256 * 1024)
        .spawn(move || unsafe {
            let _ = RoInitialize(RO_INIT_MULTITHREADED);
            let _ = run(rx, t, st, hwnd, msg);
        })
        .ok();
    Media { tx, state }
}

fn hook(s: &Session, tx: &Sender<Msg>) -> [EventRegistrationToken; 3] {
    let (a, b, c) = (tx.clone(), tx.clone(), tx.clone());
    [
        s.MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
            let _ = a.send(Msg::Refresh);
            Ok(())
        }))
        .unwrap_or_default(),
        s.PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
            let _ = b.send(Msg::Refresh);
            Ok(())
        }))
        .unwrap_or_default(),
        s.TimelinePropertiesChanged(&TypedEventHandler::new(move |_, _| {
            let _ = c.send(Msg::Refresh);
            Ok(())
        }))
        .unwrap_or_default(),
    ]
}

fn unhook(s: &Session, t: [EventRegistrationToken; 3]) {
    let _ = s.RemoveMediaPropertiesChanged(t[0]);
    let _ = s.RemovePlaybackInfoChanged(t[1]);
    let _ = s.RemoveTimelinePropertiesChanged(t[2]);
}

fn run(rx: Receiver<Msg>, tx: Sender<Msg>, state: Arc<Mutex<MediaState>>, hwnd: isize, wm: u32) -> windows::core::Result<()> {
    let mgr = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.get()?;
    let t = tx.clone();
    mgr.CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Msg::Refresh);
        Ok(())
    }))?;
    let t = tx.clone();
    mgr.SessionsChanged(&TypedEventHandler::new(move |_, _| {
        let _ = t.send(Msg::Session);
        Ok(())
    }))?;
    let wic: IWICImagingFactory = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };

    let _ = tx.send(Msg::Session);
    // Every session is watched, so a song starting anywhere is noticed; `pick` is the one shown.
    let mut all: Vec<(Session, [EventRegistrationToken; 3])> = Vec::new();
    let mut pick: Option<Session> = None;
    let mut art: Option<Arc<Art>> = None;
    let mut art_hash = 0u64;
    // Some players don't report the new position after a seek; hold our target until they do.
    let mut seek_hint: Option<(f64, i64)> = None;

    while let Ok(first) = rx.recv() {
        // Events arrive in bursts; coalesce them.
        let mut msgs = vec![first];
        std::thread::sleep(Duration::from_millis(60));
        while let Ok(m) = rx.try_recv() {
            msgs.push(m);
        }
        let mut session_changed = false;
        for m in msgs {
            match m {
                Msg::Session => session_changed = true,
                Msg::Refresh => {}
                Msg::Cmd(c) => {
                    if let Some(s) = &pick {
                        let _ = match c {
                            Cmd::Toggle => s.TryTogglePlayPauseAsync().and_then(|o| o.get()),
                            Cmd::Next => s.TrySkipNextAsync().and_then(|o| o.get()),
                            Cmd::Prev => s.TrySkipPreviousAsync().and_then(|o| o.get()),
                            Cmd::Seek(secs) => {
                                seek_hint = Some((secs, now_ft()));
                                let start = s.GetTimelineProperties().and_then(|t| t.StartTime()).map(|t| t.Duration).unwrap_or(0);
                                s.TryChangePlaybackPositionAsync(start + (secs.max(0.0) * 1e7) as i64).and_then(|o| o.get())
                            }
                        };
                    }
                }
            }
        }
        if session_changed {
            for (s, t) in all.drain(..) {
                unhook(&s, t);
            }
            if let Ok(list) = mgr.GetSessions() {
                for s in list {
                    let toks = hook(&s, &tx);
                    all.push((s, toks));
                }
            }
        }
        // Re-rank on every change: music that is playing always wins.
        let prev_id = pick.as_ref().map(app_id);
        let sys_id = mgr.GetCurrentSession().ok().map(|s| app_id(&s));
        let best = all
            .iter()
            .map(|(s, _)| {
                let id = app_id(s);
                // Ties go to what we already show, then to Windows' own choice.
                let tie = (prev_id.as_deref() == Some(id.as_str())) as i32 * 2 + (sys_id.as_deref() == Some(id.as_str())) as i32;
                (rank(s, &id) * 10 + tie, s)
            })
            .max_by_key(|(k, _)| *k)
            .map(|(_, s)| s.clone());
        let switched = best.as_ref().map(app_id) != prev_id;
        if switched {
            session_changed = true;
        }
        pick = best;
        let mut snap = match &pick {
            Some(s) => snapshot(s, &wic, &mut art, &mut art_hash),
            None => {
                art = None;
                art_hash = 0;
                MediaState::default()
            }
        };
        if session_changed {
            seek_hint = None;
        }
        if let Some((target, at)) = seek_hint {
            if snap.updated < at && now_ft() - at < 150_000_000 {
                snap.pos = target;
                snap.updated = at;
            } else {
                seek_hint = None;
            }
        }
        if let Ok(mut g) = state.lock() {
            *g = snap;
        }
        unsafe {
            let _ = PostMessageW(HWND(hwnd as *mut _), wm, WPARAM(0), LPARAM(0));
        }
    }
    Ok(())
}

fn app_id(s: &Session) -> String {
    s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default()
}

/// Apps that are music players whatever they report.
fn music_app(id: &str) -> bool {
    let l = id.to_lowercase();
    [
        "spotify", "applemusic", "zunemusic", "tidal", "deezer", "music.youtube", "youtubemusic", "amazonmusic", "foobar", "aimp",
        "musicbee", "winamp", "soundcloud", "itunes", "qobuz", "pandora", "audacious", "dopamine",
    ]
    .iter()
    .any(|k| l.contains(k))
}

/// Higher is better. Playing beats paused; then music beats other media; then
/// a proper song (title and artist) beats bare clips like muted autoplay videos.
fn rank(s: &Session, id: &str) -> i32 {
    let Ok(pb) = s.GetPlaybackInfo() else { return 0 };
    let playing = pb
        .PlaybackStatus()
        .map(|x| x == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
        .unwrap_or(false);
    let music = music_app(id)
        || pb.PlaybackType().and_then(|t| t.Value()).map(|t| t == windows::Media::MediaPlaybackType::Music).unwrap_or(false);
    let meta = s
        .TryGetMediaPropertiesAsync()
        .and_then(|o| o.get())
        .map(|p| {
            let t = p.Title().map(|h| !h.is_empty()).unwrap_or(false);
            let a = p.Artist().map(|h| !h.is_empty()).unwrap_or(false);
            t && a
        })
        .unwrap_or(false);
    playing as i32 * 4 + music as i32 * 2 + meta as i32
}

fn snapshot(s: &Session, wic: &IWICImagingFactory, art: &mut Option<Arc<Art>>, art_hash: &mut u64) -> MediaState {
    let mut st = MediaState { has: true, ..Default::default() };
    st.app = pretty_app(&s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default());
    if let Ok(p) = s.TryGetMediaPropertiesAsync().and_then(|o| o.get()) {
        st.title = p.Title().map(|h| h.to_string()).unwrap_or_default();
        st.artist = p.Artist().map(|h| h.to_string()).unwrap_or_default();
        if st.artist.is_empty() {
            st.artist = p.AlbumTitle().map(|h| h.to_string()).unwrap_or_default();
        }
        match p.Thumbnail().ok().and_then(|t| read_thumb(&t)) {
            Some(bytes) => {
                let mut h = std::collections::hash_map::DefaultHasher::new();
                bytes.hash(&mut h);
                let id = h.finish() | 1;
                if id != *art_hash {
                    *art = decode_square(wic, &bytes, 256).map(Arc::new);
                    *art_hash = if art.is_some() { id } else { 0 };
                }
            }
            None => {
                *art = None;
                *art_hash = 0;
            }
        }
    }
    if let Ok(pb) = s.GetPlaybackInfo() {
        st.playing = pb
            .PlaybackStatus()
            .map(|x| x == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
            .unwrap_or(false);
        st.can_seek = pb.Controls().and_then(|c| c.IsPlaybackPositionEnabled()).unwrap_or(false);
    }
    if let Ok(tl) = s.GetTimelineProperties() {
        let start = tl.StartTime().map(|t| t.Duration).unwrap_or(0);
        let end = tl.EndTime().map(|t| t.Duration).unwrap_or(0);
        st.pos = (tl.Position().map(|t| t.Duration).unwrap_or(0) - start) as f64 / 1e7;
        st.dur = (end - start).max(0) as f64 / 1e7;
        st.updated = tl.LastUpdatedTime().map(|t| t.UniversalTime).unwrap_or(0);
        st.start = start;
    }
    st.art = art.clone();
    st.art_id = *art_hash;
    st
}

fn read_thumb(t: &windows::Storage::Streams::IRandomAccessStreamReference) -> Option<Vec<u8>> {
    let stream = t.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 16 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.get().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

/// Decode any image, center-crop to a square and scale to `n`x`n` premultiplied BGRA.
fn decode_square(wic: &IWICImagingFactory, bytes: &[u8], n: u32) -> Option<Art> {
    unsafe {
        let stream = wic.CreateStream().ok()?;
        stream.InitializeFromMemory(bytes).ok()?;
        let dec = wic.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let side = w.min(h);
        let clip = wic.CreateBitmapClipper().ok()?;
        let rc = WICRect { X: ((w - side) / 2) as i32, Y: ((h - side) / 2) as i32, Width: side as i32, Height: side as i32 };
        clip.Initialize(&frame, &rc).ok()?;
        let sc = wic.CreateBitmapScaler().ok()?;
        sc.Initialize(&clip, n, n, WICBitmapInterpolationModeHighQualityCubic).ok()?;
        let cv = wic.CreateFormatConverter().ok()?;
        cv.Initialize(&sc, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .ok()?;
        let mut px = vec![0u8; (n * n * 4) as usize];
        cv.CopyPixels(std::ptr::null(), n * 4, &mut px).ok()?;
        let tint = vivid_color(&px);
        Some(Art { w: n, h: n, px, tint })
    }
}

fn pretty_app(id: &str) -> String {
    let l = id.to_lowercase();
    let known = [
        ("spotify", "Spotify"),
        ("msedge", "Edge"),
        ("chrome", "Chrome"),
        ("firefox", "Firefox"),
        ("308046b0af4a39cb", "Firefox"),
        ("brave", "Brave"),
        ("opera", "Opera"),
        ("vivaldi", "Vivaldi"),
        ("zunemusic", "Media Player"),
        ("applemusic", "Apple Music"),
        ("vlc", "VLC"),
        ("foobar", "foobar2000"),
        ("aimp", "AIMP"),
        ("musicbee", "MusicBee"),
        ("tidal", "TIDAL"),
        ("deezer", "Deezer"),
        ("youtube", "YouTube Music"),
    ];
    for (k, v) in known {
        if l.contains(k) {
            return v.to_string();
        }
    }
    let mut s = id.rsplit('!').next().unwrap_or(id).to_string();
    if let Some(i) = s.rfind(['\\', '/']) {
        s = s[i + 1..].to_string();
    }
    let s = s.trim_end_matches(".exe").trim_end_matches(".EXE");
    let s = s.split('_').next().unwrap_or(s);
    let s = s.rsplit('.').next().unwrap_or(s);
    if s.is_empty() { "Media".into() } else { s.to_string() }
}

/// Saturation-weighted average color, pushed bright and vivid so bars glow on glass.
fn vivid_color(px: &[u8]) -> u32 {
    let (mut r, mut g, mut b, mut wsum) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for p in px.chunks_exact(4).step_by(7) {
        let a = p[3] as f32 / 255.0;
        if a < 0.5 {
            continue;
        }
        let (pb, pg, pr) = (p[0] as f32 / 255.0 / a, p[1] as f32 / 255.0 / a, p[2] as f32 / 255.0 / a);
        let mx = pr.max(pg).max(pb);
        let mn = pr.min(pg).min(pb);
        let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
        let w = sat * sat * mx + 0.002;
        r += pr * w;
        g += pg * w;
        b += pb * w;
        wsum += w;
    }
    if wsum <= 0.0 {
        return 0xFFFFFF;
    }
    let (r, g, b) = (r / wsum, g / wsum, b / wsum);
    // To HSV, then lift saturation and brightness.
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    let d = mx - mn;
    if d < 0.06 {
        return 0xFFFFFF; // greyscale art: white bars
    }
    let h = if mx == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if mx == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } * 60.0;
    let s = (d / mx).clamp(0.5, 0.8);
    let v = 1.0f32;
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = match (h / 60.0) as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let to = |v: f32| (((v + m) * 255.0).round() as u32).min(255);
    (to(r1) << 16) | (to(g1) << 8) | to(b1)
}
/// Current UTC time as FILETIME ticks (same clock as the media timeline).
fn now_ft() -> i64 {
    let f = unsafe { windows::Win32::System::SystemInformation::GetSystemTimePreciseAsFileTime() };
    ((f.dwHighDateTime as i64) << 32) | f.dwLowDateTime as i64
}

#[cfg(test)]
mod tests {
    /// Run with a music session playing, then a video session started after it.
    #[test]
    #[ignore]
    fn live_pick() {
        unsafe {
            let _ = windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED);
        }
        let mgr = super::GlobalSystemMediaTransportControlsSessionManager::RequestAsync().unwrap().get().unwrap();
        for s in mgr.GetSessions().unwrap() {
            let id = super::app_id(&s);
            let ty = s.GetPlaybackInfo().and_then(|p| p.PlaybackType()).and_then(|t| t.Value()).map(|t| t.0).unwrap_or(-1);
            println!("session {id} type={ty} rank={}", super::rank(&s, &id));
        }
        let cur = mgr.GetCurrentSession().map(|s| super::app_id(&s)).unwrap_or_default();
        println!("windows current: {cur}");
        let m = super::start(0, 0);
        for i in 0..8 {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let s = m.state.lock().unwrap().clone();
            println!("t={}s pick: app={:?} has={} title_set={} playing={}", i + 1, s.app, s.has, !s.title.is_empty(), s.playing);
        }
    }
}
