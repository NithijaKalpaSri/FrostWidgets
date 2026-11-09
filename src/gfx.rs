//! Rendering: Direct2D (software, no GPU device = low RAM) into GDI DIBs that feed
//! layered windows, plus the precomputed "glass" background built from a blurred
//! copy of the wallpaper (blurred once, so frosted glass costs zero CPU per frame).
use std::collections::HashMap;
use std::f32::consts::PI;
use std::path::Path;

use windows::core::*;
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::HKEY_CURRENT_USER;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::util::{reg_string, wide};

// ------------------------------------------------------------------ color

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn hex(h: u32) -> Self {
        Color {
            r: ((h >> 16) & 255) as f32 / 255.0,
            g: ((h >> 8) & 255) as f32 / 255.0,
            b: (h & 255) as f32 / 255.0,
            a: 1.0,
        }
    }
    /// Back to 0xRRGGBB.
    pub fn rgb(self) -> u32 {
        let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
        (c(self.r) << 16) | (c(self.g) << 8) | c(self.b)
    }
    pub fn alpha(self, a: f32) -> Self {
        Color { a: self.a * a, ..self }
    }
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('#');
        (s.len() == 6).then(|| u32::from_str_radix(s, 16).ok()).flatten().map(Color::hex)
    }
    fn d2d(self) -> D2D1_COLOR_F {
        D2D1_COLOR_F { r: self.r, g: self.g, b: self.b, a: self.a }
    }
}

pub const WHITE: Color = Color::hex(0xFFFFFF);

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fam {
    Display,
    Text,
    Icon,
}

fn v2(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

// ------------------------------------------------------------------ Gfx

type FmtKey = (Fam, u32, i32, Align, bool);

pub struct Gfx {
    pub d2d: ID2D1Factory1,
    pub dw: IDWriteFactory,
    pub rt: ID2D1DCRenderTarget,
    pub wic: IWICImagingFactory,
    brush: ID2D1SolidColorBrush,
    round: ID2D1StrokeStyle,
    fmts: HashMap<FmtKey, IDWriteTextFormat>,
    fam_display: Vec<u16>,
    fam_text: Vec<u16>,
    fam_icon: Vec<u16>,
}

impl Gfx {
    pub fn new() -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = d2d.CreateDCRenderTarget(&props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = rt.CreateSolidColorBrush(&WHITE.d2d(), None)?;
            let f0: ID2D1Factory = d2d.cast()?;
            let round = f0.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                },
                None,
            )?;
            let wic: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;

            let mut coll = None;
            dw.GetSystemFontCollection(&mut coll, false)?;
            let has = |n: &str| -> bool {
                let Some(c) = &coll else { return false };
                let w = wide(n);
                let (mut i, mut e) = (0u32, BOOL(0));
                c.FindFamilyName(PCWSTR(w.as_ptr()), &mut i, &mut e).is_ok() && e.as_bool()
            };
            let pick = |a: &str, b: &str| wide(if has(a) { a } else { b });
            Ok(Gfx {
                fam_display: pick("Segoe UI Variable Display", "Segoe UI"),
                fam_text: pick("Segoe UI Variable Text", "Segoe UI"),
                fam_icon: pick("Segoe Fluent Icons", "Segoe MDL2 Assets"),
                d2d,
                dw,
                rt,
                wic,
                brush,
                round,
                fmts: HashMap::new(),
            })
        }
    }

    fn fmt(&mut self, fam: Fam, size: f32, weight: i32, align: Align, ell: bool) -> IDWriteTextFormat {
        let key = (fam, (size * 100.0) as u32, weight, align, ell);
        if let Some(f) = self.fmts.get(&key) {
            return f.clone();
        }
        let name = match fam {
            Fam::Display => &self.fam_display,
            Fam::Text => &self.fam_text,
            Fam::Icon => &self.fam_icon,
        };
        unsafe {
            let f = self
                .dw
                .CreateTextFormat(
                    PCWSTR(name.as_ptr()),
                    None,
                    DWRITE_FONT_WEIGHT(weight),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )
                .expect("text format");
            let _ = f.SetTextAlignment(match align {
                Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
            });
            let _ = f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
            if ell {
                if let Ok(sign) = self.dw.CreateEllipsisTrimmingSign(&f) {
                    let t = DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    };
                    let _ = f.SetTrimming(&t, &sign);
                }
            }
            self.fmts.insert(key, f.clone());
            f
        }
    }

    fn set(&self, c: Color) {
        unsafe { self.brush.SetColor(&c.d2d()) }
    }

    /// Single line of text, vertically centered in the rect.
    #[allow(clippy::too_many_arguments)]
    pub fn text(&mut self, s: &str, x: f32, y: f32, w: f32, h: f32, size: f32, weight: i32, align: Align, fam: Fam, c: Color, ell: bool) {
        let f = self.fmt(fam, size, weight, align, ell);
        let t: Vec<u16> = s.encode_utf16().collect();
        self.set(c);
        unsafe {
            self.rt.DrawText(
                &t,
                &f,
                &D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    pub fn text_width(&mut self, s: &str, size: f32, weight: i32, fam: Fam) -> f32 {
        let f = self.fmt(fam, size, weight, Align::Left, false);
        let t: Vec<u16> = s.encode_utf16().collect();
        unsafe {
            let Ok(l) = self.dw.CreateTextLayout(&t, &f, 10000.0, 1000.0) else { return 0.0 };
            let mut m = DWRITE_TEXT_METRICS::default();
            if l.GetMetrics(&mut m).is_ok() {
                m.widthIncludingTrailingWhitespace
            } else {
                0.0
            }
        }
    }

    pub fn glyph(&mut self, ch: char, cx: f32, cy: f32, size: f32, c: Color) {
        let s = ch.to_string();
        self.text(&s, cx - size, cy - size, size * 2.0, size * 2.0, size, 400, Align::Center, Fam::Icon, c, false);
    }

    pub fn fill_circle(&self, cx: f32, cy: f32, r: f32, c: Color) {
        self.set(c);
        unsafe {
            self.rt.FillEllipse(&D2D1_ELLIPSE { point: v2(cx, cy), radiusX: r, radiusY: r }, &self.brush);
        }
    }

    pub fn stroke_circle(&self, cx: f32, cy: f32, r: f32, w: f32, c: Color) {
        self.set(c);
        unsafe {
            self.rt.DrawEllipse(&D2D1_ELLIPSE { point: v2(cx, cy), radiusX: r, radiusY: r }, &self.brush, w, &self.round);
        }
    }

    pub fn line(&self, x1: f32, y1: f32, x2: f32, y2: f32, w: f32, c: Color) {
        self.set(c);
        unsafe { self.rt.DrawLine(v2(x1, y1), v2(x2, y2), &self.brush, w, &self.round) }
    }

    pub fn fill_rrect(&self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Color) {
        self.set(c);
        let rr = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: r,
            radiusY: r,
        };
        unsafe { self.rt.FillRoundedRectangle(&rr, &self.brush) }
    }

    pub fn stroke_rrect(&self, x: f32, y: f32, w: f32, h: f32, r: f32, sw: f32, c: Color) {
        self.set(c);
        let rr = D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
            radiusX: r,
            radiusY: r,
        };
        unsafe { self.rt.DrawRoundedRectangle(&rr, &self.brush, sw, None) }
    }

    /// Rounded rect with a top-to-bottom gradient (glass sheen).
    pub fn fill_rrect_vgrad(&self, x: f32, y: f32, w: f32, h: f32, r: f32, top: Color, bottom: Color) {
        unsafe {
            let stops = [D2D1_GRADIENT_STOP { position: 0.0, color: top.d2d() }, D2D1_GRADIENT_STOP { position: 1.0, color: bottom.d2d() }];
            let Ok(coll) = self.rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP) else { return };
            let props = D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: v2(x, y), endPoint: v2(x, y + h) };
            let Ok(br) = self.rt.CreateLinearGradientBrush(&props, None, &coll) else { return };
            let rr = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
                radiusX: r,
                radiusY: r,
            };
            self.rt.FillRoundedRectangle(&rr, &br);
        }
    }

    /// Progress ring starting at 12 o'clock, clockwise.
    pub fn arc(&self, cx: f32, cy: f32, r: f32, w: f32, frac: f32, c: Color) {
        let frac = frac.clamp(0.0, 1.0);
        if frac < 0.002 {
            return;
        }
        if frac > 0.998 {
            return self.stroke_circle(cx, cy, r, w, c);
        }
        let a0 = -PI / 2.0;
        let a1 = a0 + frac * 2.0 * PI;
        unsafe {
            let Ok(g) = self.d2d.CreatePathGeometry() else { return };
            let Ok(sink) = g.Open() else { return };
            sink.BeginFigure(v2(cx + r * a0.cos(), cy + r * a0.sin()), D2D1_FIGURE_BEGIN_HOLLOW);
            sink.AddArc(&D2D1_ARC_SEGMENT {
                point: v2(cx + r * a1.cos(), cy + r * a1.sin()),
                size: D2D_SIZE_F { width: r, height: r },
                rotationAngle: 0.0,
                sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                arcSize: if frac > 0.5 { D2D1_ARC_SIZE_LARGE } else { D2D1_ARC_SIZE_SMALL },
            });
            sink.EndFigure(D2D1_FIGURE_END_OPEN);
            let _ = sink.Close();
            self.set(c);
            self.rt.DrawGeometry(&g, &self.brush, w, &self.round);
        }
    }

    pub fn fill_poly(&self, pts: &[(f32, f32)], c: Color) {
        if pts.len() < 3 {
            return;
        }
        unsafe {
            let Ok(g) = self.d2d.CreatePathGeometry() else { return };
            let Ok(sink) = g.Open() else { return };
            sink.BeginFigure(v2(pts[0].0, pts[0].1), D2D1_FIGURE_BEGIN_FILLED);
            for p in &pts[1..] {
                sink.AddLine(v2(p.0, p.1));
            }
            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
            let _ = sink.Close();
            self.set(c);
            self.rt.FillGeometry(&g, &self.brush, None);
        }
    }

    /// Circle minus an offset circle (moon shape).
    pub fn fill_crescent(&self, cx: f32, cy: f32, r: f32, ox: f32, oy: f32, c: Color) {
        unsafe {
            let e1 = D2D1_ELLIPSE { point: v2(cx, cy), radiusX: r, radiusY: r };
            let e2 = D2D1_ELLIPSE { point: v2(cx + ox, cy + oy), radiusX: r * 0.85, radiusY: r * 0.85 };
            let (Ok(g1), Ok(g2)) = (self.d2d.CreateEllipseGeometry(&e1), self.d2d.CreateEllipseGeometry(&e2)) else { return };
            let Ok(p) = self.d2d.CreatePathGeometry() else { return };
            let Ok(sink) = p.Open() else { return };
            let _ = g1.CombineWithGeometry(&g2, D2D1_COMBINE_MODE_EXCLUDE, None, 0.1, &sink);
            let _ = sink.Close();
            self.set(c);
            self.rt.FillGeometry(&p, &self.brush, None);
        }
    }

    pub fn bitmap(&self, px: &[u8], w: u32, h: u32) -> Option<ID2D1Bitmap> {
        let props = D2D1_BITMAP_PROPERTIES {
            pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
            dpiX: 96.0,
            dpiY: 96.0,
        };
        unsafe {
            self.rt
                .CreateBitmap(D2D_SIZE_U { width: w, height: h }, Some(px.as_ptr() as *const _), w * 4, &props)
                .ok()
        }
    }

    /// Draw a bitmap clipped to a rounded rectangle.
    #[allow(clippy::too_many_arguments)]
    pub fn bitmap_rounded(&self, b: &ID2D1Bitmap, bw: f32, bh: f32, x: f32, y: f32, w: f32, h: f32, r: f32) {
        unsafe {
            let props = D2D1_BITMAP_BRUSH_PROPERTIES {
                extendModeX: D2D1_EXTEND_MODE_CLAMP,
                extendModeY: D2D1_EXTEND_MODE_CLAMP,
                interpolationMode: D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            };
            let Ok(br) = self.rt.CreateBitmapBrush(b, Some(&props), None) else { return };
            br.SetTransform(&Matrix3x2 { M11: w / bw, M12: 0.0, M21: 0.0, M22: h / bh, M31: x, M32: y });
            let rr = D2D1_ROUNDED_RECT {
                rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h },
                radiusX: r,
                radiusY: r,
            };
            self.rt.FillRoundedRectangle(&rr, &br);
        }
    }
}

// ------------------------------------------------------------------ DIB surface

pub struct Surface {
    pub dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    pub bits: *mut u32,
    pub w: i32,
    pub h: i32,
}

impl Surface {
    pub fn new(screen: HDC, w: i32, h: i32) -> Option<Self> {
        unsafe {
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let dc = CreateCompatibleDC(screen);
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(screen, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0).ok()?;
            let old = SelectObject(dc, bmp);
            Some(Surface { dc, bmp, old, bits: bits as *mut u32, w, h })
        }
    }
    pub fn pixels(&mut self) -> &mut [u32] {
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.dc);
        }
    }
}

// ------------------------------------------------------------------ wallpaper

/// A small, heavily blurred, saturated copy of the wallpaper for one monitor.
pub struct Wall {
    pub mon: RECT,
    w: usize,
    h: usize,
    kx: f32,
    ky: f32,
    px: Vec<u8>, // RGB
}

impl Wall {
    fn sample(&self, sx: f32, sy: f32) -> [f32; 3] {
        let u = ((sx - self.mon.left as f32) * self.kx - 0.5).clamp(0.0, (self.w - 1) as f32);
        let v = ((sy - self.mon.top as f32) * self.ky - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (u as usize, v as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (u - x0 as f32, v - y0 as f32);
        let p = |x: usize, y: usize, c: usize| self.px[(y * self.w + x) * 3 + c] as f32;
        let mut o = [0.0; 3];
        for (c, oc) in o.iter_mut().enumerate() {
            let a = p(x0, y0, c) + (p(x1, y0, c) - p(x0, y0, c)) * fx;
            let b = p(x0, y1, c) + (p(x1, y1, c) - p(x0, y1, c)) * fx;
            *oc = (a + (b - a) * fy) / 255.0;
        }
        o
    }
}

fn wallpaper_path() -> Option<String> {
    let mut buf = [0u16; 520];
    unsafe {
        let _ = SystemParametersInfoW(
            SPI_GETDESKWALLPAPER,
            buf.len() as u32,
            Some(buf.as_mut_ptr() as *mut _),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        );
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
    let s = String::from_utf16_lossy(&buf[..n]);
    if !s.is_empty() && Path::new(&s).exists() {
        return Some(s);
    }
    let t = std::env::var("APPDATA").ok()? + "\\Microsoft\\Windows\\Themes\\TranscodedWallpaper";
    Path::new(&t).exists().then_some(t)
}

pub fn load_wall(wic: &IWICImagingFactory, mon: RECT) -> Wall {
    let (mw, mh) = ((mon.right - mon.left).max(1), (mon.bottom - mon.top).max(1));
    let (w, h) = ((mw / 4).max(16) as usize, (mh / 4).max(16) as usize);
    let bg = unsafe { GetSysColor(COLOR_DESKTOP) };
    let bgc = [(bg & 255) as u8, ((bg >> 8) & 255) as u8, ((bg >> 16) & 255) as u8];
    let mut px: Vec<u8> = bgc.iter().copied().cycle().take(w * h * 3).collect();
    let style = reg_string(HKEY_CURRENT_USER, "Control Panel\\Desktop", "WallpaperStyle")
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(10);
    if let Some(p) = wallpaper_path() {
        let _ = decode_wall(wic, &p, w, h, style, &mut px);
    }
    for _ in 0..3 {
        box_blur(&mut px, w, h, 9);
    }
    // Vibrancy: boost saturation like macOS materials.
    for c in px.chunks_exact_mut(3) {
        let (r, g, b) = (c[0] as f32, c[1] as f32, c[2] as f32);
        let l = 0.299 * r + 0.587 * g + 0.114 * b;
        let s = 1.7;
        c[0] = (l + (r - l) * s).clamp(0.0, 255.0) as u8;
        c[1] = (l + (g - l) * s).clamp(0.0, 255.0) as u8;
        c[2] = (l + (b - l) * s).clamp(0.0, 255.0) as u8;
    }
    Wall { mon, w, h, kx: w as f32 / mw as f32, ky: h as f32 / mh as f32, px }
}

/// Decode wallpaper to `tw`x`th` RGB honoring Fill (10) / Fit (6) / Stretch (2).
pub fn decode_wall(wic: &IWICImagingFactory, path: &str, tw: usize, th: usize, style: u32, out: &mut [u8]) -> Option<()> {
    unsafe {
        let p = wide(path);
        let dec = wic
            .CreateDecoderFromFilename(PCWSTR(p.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let (mut iw, mut ih) = (0u32, 0u32);
        frame.GetSize(&mut iw, &mut ih).ok()?;
        if iw == 0 || ih == 0 {
            return None;
        }
        let (fw, fh) = (tw as f32 / iw as f32, th as f32 / ih as f32);
        let (sw, sh) = match style {
            2 => (tw as u32, th as u32),
            6 => {
                let s = fw.min(fh);
                (((iw as f32 * s).round() as u32).max(1), ((ih as f32 * s).round() as u32).max(1))
            }
            _ => {
                let s = fw.max(fh);
                (((iw as f32 * s).ceil() as u32).max(tw as u32), ((ih as f32 * s).ceil() as u32).max(th as u32))
            }
        };
        let sc = wic.CreateBitmapScaler().ok()?;
        sc.Initialize(&frame, sw, sh, WICBitmapInterpolationModeFant).ok()?;
        let cv = wic.CreateFormatConverter().ok()?;
        cv.Initialize(&sc, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .ok()?;
        // Region of the scaled image to copy and where it lands in the output.
        let cw = sw.min(tw as u32);
        let ch = sh.min(th as u32);
        let sx = (sw - cw) / 2;
        let sy = (sh - ch) / 2;
        let dx = (tw as u32 - cw) / 2;
        let dy = (th as u32 - ch) / 2;
        let mut buf = vec![0u8; (cw * ch * 4) as usize];
        let rc = WICRect { X: sx as i32, Y: sy as i32, Width: cw as i32, Height: ch as i32 };
        cv.CopyPixels(&rc, cw * 4, &mut buf).ok()?;
        for y in 0..ch as usize {
            for x in 0..cw as usize {
                let s = (y * cw as usize + x) * 4;
                let d = ((y + dy as usize) * tw + x + dx as usize) * 3;
                out[d] = buf[s + 2];
                out[d + 1] = buf[s + 1];
                out[d + 2] = buf[s];
            }
        }
        Some(())
    }
}

fn box_blur(px: &mut [u8], w: usize, h: usize, r: usize) {
    let mut tmp = vec![0u8; px.len()];
    let d = (2 * r + 1) as u32;
    // horizontal
    for y in 0..h {
        for c in 0..3 {
            let at = |x: isize| px[(y * w + x.clamp(0, w as isize - 1) as usize) * 3 + c] as u32;
            let mut acc: u32 = (-(r as isize)..=r as isize).map(at).sum();
            for x in 0..w {
                tmp[(y * w + x) * 3 + c] = (acc / d) as u8;
                acc = acc + at(x as isize + r as isize + 1) - at(x as isize - r as isize);
            }
        }
    }
    // vertical
    for x in 0..w {
        for c in 0..3 {
            let at = |y: isize| tmp[(y.clamp(0, h as isize - 1) as usize * w + x) * 3 + c] as u32;
            let mut acc: u32 = (-(r as isize)..=r as isize).map(at).sum();
            for y in 0..h {
                px[(y * w + x) * 3 + c] = (acc / d) as u8;
                acc = acc + at(y as isize + r as isize + 1) - at(y as isize - r as isize);
            }
        }
    }
}

// ------------------------------------------------------------------ glass

pub struct Look {
    pub dark: bool,
    pub glass: f32,
}

/// Rounded-rect signed distance with smooth "continuous" corners.
fn sdf(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - hw + r;
    let qy = py.abs() - hh + r;
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    let outer = if ox > 0.0 && oy > 0.0 {
        let p = 2.6f32;
        (ox.powf(p) + oy.powf(p)).powf(1.0 / p)
    } else {
        ox + oy
    };
    outer + qx.max(qy).min(0.0) - r
}

fn hash(x: i32, y: i32) -> f32 {
    let mut n = (x as u32).wrapping_mul(374761393) ^ (y as u32).wrapping_mul(668265263);
    n = (n ^ (n >> 13)).wrapping_mul(1274126177);
    ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
}

/// Build the static glass background (shadow + frosted wallpaper + border + sheen)
/// as premultiplied BGRA. Returns the pixels and the average glass luminance.
#[allow(clippy::too_many_arguments)]
pub fn build_glass(wall: &Wall, wx: i32, wy: i32, ww: i32, wh: i32, m: i32, s: f32, look: &Look) -> (Vec<u32>, f32) {
    let gw = (ww - 2 * m) as f32;
    let gh = (wh - 2 * m) as f32;
    let (cx, cy) = (m as f32 + gw / 2.0, m as f32 + gh / 2.0);
    let (hw, hh) = (gw / 2.0, gh / 2.0);
    let r = 24.0 * s;
    let (off, blur) = (8.0 * s, 22.0 * s);
    let bw = s.max(1.0);
    let g = look.glass.clamp(0.0, 2.0);
    let (tint, ta, ba, hl, sh) = if look.dark {
        ([0.07f32, 0.07, 0.09], 0.36 * g, 0.16, 0.06, 0.34)
    } else {
        ([1.0f32, 1.0, 1.0], 0.20 * g, 0.42, 0.16, 0.16)
    };
    let mut out = vec![0u32; (ww * wh) as usize];
    let (mut lum, mut n) = (0.0f32, 0u32);
    for y in 0..wh {
        let fy = y as f32 + 0.5 - cy;
        let ty = ((y - m) as f32 / gh).clamp(0.0, 1.0);
        for x in 0..ww {
            let fx = x as f32 + 0.5 - cx;
            let d = sdf(fx, fy, hw, hh, r);
            let cov = (0.5 - d).clamp(0.0, 1.0);
            let ds = sdf(fx, fy - off, hw, hh, r);
            let sa = if ds <= 0.0 {
                sh
            } else {
                let t = (ds / blur).min(1.0);
                sh * (1.0 - t) * (1.0 - t)
            };
            if cov <= 0.0 && sa < 0.003 {
                continue;
            }
            let mut c = [0.0f32; 3];
            if cov > 0.0 {
                c = wall.sample((wx + x) as f32, (wy + y) as f32);
                for i in 0..3 {
                    c[i] = c[i] * (1.0 - ta) + tint[i] * ta;
                }
                // soft top sheen
                let sheen = hl * (1.0 - ty * 2.2).max(0.0);
                // 1px inner border, brighter at the top
                let bcov = cov - (0.5 - (d + bw)).clamp(0.0, 1.0);
                let ba = ba * bcov * (1.0 - 0.55 * ty);
                let noise = (hash(x, y) - 0.5) * (3.0 / 255.0);
                for v in c.iter_mut() {
                    *v = (*v + sheen * (1.0 - *v)) * (1.0 - ba) + ba + noise;
                    *v = v.clamp(0.0, 1.0);
                }
                if cov >= 1.0 {
                    lum += 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
                    n += 1;
                }
            }
            let a = cov + sa * (1.0 - cov);
            let px = |v: f32| ((v * cov) * 255.0 + 0.5) as u32;
            out[(y * ww + x) as usize] = (((a * 255.0 + 0.5) as u32) << 24) | (px(c[0]) << 16) | (px(c[1]) << 8) | px(c[2]);
        }
    }
    (out, if n > 0 { lum / n as f32 } else { 0.5 })
}

// ------------------------------------------------------------------ icons & png

/// Tray icon drawn in code (rounded glass square with a gradient).
pub fn make_icon(size: i32) -> HICON {
    unsafe {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let dc = GetDC(None);
        let mut bits = std::ptr::null_mut();
        let Ok(color) = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, HANDLE::default(), 0) else {
            ReleaseDC(None, dc);
            return HICON::default();
        };
        ReleaseDC(None, dc);
        let px = std::slice::from_raw_parts_mut(bits as *mut u32, (size * size) as usize);
        let s = size as f32;
        let a = [1.0f32, 0.42, 0.53];
        let b = [0.42f32, 0.36, 1.0];
        for y in 0..size {
            for x in 0..size {
                let (fx, fy) = (x as f32 + 0.5 - s / 2.0, y as f32 + 0.5 - s / 2.0);
                let d = sdf(fx, fy, s / 2.0 - 1.0, s / 2.0 - 1.0, s * 0.28);
                let cov = (0.5 - d).clamp(0.0, 1.0);
                let t = (x + y) as f32 / (2.0 * s);
                let mut c = [0.0; 3];
                for i in 0..3 {
                    c[i] = a[i] + (b[i] - a[i]) * t;
                }
                // glass highlight on top half
                if fy < 0.0 {
                    for v in c.iter_mut() {
                        *v += (1.0 - *v) * 0.25;
                    }
                }
                let p = |v: f32| ((v * cov) * 255.0) as u32;
                px[(y * size + x) as usize] = (((cov * 255.0) as u32) << 24) | (p(c[0]) << 16) | (p(c[1]) << 8) | p(c[2]);
            }
        }
        let mask = CreateBitmap(size, size, 1, 1, None);
        let info = ICONINFO { fIcon: TRUE, xHotspot: 0, yHotspot: 0, hbmMask: mask, hbmColor: color };
        let icon = CreateIconIndirect(&info).unwrap_or_default();
        let _ = DeleteObject(mask);
        let _ = DeleteObject(color);
        icon
    }
}

pub fn save_png(wic: &IWICImagingFactory, path: &str, w: u32, h: u32, bgra: &mut [u8]) -> Result<()> {
    unsafe {
        let stream = wic.CreateStream()?;
        let p = wide(path);
        stream.InitializeFromFilename(PCWSTR(p.as_ptr()), GENERIC_WRITE.0)?;
        let enc = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        enc.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        let mut bag = None;
        enc.CreateNewFrame(&mut frame, &mut bag)?;
        let frame = frame.ok_or(Error::from(E_FAIL))?;
        frame.Initialize(None)?;
        frame.SetSize(w, h)?;
        let mut fmt = GUID_WICPixelFormat32bppBGRA;
        frame.SetPixelFormat(&mut fmt)?;
        frame.WritePixels(h, w * 4, bgra)?;
        frame.Commit()?;
        enc.Commit()?;
        Ok(())
    }
}
