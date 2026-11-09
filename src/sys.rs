//! Cheap system data: CPU, memory, disk, network, battery, weather.
use std::os::windows::process::CommandExt;
use std::time::Instant;

use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::NetworkManagement::IpHelper::*;
use windows::Win32::NetworkManagement::Ndis::IfOperStatusUp;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Power::*;
use windows::Win32::System::SystemInformation::*;
use windows::Win32::System::Threading::GetSystemTimes;

use crate::util::wide;

#[derive(Clone, Copy, Default, PartialEq)]
pub struct Battery {
    pub has: bool,
    pub pct: u8,
    pub charging: bool,
    pub ac: bool,
}

pub struct Stats {
    idle: u64,
    total: u64,
    net: Option<(u64, u64, Instant)>,
    disk_tick: u32,
    drive: Vec<u16>,
    pub cpu: f32,
    pub ram: f32,
    pub disk: f32,
    pub down: f64,
    pub up: f64,
    pub bat: Battery,
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

impl Stats {
    pub fn new() -> Self {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\";
        Stats {
            idle: 0,
            total: 0,
            net: None,
            disk_tick: 0,
            drive: wide(&drive),
            cpu: 0.0,
            ram: 0.0,
            disk: 0.0,
            down: 0.0,
            up: 0.0,
            bat: Battery::default(),
        }
    }

    pub fn sample(&mut self) {
        unsafe {
            // CPU: delta of system idle vs kernel+user time.
            let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
            if GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)).is_ok() {
                let (idle, total) = (ft(i), ft(k) + ft(u));
                if self.total != 0 && total > self.total {
                    let dt = (total - self.total) as f64;
                    let di = (idle - self.idle) as f64;
                    self.cpu = ((1.0 - di / dt) * 100.0).clamp(0.0, 100.0) as f32;
                }
                self.idle = idle;
                self.total = total;
            }

            let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
            if GlobalMemoryStatusEx(&mut m).is_ok() && m.ullTotalPhys > 0 {
                self.ram = ((1.0 - m.ullAvailPhys as f64 / m.ullTotalPhys as f64) * 100.0) as f32;
            }

            // Disk fullness changes slowly: check once a minute.
            if self.disk_tick == 0 {
                let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
                if GetDiskFreeSpaceExW(PCWSTR(self.drive.as_ptr()), Some(&mut avail), Some(&mut total), Some(&mut free)).is_ok()
                    && total > 0
                {
                    self.disk = ((1.0 - free as f64 / total as f64) * 100.0) as f32;
                }
            }
            self.disk_tick = (self.disk_tick + 1) % 30;

            self.sample_net();

            let mut p = SYSTEM_POWER_STATUS::default();
            if GetSystemPowerStatus(&mut p).is_ok() {
                let has = p.BatteryFlag & 128 == 0 && p.BatteryLifePercent != 255;
                self.bat = Battery {
                    has,
                    pct: if has { p.BatteryLifePercent.min(100) } else { 100 },
                    charging: p.BatteryFlag & 8 != 0,
                    ac: p.ACLineStatus == 1,
                };
            }
        }
    }

    unsafe fn sample_net(&mut self) {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return;
        }
        let t = &*table;
        let rows = std::slice::from_raw_parts(t.Table.as_ptr(), t.NumEntries as usize);
        let (mut rx, mut tx) = (0u64, 0u64);
        for r in rows {
            let flags = r.InterfaceAndOperStatusFlags._bitfield;
            let hardware = flags & 1 != 0;
            let filter = flags & 2 != 0;
            if hardware && !filter && r.OperStatus == IfOperStatusUp && r.Type != 24 {
                rx += r.InOctets;
                tx += r.OutOctets;
            }
        }
        FreeMibTable(table as *const _);
        let now = Instant::now();
        if let Some((prx, ptx, pt)) = self.net {
            let dt = now.duration_since(pt).as_secs_f64().max(0.1);
            self.down = rx.saturating_sub(prx) as f64 / dt;
            self.up = tx.saturating_sub(ptx) as f64 / dt;
        }
        self.net = Some((rx, tx, now));
    }
}

// ---------------------------------------------------------------- weather

#[derive(Clone, PartialEq)]
pub struct Weather {
    pub place: String,
    pub temp: f32,
    pub hi: f32,
    pub lo: f32,
    pub code: i32,
    pub day: bool,
}

#[derive(Clone)]
pub struct Geo {
    pub query: String,
    pub lat: f64,
    pub lon: f64,
    pub name: String,
}

pub struct WeatherResult {
    pub weather: Option<Weather>,
    pub geo: Option<Geo>,
    /// Short reason when `weather` is None.
    pub error: &'static str,
}

/// Uses Windows' built-in curl.exe so no TLS stack lives in our process.
fn curl(url: &str) -> Option<String> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("curl.exe")
        .args(["-s", "-f", "-m", "20", url])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

fn enc(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

pub fn fetch_weather(city: &str, imperial: bool, geo: Option<Geo>) -> WeatherResult {
    let geo = match geo.filter(|g| g.query == city) {
        Some(g) => g,
        None => {
            let url = format!("https://geocoding-api.open-meteo.com/v1/search?count=1&format=json&name={}", enc(city.trim()));
            let Some(t) = curl(&url) else {
                return WeatherResult { weather: None, geo: None, error: "Offline" };
            };
            let found = (|| {
                let v: serde_json::Value = serde_json::from_str(&t).ok()?;
                let r = v.get("results")?.get(0)?;
                Some(Geo {
                    query: city.to_string(),
                    lat: r.get("latitude")?.as_f64()?,
                    lon: r.get("longitude")?.as_f64()?,
                    name: r.get("name")?.as_str()?.to_string(),
                })
            })();
            match found {
                Some(g) => g,
                None => return WeatherResult { weather: None, geo: None, error: "City not found" },
            }
        }
    };
    let g = geo;
    let weather = (|| {
        let unit = if imperial { "&temperature_unit=fahrenheit" } else { "" };
        let t = curl(&format!(
            "https://api.open-meteo.com/v1/forecast?latitude={:.4}&longitude={:.4}&current=temperature_2m,weather_code,is_day&daily=temperature_2m_max,temperature_2m_min&timezone=auto&forecast_days=1{unit}",
            g.lat, g.lon
        ))?;
        let v: serde_json::Value = serde_json::from_str(&t).ok()?;
        let c = v.get("current")?;
        let d = v.get("daily")?;
        Some(Weather {
            place: g.name.clone(),
            temp: c.get("temperature_2m")?.as_f64()? as f32,
            code: c.get("weather_code")?.as_i64()? as i32,
            day: c.get("is_day").and_then(|x| x.as_i64()).unwrap_or(1) == 1,
            hi: d.get("temperature_2m_max")?.get(0)?.as_f64()? as f32,
            lo: d.get("temperature_2m_min")?.get(0)?.as_f64()? as f32,
        })
    })();
    let error = if weather.is_some() { "" } else { "Offline" };
    WeatherResult { weather, geo: Some(g), error }
}

pub fn condition(code: i32) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Cloudy",
        45 | 48 => "Fog",
        51..=57 => "Drizzle",
        61 | 63 | 80 | 81 => "Rain",
        65 | 82 => "Heavy rain",
        66 | 67 => "Freezing rain",
        71..=77 | 85 | 86 => "Snow",
        95..=99 => "Thunderstorm",
        _ => "\u{2014}",
    }
}
