//! Build script:
//! 1. Draws the app icon (no image files in the repo) and writes app.ico.
//! 2. Embeds icon, version info and manifest into both exes via windres.
//! 3. When FROST_PAYLOAD points at a built frost.exe, compresses it for the installer.
use std::{env, fs, path::PathBuf, process::Command};

fn main() {
    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=FROST_PAYLOAD");
    println!("cargo:rerun-if-env-changed=FROST_UIACCESS");

    // Installer payload: [u32 original size][deflate stream]
    let payload = env::var("FROST_PAYLOAD").ok().and_then(|p| {
        println!("cargo:rerun-if-changed={p}");
        fs::read(p).ok()
    });
    let data = match payload {
        Some(b) => {
            let mut v = (b.len() as u32).to_le_bytes().to_vec();
            v.extend(miniz_oxide::deflate::compress_to_vec(&b, 10));
            v
        }
        None => Vec::new(),
    };
    fs::write(out.join("payload.bin"), data).unwrap();

    // Icon + resources
    let ico = out.join("app.ico");
    fs::write(&ico, make_ico(&[16, 20, 24, 32, 40, 48, 64, 96, 128, 256])).unwrap();
    // frost.exe asks for UI Access (its taskbar pill may sit above Start and search);
    // Windows only grants that to a signed exe in a protected folder like Program Files.
    // The setup asks for admin, since it installs into Program Files.
    // Only the UI Access edition (FROST_UIACCESS=1, see build-setup.ps1 -UIAccess) does
    // this; the normal edition installs per user with no admin prompt or signing.
    let uia = env::var_os("FROST_UIACCESS").is_some();
    let manifest = out.join("app.manifest");
    let setup_manifest = out.join("setup.manifest");
    if uia {
        fs::write(&manifest, MANIFEST.replace("uiAccess=\"false\"", "uiAccess=\"true\"")).unwrap();
        fs::write(&setup_manifest, MANIFEST.replace("level=\"asInvoker\"", "level=\"requireAdministrator\"")).unwrap();
    } else {
        fs::write(&manifest, MANIFEST).unwrap();
        fs::write(&setup_manifest, MANIFEST).unwrap();
    }
    let ver = env::var("CARGO_PKG_VERSION").unwrap();
    let nums: Vec<&str> = ver.split('.').collect();
    let comma = format!("{},{},{},0", nums[0], nums.get(1).unwrap_or(&"0"), nums.get(2).unwrap_or(&"0"));
    let fwd = |p: &PathBuf| p.display().to_string().replace('\\', "/");
    for (bin, desc, file) in [
        ("frost", "Frost Widgets", "frost.exe"),
        ("frost-setup", "Frost Widgets Setup", "FrostWidgets-Setup.exe"),
    ] {
        let rc = out.join(format!("{bin}.rc"));
        let text = format!(
            r#"1 ICON "{ico}"
{man}
1 VERSIONINFO
FILEVERSION {comma}
PRODUCTVERSION {comma}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Frost Widgets"
      VALUE "FileDescription", "{desc}"
      VALUE "FileVersion", "{ver}"
      VALUE "InternalName", "{bin}"
      VALUE "LegalCopyright", "Copyright (c) 2026"
      VALUE "OriginalFilename", "{file}"
      VALUE "ProductName", "Frost Widgets"
      VALUE "ProductVersion", "{ver}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
            ico = fwd(&ico),
            man = format!("1 24 \"{}\"", fwd(if bin == "frost" { &manifest } else { &setup_manifest })),
        );
        fs::write(&rc, text).unwrap();
        let obj = out.join(format!("{bin}-res.o"));
        let ok = Command::new("windres")
            .arg("-i")
            .arg(&rc)
            .arg("-o")
            .arg(&obj)
            .args(["-O", "coff"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            println!("cargo:rustc-link-arg-bin={bin}={}", obj.display());
        } else {
            println!("cargo:warning=windres not found: {bin}.exe is built without icon and version info");
        }
    }
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application><supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/></application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
  </dependentAssembly></dependency>
</assembly>
"#;

// ------------------------------------------------------------------ icon art

fn sdf(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - hw + r;
    let qy = (py - cy).abs() - hh + r;
    let (ox, oy) = (qx.max(0.0), qy.max(0.0));
    let outer = if ox > 0.0 && oy > 0.0 { (ox.powf(2.6) + oy.powf(2.6)).powf(1.0 / 2.6) } else { ox + oy };
    outer + qx.max(qy).min(0.0) - r
}

fn over(dst: &mut [f32; 4], c: [f32; 3], a: f32) {
    if a <= 0.0 {
        return;
    }
    for i in 0..3 {
        dst[i] = dst[i] * (1.0 - a) + c[i] * a;
    }
    dst[3] = dst[3] + a * (1.0 - dst[3]);
}

/// Glass widget card on a pink-to-violet squircle. Straight-alpha RGBA.
fn icon_rgba(n: u32) -> Vec<u8> {
    let ss = 4;
    let detail = n >= 40;
    let mut px = vec![0u8; (n * n * 4) as usize];
    for y in 0..n {
        for x in 0..n {
            let mut acc = [0.0f32; 4];
            for sy in 0..ss {
                for sx in 0..ss {
                    let u = (x as f32 + (sx as f32 + 0.5) / ss as f32) / n as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / ss as f32) / n as f32;
                    let mut c = [0.0f32; 4];
                    if sdf(u, v, 0.5, 0.5, 0.46, 0.46, 0.2) < 0.0 {
                        let t = ((u + v) / 2.0).clamp(0.0, 1.0);
                        let a = [1.0, 0.45, 0.58];
                        let b = [0.42, 0.36, 1.0];
                        let g = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
                        over(&mut c, g, 1.0);
                        // top sheen
                        if v < 0.5 {
                            over(&mut c, [1.0; 3], 0.10 * (1.0 - v * 2.0));
                        }
                        // glass card
                        let d = sdf(u, v, 0.5, 0.55, 0.31, 0.24, 0.09);
                        if d < 0.0 {
                            over(&mut c, [1.0; 3], 0.30);
                            if detail {
                                // ring
                                let r = ((u - 0.355).powi(2) + (v - 0.55).powi(2)).sqrt();
                                if (0.07..0.11).contains(&r) {
                                    over(&mut c, [1.0; 3], 0.95);
                                }
                                // bars
                                for (by, bw) in [(0.5, 0.17), (0.6, 0.12)] {
                                    if sdf(u, v, 0.56 + bw / 2.0, by, bw / 2.0, 0.022, 0.022) < 0.0 {
                                        over(&mut c, [1.0; 3], 0.9);
                                    }
                                }
                            }
                        }
                        if d < 0.0 && d > -0.014 && v < 0.55 {
                            over(&mut c, [1.0; 3], 0.55);
                        }
                    }
                    for i in 0..3 {
                        acc[i] += c[i] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let n2 = (ss * ss) as f32;
            let a = acc[3] / n2;
            let o = ((y * n + x) * 4) as usize;
            for i in 0..3 {
                px[o + i] = if a > 0.0 { ((acc[i] / acc[3]) * 255.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
            }
            px[o + 3] = (a * 255.0).round() as u8;
        }
    }
    px
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
    }
    !c
}

fn png(n: u32, rgba: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity(rgba.len() + n as usize);
    for row in rgba.chunks((n * 4) as usize) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut chunk = |kind: &[u8], body: &[u8]| {
        out.extend((body.len() as u32).to_be_bytes());
        let mut t = kind.to_vec();
        t.extend_from_slice(body);
        out.extend_from_slice(&t);
        out.extend(crc32(&t).to_be_bytes());
    };
    let mut ihdr = n.to_be_bytes().to_vec();
    ihdr.extend(n.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &miniz_oxide::deflate::compress_to_vec_zlib(&raw, 9));
    chunk(b"IEND", &[]);
    out
}

fn make_ico(sizes: &[u32]) -> Vec<u8> {
    let images: Vec<Vec<u8>> = sizes.iter().map(|&n| png(n, &icon_rgba(n))).collect();
    let mut out = vec![0, 0, 1, 0];
    out.extend((sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&n, img) in sizes.iter().zip(&images) {
        let b = if n >= 256 { 0 } else { n as u8 };
        out.extend([b, b, 0, 0]);
        out.extend(1u16.to_le_bytes());
        out.extend(32u16.to_le_bytes());
        out.extend((img.len() as u32).to_le_bytes());
        out.extend(offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        out.extend(img);
    }
    out
}
