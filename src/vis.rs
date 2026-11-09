//! Audio visualizer: captures what the PC is playing (WASAPI loopback, no
//! driver or permission needed), runs a small FFT and publishes 16 band levels.
//! The thread only exists in "active" mode while music plays and the widget is
//! visible; otherwise it sleeps (parked) and holds no audio resources.
use std::f32::consts::PI;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::Thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const BANDS: usize = 16;
const N: usize = 1024;

pub struct Vis {
    pub levels: Arc<Mutex<[f32; BANDS]>>,
    active: Arc<AtomicBool>,
    thread: Option<Thread>,
}

impl Vis {
    pub fn set_active(&self, on: bool) {
        if self.active.swap(on, Ordering::Relaxed) != on {
            if let Some(t) = &self.thread {
                t.unpark();
            }
        }
    }
    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }
}

pub fn start(hwnd: isize, msg: u32) -> Vis {
    let levels = Arc::new(Mutex::new([0.0; BANDS]));
    let active = Arc::new(AtomicBool::new(false));
    let (l, a) = (levels.clone(), active.clone());
    let thread = std::thread::Builder::new()
        .name("visualizer".into())
        .stack_size(256 * 1024)
        .spawn(move || run(l, a, hwnd, msg))
        .ok()
        .map(|h| h.thread().clone());
    Vis { levels, active, thread }
}

fn post(hwnd: isize, msg: u32) {
    unsafe {
        let _ = PostMessageW(HWND(hwnd as *mut _), msg, WPARAM(0), LPARAM(0));
    }
}

fn run(levels: Arc<Mutex<[f32; BANDS]>>, active: Arc<AtomicBool>, hwnd: isize, msg: u32) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let mut an = Analyzer::new();
    loop {
        while !active.load(Ordering::Relaxed) {
            std::thread::park();
        }
        let Ok(mut cap) = Capture::new() else {
            std::thread::sleep(Duration::from_secs(2));
            continue;
        };
        an.reset(cap.rate);
        let mut last_audio = Instant::now();
        let mut was_zero = false;
        while active.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(33));
            match cap.read(&mut an) {
                Ok(true) => last_audio = Instant::now(),
                Ok(false) => {
                    // Silence for a while: the default device may have changed. Re-open it.
                    if last_audio.elapsed() > Duration::from_secs(3) {
                        break;
                    }
                }
                Err(_) => break,
            }
            let bands = an.bands();
            let zero = bands.iter().all(|&b| b < 0.01);
            if let Ok(mut g) = levels.lock() {
                *g = bands;
            }
            if !(zero && was_zero) {
                post(hwnd, msg);
            }
            was_zero = zero;
        }
        drop(cap);
        an.clear();
        if let Ok(mut g) = levels.lock() {
            *g = [0.0; BANDS];
        }
        post(hwnd, msg);
    }
}

struct Capture {
    client: IAudioClient,
    cc: IAudioCaptureClient,
    channels: usize,
    bits: u16,
    rate: f32,
}

impl Capture {
    fn new() -> windows::core::Result<Self> {
        unsafe {
            let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let dev = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
            let fmt = client.GetMixFormat()?;
            let f = *fmt;
            let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 1_000_000, 0, fmt, None);
            CoTaskMemFree(Some(fmt as *const _));
            r?;
            let cc: IAudioCaptureClient = client.GetService()?;
            client.Start()?;
            Ok(Capture { client, cc, channels: f.nChannels.max(1) as usize, bits: f.wBitsPerSample, rate: f.nSamplesPerSec as f32 })
        }
    }

    /// Pull all pending audio into the analyzer. Ok(true) if any non-silent audio arrived.
    fn read(&mut self, an: &mut Analyzer) -> windows::core::Result<bool> {
        let mut got = false;
        unsafe {
            loop {
                let n = self.cc.GetNextPacketSize()?;
                if n == 0 {
                    break;
                }
                let mut data = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                self.cc.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let silent = flags & (AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0 || data.is_null();
                for i in 0..frames as usize {
                    let mut s = 0.0f32;
                    if !silent {
                        for c in 0..self.channels {
                            let k = i * self.channels + c;
                            s += match self.bits {
                                32 => *(data as *const f32).add(k),
                                16 => *(data as *const i16).add(k) as f32 / 32768.0,
                                _ => 0.0,
                            };
                        }
                        s /= self.channels as f32;
                    }
                    an.push(s);
                }
                if !silent && frames > 0 {
                    got = true;
                }
                self.cc.ReleaseBuffer(frames)?;
            }
        }
        Ok(got)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// Diagnostics: open the loopback capture and count reads with audio for `ms` milliseconds.
pub fn probe(ms: u64) -> String {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    match Capture::new() {
        Err(e) => format!("capture failed: {e}"),
        Ok(mut cap) => {
            let mut an = Analyzer::new();
            an.reset(cap.rate);
            let t = Instant::now();
            let (mut reads, mut audio) = (0, 0);
            while t.elapsed() < Duration::from_millis(ms) {
                std::thread::sleep(Duration::from_millis(33));
                reads += 1;
                match cap.read(&mut an) {
                    Ok(true) => audio += 1,
                    Ok(false) => {}
                    Err(e) => return format!("read failed: {e}"),
                }
            }
            format!(
                "capture ok: {} Hz, {} ch, {} bit; {reads} reads, {audio} with audio",
                cap.rate, cap.channels, cap.bits
            )
        }
    }
}

// ------------------------------------------------------------------ analysis

pub struct Analyzer {
    ring: Vec<f32>,
    pos: usize,
    window: Vec<f32>,
    edges: [usize; BANDS + 1],
    smooth: [f32; BANDS],
}

impl Analyzer {
    pub fn new() -> Self {
        let window = (0..N).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / (N - 1) as f32).cos()).collect();
        let mut a = Analyzer { ring: vec![0.0; N], pos: 0, window, edges: [0; BANDS + 1], smooth: [0.0; BANDS] };
        a.reset(48000.0);
        a
    }

    /// Log-spaced bands from 45 Hz to 14 kHz.
    pub fn reset(&mut self, rate: f32) {
        let (lo, hi) = (45.0f32, 14000.0f32.min(rate / 2.0 - 100.0));
        let mut prev = 0;
        for b in 0..=BANDS {
            let f = lo * (hi / lo).powf(b as f32 / BANDS as f32);
            let mut k = (f * N as f32 / rate).round() as usize;
            if b > 0 && k <= prev {
                k = prev + 1;
            }
            self.edges[b] = k.min(N / 2);
            prev = self.edges[b];
        }
    }

    pub fn clear(&mut self) {
        self.ring.iter_mut().for_each(|x| *x = 0.0);
        self.smooth = [0.0; BANDS];
    }

    pub fn push(&mut self, s: f32) {
        self.ring[self.pos] = s;
        self.pos = (self.pos + 1) % N;
    }

    pub fn bands(&mut self) -> [f32; BANDS] {
        let mut re = vec![0.0f32; N];
        let mut im = vec![0.0f32; N];
        for i in 0..N {
            re[i] = self.ring[(self.pos + i) % N] * self.window[i];
        }
        fft(&mut re, &mut im);
        let mut out = [0.0f32; BANDS];
        for b in 0..BANDS {
            let (k0, k1) = (self.edges[b].max(1), self.edges[b + 1].max(self.edges[b] + 1));
            let mut peak = 0.0f32;
            for k in k0..k1.min(N / 2) {
                peak = peak.max((re[k] * re[k] + im[k] * im[k]).sqrt());
            }
            // A full-scale sine peaks near N/4 with a Hann window.
            let db = 20.0 * (peak / (N as f32 / 4.0) + 1e-9).log10() + b as f32 * 0.7;
            let v = ((db + 58.0) / 50.0).clamp(0.0, 1.0);
            let s = &mut self.smooth[b];
            *s = if v > *s { *s * 0.35 + v * 0.65 } else { *s * 0.8 + v * 0.2 };
            out[b] = *s;
        }
        out
    }
}

/// In-place iterative radix-2 FFT.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_lights_the_right_band() {
        let mut a = Analyzer::new();
        a.reset(48000.0);
        for i in 0..N * 4 {
            a.push(0.5 * (2.0 * PI * 1000.0 * i as f32 / 48000.0).sin());
        }
        let mut b = [0.0; BANDS];
        for _ in 0..10 {
            b = a.bands();
        }
        let hot = (0..BANDS).max_by(|&x, &y| b[x].total_cmp(&b[y])).unwrap();
        let f_lo = 45.0 * (14000.0f32 / 45.0).powf(hot as f32 / BANDS as f32);
        let f_hi = 45.0 * (14000.0f32 / 45.0).powf((hot + 1) as f32 / BANDS as f32);
        assert!(f_lo <= 1100.0 && f_hi >= 900.0, "1 kHz landed in band {hot} ({f_lo}-{f_hi} Hz): {b:?}");
        assert!(b[hot] > 0.6, "level too low: {b:?}");
        assert!(b[0] < 0.3 && b[BANDS - 1] < 0.4, "leakage too high: {b:?}");
    }

    #[test]
    fn silence_is_flat() {
        let mut a = Analyzer::new();
        for _ in 0..N {
            a.push(0.0);
        }
        assert!(a.bands().iter().all(|&v| v < 0.01));
    }
}
