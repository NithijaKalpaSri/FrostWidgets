//! Reminders editor: a normal window drawn with the same Direct2D pipeline,
//! plus one native EDIT control for typing (caret, IME, clipboard for free).
//! Exists only while open.
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::gfx::{Align, Color, Fam, Gfx, Surface, WHITE};

pub const W0: f32 = 440.0;
pub const H0: f32 = 560.0;
const LIST_TOP: f32 = 88.0;
const BOTTOM: f32 = 84.0;
const ROW: f32 = 44.0;

const G_CIRCLE: char = '\u{ECCA}';
const G_DONE: char = '\u{EC61}';
const G_EDIT: char = '\u{E70F}';
const G_UP: char = '\u{E70E}';
const G_DOWN: char = '\u{E70D}';
const G_DELETE: char = '\u{E74D}';

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hit {
    None,
    Row(usize),
    Toggle(usize),
    Edit(usize),
    Up(usize),
    Down(usize),
    Delete(usize),
    Clear,
    Add,
}

impl Hit {
    pub fn clickable(self) -> bool {
        !matches!(self, Hit::None | Hit::Row(_))
    }
}

pub struct Theme {
    pub bg: Color,
    pub fg: Color,
    pub fg2: Color,
    pub line: Color,
    pub hover: Color,
    pub input: Color,
    pub accent: Color,
}

pub fn theme(dark: bool, accent: Color) -> Theme {
    if dark {
        let fg = WHITE;
        Theme {
            bg: Color::hex(0x1E1E22),
            fg,
            fg2: fg.alpha(0.6),
            line: fg.alpha(0.08),
            hover: fg.alpha(0.06),
            input: Color::hex(0x2A2A30),
            accent,
        }
    } else {
        let fg = Color::hex(0x1D1D1F);
        Theme {
            bg: Color::hex(0xF6F6F8),
            fg,
            fg2: fg.alpha(0.58),
            line: fg.alpha(0.08),
            hover: fg.alpha(0.05),
            input: Color::hex(0xFFFFFF),
            accent,
        }
    }
}

pub fn colorref(c: Color) -> COLORREF {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u32;
    COLORREF(b(c.r) | (b(c.g) << 8) | (b(c.b) << 16))
}

pub struct Editor {
    pub hwnd: HWND,
    pub edit: HWND,
    pub surf: Option<Surface>,
    pub s: f32,
    pub hover: Hit,
    pub editing: Option<usize>,
    pub scroll: f32,
    pub font: HFONT,
    pub brush: HBRUSH,
    pub tracking: bool,
}

impl Editor {
    /// Client size in logical px.
    pub fn size(&self) -> (f32, f32) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        ((r.right - r.left) as f32 / self.s, (r.bottom - r.top) as f32 / self.s)
    }

    fn input_rect(w: f32, h: f32) -> (f32, f32, f32, f32) {
        (16.0, h - 62.0, w - 32.0 - 98.0, 44.0)
    }
    fn add_rect(w: f32, h: f32) -> (f32, f32, f32, f32) {
        (w - 16.0 - 88.0, h - 62.0, 88.0, 44.0)
    }
    fn icon_x(w: f32) -> [f32; 4] {
        [w - 128.0, w - 98.0, w - 68.0, w - 38.0] // edit, up, down, delete
    }

    pub fn max_scroll(&self, n: usize) -> f32 {
        let (_, h) = self.size();
        (n as f32 * ROW - (h - BOTTOM - LIST_TOP - 8.0)).max(0.0)
    }

    pub fn place_edit(&self) {
        let (w, h) = self.size();
        let (x, y, iw, ih) = Self::input_rect(w, h);
        let eh = 22.0;
        let s = self.s;
        unsafe {
            let _ = MoveWindow(
                self.edit,
                ((x + 14.0) * s).round() as i32,
                ((y + (ih - eh) / 2.0) * s).round() as i32,
                ((iw - 28.0) * s).round() as i32,
                (eh * s).round() as i32,
                true,
            );
        }
    }

    pub fn hit(&self, x: f32, y: f32, notes: &[(bool, String)]) -> Hit {
        let (w, h) = self.size();
        let inside = |r: (f32, f32, f32, f32)| x >= r.0 && x <= r.0 + r.2 && y >= r.1 && y <= r.1 + r.3;
        if inside(Self::add_rect(w, h)) {
            return Hit::Add;
        }
        if notes.iter().any(|n| n.0) && inside((w - 170.0, 24.0, 150.0, 26.0)) {
            return Hit::Clear;
        }
        if y < LIST_TOP || y > h - BOTTOM {
            return Hit::None;
        }
        let i = ((y - LIST_TOP + self.scroll) / ROW) as usize;
        if i >= notes.len() {
            return Hit::None;
        }
        let cy = LIST_TOP + i as f32 * ROW - self.scroll + ROW / 2.0;
        let near = |cx: f32| (x - cx).powi(2) + (y - cy).powi(2) < 15.0f32.powi(2);
        if near(34.0) {
            return Hit::Toggle(i);
        }
        let ix = Self::icon_x(w);
        if near(ix[0]) {
            return Hit::Edit(i);
        }
        if near(ix[1]) && i > 0 {
            return Hit::Up(i);
        }
        if near(ix[2]) && i + 1 < notes.len() {
            return Hit::Down(i);
        }
        if near(ix[3]) {
            return Hit::Delete(i);
        }
        Hit::Row(i)
    }

    pub fn paint(&mut self, g: &mut Gfx, screen: HDC, hdc: HDC, notes: &[(bool, String)], th: &Theme) {
        let mut r = RECT::default();
        unsafe {
            let _ = GetClientRect(self.hwnd, &mut r);
        }
        let (cw, ch) = (r.right.max(1), r.bottom.max(1));
        if self.surf.as_ref().map(|s| (s.w, s.h)) != Some((cw, ch)) {
            self.surf = Surface::new(screen, cw, ch);
        }
        let Some(surf) = self.surf.as_ref() else { return };
        let (w, h) = (cw as f32 / self.s, ch as f32 / self.s);
        unsafe {
            if g.rt.BindDC(surf.dc, &RECT { left: 0, top: 0, right: cw, bottom: ch }).is_err() {
                return;
            }
            g.rt.BeginDraw();
            g.rt.SetTransform(&Matrix3x2 { M11: self.s, M12: 0.0, M21: 0.0, M22: self.s, M31: 0.0, M32: 0.0 });
            g.fill_rrect(-1.0, -1.0, w + 2.0, h + 2.0, 0.0, th.bg);

            // Header
            g.text("Reminders", 24.0, 18.0, 260.0, 36.0, 26.0, 700, Align::Left, Fam::Display, th.fg, false);
            let done = notes.iter().filter(|n| n.0).count();
            let open = notes.len() - done;
            let sub = format!("{open} open \u{B7} {done} done");
            g.text(&sub, 25.0, 54.0, 260.0, 20.0, 13.0, 400, Align::Left, Fam::Text, th.fg2, false);
            if done > 0 {
                if self.hover == Hit::Clear {
                    g.fill_rrect(w - 170.0, 24.0, 150.0, 26.0, 13.0, th.accent.alpha(0.14));
                }
                g.text("Clear completed", w - 170.0, 24.0, 150.0, 26.0, 13.0, 600, Align::Center, Fam::Text, th.accent, false);
            }

            // List
            g.rt.PushAxisAlignedClip(
                &D2D_RECT_F { left: 0.0, top: LIST_TOP - 2.0, right: w, bottom: h - BOTTOM },
                D2D1_ANTIALIAS_MODE_ALIASED,
            );
            if notes.is_empty() {
                g.text("No reminders yet", 0.0, LIST_TOP + 60.0, w, 28.0, 17.0, 600, Align::Center, Fam::Text, th.fg, false);
                g.text("Type below and press Enter", 0.0, LIST_TOP + 88.0, w, 22.0, 13.0, 400, Align::Center, Fam::Text, th.fg2, false);
            }
            let ix = Self::icon_x(w);
            for (i, (is_done, text)) in notes.iter().enumerate() {
                let y = LIST_TOP + i as f32 * ROW - self.scroll;
                if y + ROW < LIST_TOP - 2.0 || y > h - BOTTOM {
                    continue;
                }
                let cy = y + ROW / 2.0;
                let row_hover = matches!(self.hover, Hit::Row(j) | Hit::Toggle(j) | Hit::Edit(j) | Hit::Up(j) | Hit::Down(j) | Hit::Delete(j) if j == i);
                let editing = self.editing == Some(i);
                if editing {
                    g.fill_rrect(12.0, y + 2.0, w - 24.0, ROW - 4.0, 10.0, th.accent.alpha(0.16));
                } else if row_hover {
                    g.fill_rrect(12.0, y + 2.0, w - 24.0, ROW - 4.0, 10.0, th.hover);
                }
                if *is_done {
                    g.glyph(G_DONE, 34.0, cy, 18.0, th.accent);
                } else {
                    let c = if self.hover == Hit::Toggle(i) { th.accent } else { th.fg2 };
                    g.glyph(G_CIRCLE, 34.0, cy, 18.0, c);
                }
                let text_w = if row_hover || editing { ix[0] - 18.0 - 56.0 } else { w - 56.0 - 24.0 };
                g.text(text, 56.0, y, text_w, ROW, 15.0, 400, Align::Left, Fam::Text, if *is_done { th.fg2 } else { th.fg }, true);
                if row_hover || editing {
                    let icons = [
                        (G_EDIT, Hit::Edit(i), true),
                        (G_UP, Hit::Up(i), i > 0),
                        (G_DOWN, Hit::Down(i), i + 1 < notes.len()),
                        (G_DELETE, Hit::Delete(i), true),
                    ];
                    for (k, (glyph, hit, enabled)) in icons.iter().enumerate() {
                        if self.hover == *hit && *enabled {
                            let bg = if k == 3 { Color::hex(0xE5484D).alpha(0.16) } else { th.hover };
                            g.fill_circle(ix[k], cy, 15.0, bg);
                        }
                        let c = if !enabled {
                            th.fg2.alpha(0.3)
                        } else if k == 3 && self.hover == *hit {
                            Color::hex(0xE5484D)
                        } else {
                            th.fg2
                        };
                        g.glyph(*glyph, ix[k], cy, 14.0, c);
                    }
                }
            }
            g.rt.PopAxisAlignedClip();

            // Input bar
            g.line(0.0, h - BOTTOM, w, h - BOTTOM, 1.0, th.line);
            let (x, y, iw, ih) = Self::input_rect(w, h);
            if self.editing.is_some() {
                g.fill_rrect(x - 2.0, y - 2.0, iw + 4.0, ih + 4.0, 14.0, th.accent);
            }
            g.fill_rrect(x, y, iw, ih, 12.0, th.input);
            let (ax, ay, aw, ah) = Self::add_rect(w, h);
            let fill = if self.hover == Hit::Add { th.accent.alpha(0.85) } else { th.accent };
            g.fill_rrect(ax, ay, aw, ah, 12.0, fill);
            let label = if self.editing.is_some() { "Save" } else { "Add" };
            g.text(label, ax, ay, aw, ah, 15.0, 600, Align::Center, Fam::Text, WHITE, false);

            let _ = g.rt.EndDraw(None, None);
            let _ = BitBlt(hdc, 0, 0, cw, ch, surf.dc, 0, 0, SRCCOPY);
        }
    }
}
