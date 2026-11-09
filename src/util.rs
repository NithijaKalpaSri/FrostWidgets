#![allow(dead_code)] // shared with the installer, which uses a different subset
use windows::core::PCWSTR;
use windows::Win32::Foundation::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

/// Null-terminated UTF-16 for Win32 calls.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    String::from_utf16_lossy(&buf[..n.max(0) as usize])
}

pub fn reg_dword(root: HKEY, sub: &str, name: &str) -> Option<u32> {
    let (sub, name) = (wide(sub), wide(name));
    let mut data = 0u32;
    let mut size = 4u32;
    let r = unsafe {
        RegGetValueW(
            root,
            PCWSTR(sub.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (r == ERROR_SUCCESS).then_some(data)
}

pub fn reg_string(root: HKEY, sub: &str, name: &str) -> Option<String> {
    let (sub, name) = (wide(sub), wide(name));
    let mut buf = [0u16; 1024];
    let mut size = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(
            root,
            PCWSTR(sub.as_ptr()),
            PCWSTR(name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if r != ERROR_SUCCESS {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
    Some(String::from_utf16_lossy(&buf[..len]))
}

pub fn reg_set_string(root: HKEY, sub: &str, name: &str, value: &str) {
    let (sub, name, val) = (wide(sub), wide(name), wide(value));
    unsafe {
        let _ = RegSetKeyValueW(
            root,
            PCWSTR(sub.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_SZ.0,
            Some(val.as_ptr() as *const _),
            (val.len() * 2) as u32,
        );
    }
}

pub fn reg_set_dword(root: HKEY, sub: &str, name: &str, value: u32) {
    let (sub, name) = (wide(sub), wide(name));
    unsafe {
        let _ = RegSetKeyValueW(
            root,
            PCWSTR(sub.as_ptr()),
            PCWSTR(name.as_ptr()),
            REG_DWORD.0,
            Some(&value as *const u32 as *const _),
            4,
        );
    }
}

pub fn reg_delete_tree(root: HKEY, sub: &str) {
    let sub = wide(sub);
    unsafe {
        let _ = RegDeleteTreeW(root, PCWSTR(sub.as_ptr()));
    }
}

pub fn reg_delete(root: HKEY, sub: &str, name: &str) {
    let (sub, name) = (wide(sub), wide(name));
    unsafe {
        let _ = RegDeleteKeyValueW(root, PCWSTR(sub.as_ptr()), PCWSTR(name.as_ptr()));
    }
}
