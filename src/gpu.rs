//! GPU usage, measured like Task Manager: per adapter, the busiest engine type
//! (3D, copy, video...) summed over all processes, from the "GPU Engine"
//! performance counters. Only loaded while the System widget shows a GPU ring.
use windows::core::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::System::Performance::*;

#[derive(Clone, Debug, PartialEq)]
pub struct Adapter {
    pub name: String,
    pub luid: i64,
}

/// Hardware adapters, the one with the most video memory first.
pub fn adapters() -> Vec<Adapter> {
    let mut out: Vec<(u64, Adapter)> = Vec::new();
    unsafe {
        let Ok(f) = CreateDXGIFactory1::<IDXGIFactory1>() else { return Vec::new() };
        let mut i = 0;
        while let Ok(a) = f.EnumAdapters1(i) {
            i += 1;
            let Ok(d) = a.GetDesc1() else { continue };
            if d.Flags & (DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32) != 0 || d.VendorId == 0x1414 {
                continue; // Microsoft Basic Render Driver
            }
            let n = d.Description.iter().position(|&c| c == 0).unwrap_or(d.Description.len());
            let name = String::from_utf16_lossy(&d.Description[..n]).trim().to_string();
            let luid = ((d.AdapterLuid.HighPart as i64) << 32) | d.AdapterLuid.LowPart as i64;
            if out.iter().any(|(_, a)| a.luid == luid) {
                continue;
            }
            out.push((d.DedicatedVideoMemory as u64, Adapter { name, luid }));
        }
    }
    out.sort_by(|a, b| b.0.cmp(&a.0));
    out.into_iter().map(|(_, a)| a).collect()
}

/// Doubles, not capped at 100 (per-process values are summed).
const FMT: PDH_FMT = PDH_FMT(PDH_FMT_DOUBLE.0 | 0x8000);

pub struct Gpu {
    query: isize,
    counter: isize,
    pub luid: i64,
    buf: Vec<u8>,
}

impl Gpu {
    pub fn new(luid: i64) -> Option<Self> {
        unsafe {
            let mut query = 0isize;
            if PdhOpenQueryW(PCWSTR::null(), 0, &mut query) != 0 {
                return None;
            }
            let mut counter = 0isize;
            if PdhAddEnglishCounterW(query, w!("\\GPU Engine(*)\\Utilization Percentage"), 0, &mut counter) != 0 {
                let _ = PdhCloseQuery(query);
                return None;
            }
            let _ = PdhCollectQueryData(query);
            Some(Gpu { query, counter, luid, buf: Vec::new() })
        }
    }

    /// Usage in percent since the previous call.
    pub fn sample(&mut self) -> Option<f32> {
        unsafe {
            if PdhCollectQueryData(self.query) != 0 {
                return None;
            }
            let (mut size, mut count) = (0u32, 0u32);
            let _ = PdhGetFormattedCounterArrayW(self.counter, FMT, &mut size, &mut count, None);
            if size == 0 {
                return Some(0.0);
            }
            if self.buf.len() < size as usize {
                self.buf.resize(size as usize, 0);
            }
            let items = self.buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
            if PdhGetFormattedCounterArrayW(self.counter, FMT, &mut size, &mut count, Some(items)) != 0 {
                return None;
            }
            let items = std::slice::from_raw_parts(items, count as usize);
            // Sum per engine type for our adapter, then take the busiest type.
            let mut per_type: Vec<(String, f64)> = Vec::new();
            for it in items {
                if it.FmtValue.CStatus != 0 {
                    continue;
                }
                let name = it.szName.to_string().unwrap_or_default();
                let Some((luid, ty)) = parse(&name) else { continue };
                if luid != self.luid {
                    continue;
                }
                let v = it.FmtValue.Anonymous.doubleValue;
                match per_type.iter_mut().find(|(t, _)| *t == ty) {
                    Some(e) => e.1 += v,
                    None => per_type.push((ty.to_string(), v)),
                }
            }
            Some(per_type.iter().map(|e| e.1).fold(0.0, f64::max).clamp(0.0, 100.0) as f32)
        }
    }
}

impl Drop for Gpu {
    fn drop(&mut self) {
        unsafe {
            let _ = PdhCloseQuery(self.query);
        }
    }
}

/// "pid_1234_luid_0x00000000_0x0000D1E2_phys_0_eng_0_engtype_3D" -> (luid, "3D")
fn parse(s: &str) -> Option<(i64, &str)> {
    let rest = &s[s.find("luid_0x")? + 7..];
    let hi = i64::from_str_radix(rest.get(..8)?, 16).ok()?;
    let lo = i64::from_str_radix(rest.get(11..19)?, 16).ok()?;
    let ty = &s[s.find("engtype_")? + 8..];
    Some(((hi << 32) | lo, ty))
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_instance_names() {
        let (l, t) = super::parse("pid_1234_luid_0x00000001_0x0000D1E2_phys_0_eng_0_engtype_3D").unwrap();
        assert_eq!(l, (1i64 << 32) | 0xD1E2);
        assert_eq!(t, "3D");
    }

    #[test]
    #[ignore]
    fn live_sample() {
        for a in super::adapters() {
            let mut g = super::Gpu::new(a.luid).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(1000));
            println!("{} luid={:#x} -> {:?}", a.name, a.luid, g.sample());
        }
    }
}