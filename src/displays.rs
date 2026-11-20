use crate::layout::{Display, Rect};
use anyhow::{Result, ensure};
use windows_sys::Win32::Graphics::Gdi::*;
use winreg::{RegKey, enums::HKEY_LOCAL_MACHINE};

fn string(buf: &[u16]) -> String {
    String::from_utf16_lossy(&buf[..buf.iter().position(|&x| x == 0).unwrap_or(buf.len())])
}
struct Panel {
    name: String,
    size: [f32; 2],
    estimated: bool,
}

fn hardware(id: &str) -> Option<Panel> {
    let mut parts = id.trim_start_matches(r"\\?\").split('#');
    let key = format!(
        r"SYSTEM\CurrentControlSet\Enum\{}\{}\{}\Device Parameters",
        parts.next()?,
        parts.next()?,
        parts.next()?
    );
    let data = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(key)
        .ok()?
        .get_raw_value("EDID")
        .ok()?
        .bytes;
    if data.len() < 128 || data[..8] != [0, 255, 255, 255, 255, 255, 255, 0] {
        return None;
    }
    let mut size = [data[21] as f32 * 10., data[22] as f32 * 10.];
    let mut name = String::new();
    let mut timing_size = None;
    let mut native_aspect = None;
    for d in data[54..126].chunks_exact(18) {
        if d[..2] != [0, 0] {
            if data[24] & 2 != 0 && native_aspect.is_none() {
                let width = d[2] as u32 | ((d[4] as u32 & 0xf0) << 4);
                let height = d[5] as u32 | ((d[7] as u32 & 0xf0) << 4);
                if width > 0 && height > 0 {
                    native_aspect = Some(width as f32 / height as f32);
                }
            }
            let w = d[12] as u32 + ((d[14] as u32 & 0xf0) << 4);
            let h = d[13] as u32 + ((d[14] as u32 & 15) << 8);
            if w > 50 && h > 50 {
                timing_size.get_or_insert([w as f32, h as f32]);
            }
        } else if d[3] == 0xfc {
            name = String::from_utf8_lossy(&d[5..18])
                .trim_matches(['\0', ' ', '\n', '\r'])
                .to_owned();
        }
    }
    if let Some(s) = timing_size {
        size = s;
    }
    if size[0] == 0. || size[1] == 0. {
        return None;
    }
    let mut estimated = false;
    if let Some(aspect) = native_aspect {
        if (size[0] / size[1] / aspect - 1.).abs() > 0.03 {
            // Keep the reported long edge and infer the short edge from the
            // preferred timing, assuming a square-pixel panel.
            if aspect >= 1. {
                size[1] = size[0] / aspect;
            } else {
                size[0] = size[1] * aspect;
            }
            estimated = true;
        }
    }
    Some(Panel {
        name,
        size,
        estimated,
    })
}

pub fn detect() -> Result<Vec<Display>> {
    let mut result = Vec::new();
    unsafe {
        for i in 0.. {
            let mut adapter: DISPLAY_DEVICEW = std::mem::zeroed();
            adapter.cb = size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(std::ptr::null(), i, &mut adapter, 0) == 0 {
                break;
            }
            if adapter.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP == 0 {
                continue;
            }
            let mut mode: DEVMODEW = std::mem::zeroed();
            mode.dmSize = size_of::<DEVMODEW>() as u16;
            if EnumDisplaySettingsExW(
                adapter.DeviceName.as_ptr(),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
                0,
            ) == 0
            {
                continue;
            }
            let mut monitor: DISPLAY_DEVICEW = std::mem::zeroed();
            monitor.cb = size_of::<DISPLAY_DEVICEW>() as u32;
            EnumDisplayDevicesW(adapter.DeviceName.as_ptr(), 0, &mut monitor, 1);
            let mut id = string(&monitor.DeviceID);
            if id.is_empty() {
                id = string(&adapter.DeviceName);
            }
            let state = mode.Anonymous1.Anonymous2;
            let Panel {
                mut name,
                size: mm,
                estimated,
            } = if let Some(mut panel) = hardware(&id) {
                if state.dmDisplayOrientation % 2 == 1 {
                    panel.size.swap(0, 1);
                }
                panel
            } else {
                Panel {
                    name: String::new(),
                    size: [
                        mode.dmPelsWidth as f32 * 25.4 / 96.,
                        mode.dmPelsHeight as f32 * 25.4 / 96.,
                    ],
                    estimated: true,
                }
            };
            if name.is_empty() {
                name = string(&monitor.DeviceString);
            }
            if name.is_empty() {
                name = string(&adapter.DeviceName);
            }
            result.push(Display {
                id,
                name,
                primary: adapter.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0,
                px: Rect::new(
                    state.dmPosition.x as f32,
                    state.dmPosition.y as f32,
                    mode.dmPelsWidth as f32,
                    mode.dmPelsHeight as f32,
                ),
                mm,
                estimated,
            });
        }
    }
    result.sort_by_key(|d| !d.primary);
    ensure!(!result.is_empty(), "Windows reported no active displays");
    Ok(result)
}
