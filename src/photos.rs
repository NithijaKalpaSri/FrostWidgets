//! Photos widget loader. A background thread lists a folder (and its
//! subfolders) and decodes one random picture at exactly the widget's pixel
//! size, cover-cropped and turned upright, so the UI thread never touches big files.
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

pub struct Photo {
    pub px: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// What the loader sends back: a photo, or why there is none.
pub enum Loaded {
    Photo(Photo),
    Empty(&'static str),
}

struct Req {
    dir: PathBuf,
    w: u32,
    h: u32,
}

pub struct Photos {
    tx: Sender<Req>,
}

impl Photos {
    /// Ask for a new random photo from `dir` at `w`x`h` pixels.
    pub fn next(&self, dir: &Path, w: u32, h: u32) {
        let _ = self.tx.send(Req { dir: dir.to_path_buf(), w: w.max(1), h: h.max(1) });
    }
}

/// The user's Pictures folder (the default).
pub fn default_dir() -> PathBuf {
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Pictures, KNOWN_FOLDER_FLAG(0), None) {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            if !s.is_empty() {
                return PathBuf::from(s);
            }
        }
    }
    PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()).join("Pictures")
}

pub fn start(hwnd: isize, msg: u32) -> Photos {
    let (tx, rx) = channel::<Req>();
    let _ = std::thread::Builder::new().name("photos".into()).stack_size(256 * 1024).spawn(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let wic: Option<IWICImagingFactory> = unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok() };
        let mut list: (PathBuf, Vec<PathBuf>) = (PathBuf::new(), Vec::new());
        let mut last = PathBuf::new();
        let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(7) | 1;
        while let Ok(mut req) = rx.recv() {
            // Only the newest request matters.
            while let Ok(r) = rx.try_recv() {
                req = r;
            }
            if list.0 != req.dir {
                let mut files = Vec::new();
                scan(&req.dir, 0, &mut files);
                list = (req.dir.clone(), files);
            }
            let out = match (&wic, list.1.is_empty()) {
                (None, _) => Loaded::Empty("Can't open pictures"),
                (_, true) => Loaded::Empty(if req.dir.is_dir() { "No photos in this folder" } else { "Folder not found" }),
                (Some(wic), false) => {
                    // A few tries in case some files are unreadable.
                    let mut got = Loaded::Empty("Can't open these photos");
                    for _ in 0..6 {
                        seed ^= seed << 13;
                        seed ^= seed >> 7;
                        seed ^= seed << 17;
                        let mut i = (seed % list.1.len() as u64) as usize;
                        if list.1.len() > 1 && list.1[i] == last {
                            i = (i + 1) % list.1.len();
                        }
                        let p = &list.1[i];
                        if let Some(px) = decode(wic, p, req.w, req.h) {
                            last = p.clone();
                            got = Loaded::Photo(Photo { px, w: req.w, h: req.h });
                            break;
                        }
                    }
                    got
                }
            };
            let b = Box::into_raw(Box::new(out));
            unsafe {
                if PostMessageW(HWND(hwnd as *mut _), msg, WPARAM(b as usize), LPARAM(0)).is_err() {
                    drop(Box::from_raw(b));
                }
            }
        }
    });
    Photos { tx }
}

const EXTS: [&str; 9] = ["jpg", "jpeg", "jfif", "png", "bmp", "gif", "webp", "tif", "tiff"];
const MAX_FILES: usize = 4000;

fn scan(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut subdirs = Vec::new();
    for e in rd.flatten() {
        if out.len() >= MAX_FILES {
            return;
        }
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            let hidden = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.'));
            if !hidden {
                subdirs.push(p);
            }
        } else if p.extension().and_then(|x| x.to_str()).is_some_and(|x| EXTS.iter().any(|e| x.eq_ignore_ascii_case(e))) {
            out.push(p);
        }
    }
    if depth < 3 {
        for d in subdirs {
            scan(&d, depth + 1, out);
        }
    }
}

/// EXIF orientation (1..8) of a frame, if any.
unsafe fn orientation(frame: &IWICBitmapFrameDecode) -> u16 {
    let Ok(q) = frame.GetMetadataQueryReader() else { return 1 };
    for name in [w!("/app1/ifd/{ushort=274}"), w!("/ifd/{ushort=274}")] {
        let mut v = windows::core::PROPVARIANT::default();
        if q.GetMetadataByName(name, &mut v).is_ok() {
            if let Ok(o) = u16::try_from(&v) {
                return o;
            }
        }
    }
    1
}

/// Decode `path` to exactly `tw`x`th` premultiplied BGRA, cover-cropped and upright.
fn decode(wic: &IWICImagingFactory, path: &Path, tw: u32, th: u32) -> Option<Vec<u8>> {
    unsafe {
        let p = HSTRING::from(path.as_os_str());
        let dec = wic.CreateDecoderFromFilename(&p, None, GENERIC_READ, WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let (mut iw, mut ih) = (0u32, 0u32);
        frame.GetSize(&mut iw, &mut ih).ok()?;
        if iw == 0 || ih == 0 {
            return None;
        }
        let o = orientation(&frame);
        let turned = (5..=8).contains(&o);
        let (ew, eh) = if turned { (ih, iw) } else { (iw, ih) };
        // Scale so the upright image covers the target, then rotate, then crop the middle.
        let s = (tw as f32 / ew as f32).max(th as f32 / eh as f32);
        let (sw, sh) = (((iw as f32 * s).ceil() as u32).max(1), ((ih as f32 * s).ceil() as u32).max(1));
        let sc = wic.CreateBitmapScaler().ok()?;
        sc.Initialize(&frame, sw, sh, WICBitmapInterpolationModeHighQualityCubic).ok()?;
        let src: IWICBitmapSource = if o > 1 {
            let opt = match o {
                2 => WICBitmapTransformFlipHorizontal,
                3 => WICBitmapTransformRotate180,
                4 => WICBitmapTransformFlipVertical,
                5 => WICBitmapTransformOptions(WICBitmapTransformRotate90.0 | WICBitmapTransformFlipHorizontal.0),
                6 => WICBitmapTransformRotate90,
                7 => WICBitmapTransformOptions(WICBitmapTransformRotate270.0 | WICBitmapTransformFlipHorizontal.0),
                _ => WICBitmapTransformRotate270,
            };
            let fr = wic.CreateBitmapFlipRotator().ok()?;
            fr.Initialize(&sc, opt).ok()?;
            fr.cast().ok()?
        } else {
            sc.cast().ok()?
        };
        let cv = wic.CreateFormatConverter().ok()?;
        cv.Initialize(&src, &GUID_WICPixelFormat32bppPBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom).ok()?;
        let (mut rw, mut rh) = (0u32, 0u32);
        cv.GetSize(&mut rw, &mut rh).ok()?;
        let (cw, ch) = (tw.min(rw), th.min(rh));
        let rc = WICRect { X: ((rw - cw) / 2) as i32, Y: ((rh - ch) / 2) as i32, Width: cw as i32, Height: ch as i32 };
        let mut buf = vec![0u8; (cw * ch * 4) as usize];
        cv.CopyPixels(&rc, cw * 4, &mut buf).ok()?;
        if (cw, ch) == (tw, th) {
            return Some(buf);
        }
        // Rounding left it a pixel short: pad by repeating the edge.
        let mut out = vec![0u8; (tw * th * 4) as usize];
        for y in 0..th {
            for x in 0..tw {
                let s = ((y.min(ch - 1) * cw + x.min(cw - 1)) * 4) as usize;
                let d = ((y * tw + x) * 4) as usize;
                out[d..d + 4].copy_from_slice(&buf[s..s + 4]);
            }
        }
        Some(out)
    }
}
