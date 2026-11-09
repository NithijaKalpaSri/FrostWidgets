//! Desktop pet: a small pixel critter that lives on top of the taskbar.
//! Each frame is drawn on a tiny pixel grid in software and scaled up, and it
//! only animates a few times a second, so it costs next to nothing.

pub const GW: usize = 34;
pub const GH: usize = 28;
const N: usize = GW * GH;
/// Bottom row of the feet. The row under it holds the shadow.
const F: i32 = GH as i32 - 2;
/// Rows from the top of the grid down to the ground line.
pub const GROUND_ROWS: i32 = F + 1;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Roam,
    Calm,
    Walk,
    Laptop,
    Nap,
}

/// (mode, config key, name, description)
pub const MODES: [(Mode, &str, &str, &str); 5] = [
    (Mode::Roam, "roam", "Do everything", "Walks, sits, works on its laptop and naps when it likes"),
    (Mode::Calm, "calm", "Stay calm", "Stays in one spot, sitting or working on its laptop"),
    (Mode::Walk, "walk", "Keep walking", "Strolls back and forth along the taskbar"),
    (Mode::Laptop, "laptop", "Work on laptop", "Sits still and types away"),
    (Mode::Nap, "nap", "Nap", "Settles down and sleeps"),
];

impl Mode {
    pub fn from_key(s: &str) -> Mode {
        MODES.iter().find(|m| m.1.eq_ignore_ascii_case(s)).map(|m| m.0).unwrap_or(Mode::Roam)
    }
    pub fn index(self) -> usize {
        MODES.iter().position(|m| m.0 == self).unwrap_or(0)
    }
}

/// (config key, name, walking speed in grid cells per second)
pub const SPEEDS: [(&str, &str, f32); 3] = [("slow", "Slow", 3.5), ("normal", "Normal", 6.0), ("fast", "Fast", 10.0)];
/// (config key, name, screen pixels per grid cell at 100% display scaling)
pub const SIZES: [(&str, &str, f32); 3] = [("small", "Small", 2.0), ("medium", "Medium", 3.0), ("large", "Large", 4.0)];

/// Position of `key` in a SPEEDS/SIZES list (the middle one when unknown).
pub fn option_index(list: &[(&str, &str, f32)], key: &str) -> usize {
    list.iter().position(|o| o.0.eq_ignore_ascii_case(key)).unwrap_or(1)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Act {
    Walk,
    Sit,
    Laptop,
    Sleep,
    Hop,
    Held,
    Fall,
    Land,
}

/// What the pet knows about the world each frame.
pub struct Ctx {
    /// Allowed range for the window's left edge (screen px).
    pub lo: f32,
    pub hi: f32,
    /// Walking speed in screen px per second.
    pub speed: f32,
    /// Seconds since the last keyboard or mouse input.
    pub idle: u32,
    pub hour: u16,
}

pub struct Pet {
    pub act: Act,
    /// 1 = facing right, -1 = facing left.
    pub dir: i32,
    /// Window left edge, and height above the ground while carried or falling (screen px).
    pub x: f32,
    pub lift: f32,
    vy: f32,
    step: u32,
    /// Time left in the current activity (ms).
    left: i32,
    blink: bool,
    look: i32,
    think: u8,
    /// Napping because you're away; wakes when you come back.
    idle_nap: bool,
    rng: u32,
    mode: Mode,
    after: (Act, i32),
}

/// Hop height per frame, in grid cells.
const HOP: [i32; 10] = [1, 3, 4, 5, 5, 4, 3, 1, 0, 0];

impl Pet {
    pub fn new(mode: Mode, x: f32, seed: u32) -> Self {
        Pet {
            act: Act::Sit,
            dir: 1,
            x,
            lift: 0.0,
            vy: 0.0,
            step: 0,
            left: 1500,
            blink: false,
            look: 0,
            think: 0,
            idle_nap: false,
            rng: seed | 1,
            mode,
            after: (Act::Sit, 0),
        }
    }

    fn rand(&mut self, n: u32) -> u32 {
        let mut x = self.rng;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.rng = x;
        x % n.max(1)
    }

    fn range(&mut self, a: i32, b: i32) -> i32 {
        a + self.rand((b - a + 1).max(1) as u32) as i32
    }

    pub fn set_mode(&mut self, m: Mode) {
        self.mode = m;
        if matches!(self.act, Act::Walk | Act::Sit | Act::Laptop | Act::Sleep) {
            self.left = 0;
        }
    }

    fn start(&mut self, act: Act, ms: i32) {
        self.act = act;
        self.left = ms;
        self.step = 0;
        self.blink = false;
        self.think = 0;
        self.look = 0;
        self.idle_nap = false;
    }

    fn walk(&mut self, a: i32, b: i32) {
        let ms = self.range(a, b);
        if self.rand(100) < 50 {
            self.dir = -self.dir;
        }
        self.start(Act::Walk, ms);
    }

    fn timed(&mut self, act: Act, a: i32, b: i32) {
        let ms = self.range(a, b);
        self.start(act, ms);
    }

    /// Pick the next activity for the chosen behavior.
    fn next(&mut self, c: &Ctx) {
        match self.mode {
            Mode::Nap => self.start(Act::Sleep, 60_000),
            Mode::Laptop => self.start(Act::Laptop, 60_000),
            Mode::Walk => {
                if self.act == Act::Walk && self.rand(100) < 25 {
                    self.timed(Act::Sit, 1500, 4000);
                } else {
                    self.walk(6000, 20000);
                }
            }
            Mode::Calm => match self.rand(100) {
                0..=44 => self.timed(Act::Sit, 6000, 15000),
                45..=89 => self.timed(Act::Laptop, 15000, 45000),
                _ => self.timed(Act::Sleep, 20000, 60000),
            },
            Mode::Roam => {
                if c.idle > 600 {
                    self.start(Act::Sleep, 30_000);
                    self.idle_nap = true;
                    return;
                }
                let sleep_w = if c.hour < 6 { 30 } else { 8 };
                match self.rand(100 + sleep_w) {
                    0..=39 => self.walk(4000, 12000),
                    40..=64 => self.timed(Act::Sit, 5000, 14000),
                    65..=99 => self.timed(Act::Laptop, 12000, 40000),
                    _ => self.timed(Act::Sleep, 15000, 45000),
                }
            }
        }
    }

    /// Advance one frame. Returns the delay until the next frame in ms,
    /// or 0 while the pet is being carried (nothing to animate).
    pub fn tick(&mut self, c: &Ctx) -> u32 {
        let hi = c.hi.max(c.lo);
        if self.act != Act::Held {
            self.x = self.x.clamp(c.lo, hi);
        }
        let dt: i32 = match self.act {
            Act::Held => return 0,
            Act::Fall => {
                self.vy += 2600.0 * 0.016;
                self.lift -= self.vy * 0.016;
                if self.lift <= 0.0 {
                    self.lift = 0.0;
                    self.vy = 0.0;
                    self.start(Act::Land, 0);
                }
                return 16;
            }
            Act::Land => {
                self.step += 1;
                if self.step >= 3 {
                    self.timed(Act::Sit, 2000, 4000);
                }
                return 90;
            }
            Act::Hop => {
                self.step += 1;
                if self.step as usize >= HOP.len() {
                    let (a, ms) = self.after;
                    self.start(a, ms.max(1500));
                }
                return 35;
            }
            Act::Walk => {
                self.step += 1;
                self.x += self.dir as f32 * c.speed * 0.11;
                if self.x <= c.lo {
                    self.x = c.lo;
                    self.dir = 1;
                } else if self.x >= hi {
                    self.x = hi;
                    self.dir = -1;
                }
                if self.rand(1000) < 5 {
                    self.dir = -self.dir;
                }
                110
            }
            Act::Sit => {
                if self.blink {
                    self.blink = false;
                    self.range(1500, 3500)
                } else {
                    let r = self.rand(100);
                    if r < 45 {
                        self.blink = true;
                        130
                    } else {
                        if r < 75 {
                            self.look = self.range(-1, 1);
                        }
                        self.range(1500, 3500)
                    }
                }
            }
            Act::Laptop => {
                if self.think > 0 {
                    self.think -= 1;
                    450
                } else {
                    if self.rand(100) < 5 {
                        self.think = self.range(3, 7) as u8;
                    }
                    self.step += 1;
                    190
                }
            }
            Act::Sleep => {
                self.step += 1;
                if self.idle_nap && c.idle < 3 && self.step > 3 {
                    self.left = 0;
                }
                900
            }
        };
        self.left -= dt;
        if self.left <= 0 {
            self.next(c);
        }
        dt as u32
    }

    /// Clicked: a little hop, then back to what it was doing (wakes it up).
    pub fn poke(&mut self) {
        if !matches!(self.act, Act::Walk | Act::Sit | Act::Laptop | Act::Sleep) {
            return;
        }
        self.after = if self.act == Act::Sleep { (Act::Sit, 3000) } else { (self.act, self.left) };
        self.act = Act::Hop;
        self.step = 0;
    }

    pub fn grab(&mut self) {
        self.act = Act::Held;
        self.step = 0;
    }

    pub fn release(&mut self) {
        self.vy = 0.0;
        if self.lift <= 0.5 {
            self.lift = 0.0;
            self.start(Act::Land, 0);
        } else {
            self.act = Act::Fall;
        }
    }

    pub fn carried(&self) -> bool {
        matches!(self.act, Act::Held | Act::Fall)
    }

    /// Draw the current frame as premultiplied BGRA pixels, GW x GH.
    pub fn render(&self, out: &mut [u32]) {
        draw(self, out);
    }
}

// ------------------------------------------------------------------ drawing

const BLUE: u32 = 0x5A86F0;
const PURPLE: u32 = 0x9461EA;
const LINE: u32 = 0x231B48;
const EYE: u32 = 0x17122E;
const EAR: u32 = 0xC5A8FF;
const BELLY: u32 = 0xE4E7FF;
const SCARF: u32 = 0xE6D6BD;
const SCARF_DK: u32 = 0xC9B48F;
const LID: u32 = 0xC5CCDA;
const LID_HI: u32 = 0xDDE2EB;
const LID_EDGE: u32 = 0x3B4256;
const LOGO: u32 = 0x8FD3FF;
const BASE: u32 = 0x7E879B;
const GLOW: u32 = 0x9FDBFF;
const ZZZ: u32 = 0xD3DAFF;
const SHADOW: u32 = 0x3000_0000;

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| {
        let (x, y) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
        ((x + (y - x) * t).round() as u32) << s
    };
    ch(16) | ch(8) | ch(0)
}

fn opaque(rgb: u32) -> u32 {
    0xFF00_0000 | rgb
}

#[derive(Clone, Copy, PartialEq)]
enum Legs {
    Walk(u32),
    Sit,
    Tucked,
    Dangle,
}

#[derive(Clone, Copy, PartialEq)]
enum Eyes {
    Open,
    Blink,
    Closed,
    Up,
}

struct Grid {
    px: [u32; N],
    fur: [bool; N],
}

impl Grid {
    fn idx(x: i32, y: i32) -> Option<usize> {
        (x >= 0 && y >= 0 && (x as usize) < GW && (y as usize) < GH).then(|| y as usize * GW + x as usize)
    }
    fn fur(&mut self, x: i32, y: i32, on: bool) {
        if let Some(i) = Self::idx(x, y) {
            self.fur[i] = on;
        }
    }
    fn is_fur(&self, x: i32, y: i32) -> bool {
        Self::idx(x, y).map(|i| self.fur[i]).unwrap_or(false)
    }
    fn put(&mut self, x: i32, y: i32, c: u32) {
        if let Some(i) = Self::idx(x, y) {
            self.px[i] = c;
        }
    }
    fn get(&self, x: i32, y: i32) -> u32 {
        Self::idx(x, y).map(|i| self.px[i]).unwrap_or(0)
    }
    fn rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: u32) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                self.put(x, y, c);
            }
        }
    }
}

fn draw(p: &Pet, out: &mut [u32]) {
    let mut g = Grid { px: [0; N], fur: [false; N] };
    let cx = GW as i32 / 2; // body spans cx-8 ..= cx+7
    let (mut x0, mut x1) = (cx - 8, cx + 7);
    let side = matches!(p.act, Act::Walk) || (p.act == Act::Hop && p.after.0 == Act::Walk);

    // Pose: body box, legs and eyes for this activity.
    let (y0, y1, legs, eyes, eye_dx) = match p.act {
        Act::Walk => (F - 13, F - 3, Legs::Walk(p.step % 4), Eyes::Open, 2),
        Act::Hop => {
            let h = HOP[(p.step as usize).min(HOP.len() - 1)];
            (F - 12 - h, F - 2 - h, Legs::Tucked, Eyes::Open, if side { 2 } else { 0 })
        }
        Act::Held | Act::Fall => (F - 15, F - 5, Legs::Dangle, Eyes::Open, 0),
        Act::Land if p.step == 0 => {
            (x0, x1) = (x0 - 1, x1 + 1);
            (F - 8, F - 1, Legs::Sit, Eyes::Blink, 0)
        }
        Act::Sleep => (F - 11 + (p.step % 2) as i32, F - 1, Legs::Sit, Eyes::Closed, 0),
        Act::Laptop => (F - 11, F - 1, Legs::Sit, if p.think > 0 { Eyes::Up } else { Eyes::Open }, 0),
        Act::Sit | Act::Land => (F - 11, F - 1, Legs::Sit, if p.blink { Eyes::Blink } else { Eyes::Open }, p.look),
    };

    // Body: a soft rounded block.
    for y in y0..=y1 {
        for x in x0..=x1 {
            g.fur(x, y, true);
        }
    }
    for (x, y) in [(x0, y0), (x0 + 1, y0), (x0, y0 + 1), (x1, y0), (x1 - 1, y0), (x1, y0 + 1), (x0, y1), (x1, y1)] {
        g.fur(x, y, false);
    }
    // Ears.
    for x in x0 + 2..=x0 + 4 {
        g.fur(x, y0 - 1, true);
    }
    for x in x1 - 4..=x1 - 2 {
        g.fur(x, y0 - 1, true);
    }
    for x in [x0 + 2, x0 + 3, x1 - 3, x1 - 2] {
        g.fur(x, y0 - 2, true);
    }
    // Fluffy tail on the back (left, before mirroring); it wags while walking.
    let wag = if side && p.step % 2 == 1 { 1 } else { 0 };
    for (x, y) in [
        (x0 - 1, y1 - 2),
        (x0 - 2, y1 - 2),
        (x0 - 2, y1 - 3),
        (x0 - 3, y1 - 3),
        (x0 - 3 + wag, y1 - 4),
        (x0 - 4 + wag, y1 - 4),
        (x0 - 4 + wag, y1 - 5),
        (x0 - 3 + wag, y1 - 5),
        (x0 - 3 + wag, y1 - 6),
    ] {
        g.fur(x, y, true);
    }
    // Legs.
    let leg_x = [x0 + 2, x0 + 5, x1 - 6, x1 - 3];
    match legs {
        Legs::Walk(ph) => {
            for (i, &lx) in leg_x.iter().enumerate() {
                let lifted = (ph == 0 && i % 2 == 0) || (ph == 2 && i % 2 == 1);
                let (bottom, dx) = if lifted { (F - 1, 1) } else { (F, 0) };
                for y in y1 + 1..=bottom {
                    g.fur(lx + dx, y, true);
                    g.fur(lx + dx + 1, y, true);
                }
            }
        }
        Legs::Tucked => {
            for &lx in &leg_x {
                g.fur(lx, y1 + 1, true);
                g.fur(lx + 1, y1 + 1, true);
            }
        }
        Legs::Dangle => {
            for (i, &lx) in leg_x.iter().enumerate() {
                let splay = match i {
                    0 => -1,
                    3 => 1,
                    _ => 0,
                };
                for (k, y) in (y1 + 1..=y1 + 4).enumerate() {
                    let dx = if k >= 2 { splay } else { 0 };
                    g.fur(lx + dx, y, true);
                    g.fur(lx + dx + 1, y, true);
                }
            }
        }
        Legs::Sit => {
            for x in [cx - 5, cx - 4, cx + 3, cx + 4] {
                g.fur(x, F, true);
            }
        }
    }
    // Arms at the sides of the laptop, tapping away.
    let laptop = p.act == Act::Laptop;
    if laptop {
        let (ul, ur) = if p.think > 0 { (0, 0) } else { ((p.step % 2) as i32, 1 - (p.step % 2) as i32) };
        for (ax, u) in [(cx - 10, ul), (cx + 9, ur)] {
            for y in F - 2 - u..=F - 1 - u {
                g.fur(ax, y, true);
                g.fur(ax + 1, y, true);
            }
        }
    }

    // Fur colour: blue melting into purple, lit from the top.
    let (gx0, gw) = ((x0 - 4) as f32, (x1 - x0 + 8) as f32);
    let (gy0, gh) = ((y0 - 2) as f32, (F - y0 + 2) as f32);
    let flicker = if laptop && p.step % 3 == 0 { 0.17 } else { 0.11 };
    for y in 0..GH as i32 {
        for x in 0..GW as i32 {
            if !g.is_fur(x, y) {
                continue;
            }
            let t = ((x as f32 - gx0) / gw * 0.55 + (y as f32 - gy0) / gh * 0.45).clamp(0.0, 1.0);
            let mut c = mix(BLUE, PURPLE, t);
            if !g.is_fur(x, y - 1) {
                c = mix(c, 0xFFFFFF, 0.22);
            } else if !g.is_fur(x, y + 1) {
                c = mix(c, 0x2A1E6E, 0.25);
            }
            if laptop && y <= y0 + 5 && (x - cx).abs() <= 5 {
                c = mix(c, GLOW, flicker);
            }
            g.put(x, y, opaque(c));
        }
    }
    // Inner ears and a lighter belly.
    g.put(x0 + 3, y0 - 1, opaque(EAR));
    g.put(x1 - 3, y0 - 1, opaque(EAR));
    for (y, a, b) in [(y1 - 1, cx - 4, cx + 3), (y1, cx - 3, cx + 2)] {
        for x in a..=b {
            if g.is_fur(x, y) {
                let c = g.get(x, y) & 0xFFFFFF;
                g.put(x, y, opaque(mix(c, BELLY, 0.42)));
            }
        }
    }
    // Knitted scarf, with its loose end hanging on the side it faces.
    let sy = y0 + 6;
    for y in sy..=sy + 1 {
        for x in x0..=x1 {
            if g.is_fur(x, y) {
                let c = if (x - x0) % 4 == 1 { SCARF_DK } else { SCARF };
                g.put(x, y, opaque(c));
            }
        }
    }
    let ex = if side { x1 - 5 } else { cx + 1 };
    for (k, y) in (sy + 2..=sy + 3).enumerate() {
        for x in ex..=ex + 1 {
            g.fur(x, y, true);
            g.put(x, y, opaque(if k == 1 { SCARF_DK } else { SCARF }));
        }
    }

    // Outline everything that's fur.
    let mut edge = Vec::new();
    for y in 0..GH as i32 {
        for x in 0..GW as i32 {
            if !g.is_fur(x, y) && [(1, 0), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| g.is_fur(x + dx, y + dy)) {
                edge.push((x, y));
            }
        }
    }
    for (x, y) in edge {
        g.put(x, y, opaque(LINE));
    }

    // Eyes: small and calm.
    let ey = y0 + 3;
    let eye_x = if side { [cx + 1, cx + 5] } else { [cx - 3 + eye_dx, cx + 2 + eye_dx] };
    for (k, &x) in eye_x.iter().enumerate() {
        match eyes {
            Eyes::Open => {
                g.put(x, ey, opaque(EYE));
                g.put(x, ey + 1, opaque(EYE));
            }
            Eyes::Blink => g.put(x, ey + 1, opaque(EYE)),
            Eyes::Closed => {
                let x2 = if k == 0 { x - 1 } else { x + 1 };
                g.put(x, ey + 1, opaque(EYE));
                g.put(x2, ey + 1, opaque(EYE));
            }
            Eyes::Up => {
                g.put(x + 1, ey - 1, opaque(EYE));
                g.put(x + 1, ey, opaque(EYE));
            }
        }
    }

    // Laptop in front, seen from behind its lid.
    if laptop {
        let (lx0, lx1, ly0, ly1) = (cx - 7, cx + 6, F - 6, F - 1);
        g.rect(lx0, ly0, lx1, ly1, opaque(LID_EDGE));
        g.rect(lx0 + 1, ly0 + 1, lx1 - 1, ly1 - 1, opaque(LID));
        g.rect(lx0 + 1, ly0 + 1, lx1 - 1, ly0 + 1, opaque(LID_HI));
        g.rect(cx - 1, F - 4, cx, F - 3, opaque(if p.think > 0 { LID } else { LOGO }));
        g.rect(cx - 9, F, cx + 8, F, opaque(BASE));
        g.put(cx - 9, F, opaque(LID_EDGE));
        g.put(cx + 8, F, opaque(LID_EDGE));
    }

    // Soft shadow on the ground, smaller while hopping.
    if !p.carried() {
        let h = if p.act == Act::Hop { HOP[(p.step as usize).min(HOP.len() - 1)] } else { 0 };
        let w = 7 - h / 2;
        for x in cx - w..cx + w {
            if g.get(x, F + 1) == 0 {
                g.put(x, F + 1, SHADOW);
            }
        }
    }

    // Face the walking direction.
    for y in 0..GH {
        let row = &g.px[y * GW..(y + 1) * GW];
        let dst = &mut out[y * GW..(y + 1) * GW];
        if p.dir < 0 {
            for (d, s) in dst.iter_mut().zip(row.iter().rev()) {
                *d = *s;
            }
        } else {
            dst.copy_from_slice(row);
        }
    }

    // Floating z's while asleep, after mirroring so the letters never flip.
    if p.act == Act::Sleep {
        let s = p.step as i32;
        for (zx, zy) in [(x1 + 1, y0 - 7 - s % 4), (x1 + 5, y0 - 12 - (s + 2) % 4)] {
            let zx = if p.dir < 0 { GW as i32 - zx - 5 } else { zx };
            stamp_z(out, zx, zy);
        }
    }
}

/// A small pixel "z" with a soft shadow, straight into the output.
fn stamp_z(out: &mut [u32], x: i32, y: i32) {
    let z = ["#####", "...#.", "..#..", ".#...", "#####"];
    let mut put = |px: i32, py: i32, c: u32, only_empty: bool| {
        if let Some(i) = Grid::idx(px, py) {
            if !only_empty || out[i] == 0 {
                out[i] = c;
            }
        }
    };
    for (r, row) in z.iter().enumerate() {
        for (k, ch) in row.chars().enumerate() {
            if ch == '#' {
                put(x + k as i32, y + r as i32, opaque(ZZZ), false);
            }
        }
    }
    for (r, row) in z.iter().enumerate() {
        for (k, ch) in row.chars().enumerate() {
            if ch == '#' {
                put(x + k as i32 + 1, y + r as i32 + 1, 0x9022_1A48, true);
            }
        }
    }
}

/// A posed pet, for preview images.
pub fn posed(act: Act, step: u32, dir: i32, flag: bool) -> Pet {
    let mut p = Pet::new(Mode::Roam, 0.0, 1);
    p.act = act;
    p.step = step;
    p.dir = dir;
    match act {
        Act::Sit => p.blink = flag,
        Act::Laptop if flag => p.think = 3,
        Act::Hop if flag => p.after = (Act::Walk, 0),
        _ => {}
    }
    p
}
