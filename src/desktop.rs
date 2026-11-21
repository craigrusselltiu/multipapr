use crate::layout::Rect;
use anyhow::{Context, Result, ensure};
use std::ptr::{null, null_mut};
use windows_sys::Win32::{
    Foundation::*,
    System::{LibraryLoader::GetModuleHandleW, StationsAndDesktops::*},
    UI::WindowsAndMessaging::*,
};

pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

pub const fn make_int_resource(id: u16) -> *const u16 {
    std::ptr::without_provenance(id as usize)
}

unsafe fn find(parent: HWND, after: HWND, class: &str) -> HWND {
    unsafe { FindWindowExW(parent, after, wide(class).as_ptr(), null()) }
}
unsafe fn wallpaper_layer(prog: HWND) -> Option<(HWND, HWND)> {
    unsafe {
        let view = find(prog, null_mut(), "SHELLDLL_DefView");
        if !view.is_null()
            && GetWindowLongPtrW(prog, GWL_EXSTYLE) as u32 & WS_EX_NOREDIRECTIONBITMAP != 0
        {
            return Some((prog, view));
        }
        let mut top = GetTopWindow(null_mut());
        while !top.is_null() {
            if !find(top, null_mut(), "SHELLDLL_DefView").is_null() {
                let worker = find(null_mut(), top, "WorkerW");
                return (!worker.is_null()).then_some((worker, HWND_TOP));
            }
            top = GetWindow(top, GW_HWNDNEXT);
        }
        None
    }
}
pub struct Surface {
    pub hwnd: HWND,
    parent: HWND,
}
impl Surface {
    pub fn create(r: Rect) -> Result<Self> {
        unsafe {
            let prog = find(null_mut(), null_mut(), "Progman");
            ensure!(!prog.is_null(), "Waiting for Windows Explorer");
            let mut layer = wallpaper_layer(prog);
            let title = wide("Multipapr Wallpaper");
            if layer.is_none_or(|(parent, _)| {
                FindWindowExW(parent, null_mut(), wide("STATIC").as_ptr(), title.as_ptr()).is_null()
            }) {
                let mut reply = 0;
                SendMessageTimeoutW(prog, 0x052c, 0xd, 0, SMTO_ABORTIFHUNG, 1000, &mut reply);
                SendMessageTimeoutW(prog, 0x052c, 0xd, 1, SMTO_ABORTIFHUNG, 1000, &mut reply);
                layer = wallpaper_layer(prog);
            }
            let (parent, insert_after) =
                layer.context("Waiting for Explorer’s wallpaper surface")?;
            let ex = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_LAYERED;
            let hwnd = CreateWindowExW(
                ex,
                wide("STATIC").as_ptr(),
                title.as_ptr(),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                0,
                0,
                r.w as i32,
                r.h as i32,
                parent,
                null_mut(),
                GetModuleHandleW(null()),
                null(),
            );
            ensure!(
                !hwnd.is_null(),
                "Cannot create wallpaper surface: {}",
                std::io::Error::last_os_error()
            );
            let surface = Self { hwnd, parent };
            surface.opacity(0)?;
            surface.place(r, insert_after)?;
            Ok(surface)
        }
    }
    pub fn raise(&self, desktop: Rect) -> Result<()> {
        unsafe {
            let (parent, anchor) = wallpaper_layer(find(null_mut(), null_mut(), "Progman"))
                .context("Waiting for Windows Explorer")?;
            ensure!(
                parent == self.parent,
                "Explorer recreated the wallpaper surface"
            );
            self.place(desktop, anchor)
        }
    }
    pub fn opacity(&self, alpha: u8) -> Result<()> {
        ensure!(
            unsafe { SetLayeredWindowAttributes(self.hwnd, 0, alpha, LWA_ALPHA) } != 0,
            "Cannot change wallpaper opacity: {}",
            std::io::Error::last_os_error()
        );
        Ok(())
    }
    fn place(&self, desktop: Rect, anchor: HWND) -> Result<()> {
        let mut pt = POINT {
            x: desktop.x as i32,
            y: desktop.y as i32,
        };
        unsafe {
            ensure!(
                windows_sys::Win32::Graphics::Gdi::ScreenToClient(self.parent, &mut pt) != 0
                    && SetWindowPos(
                        self.hwnd,
                        anchor,
                        pt.x,
                        pt.y,
                        desktop.w as i32,
                        desktop.h as i32,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW
                    ) != 0,
                "Cannot position wallpaper: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(())
    }
    pub fn maintain(&self, desktop: Rect) -> Result<()> {
        unsafe {
            let prog = find(null_mut(), null_mut(), "Progman");
            ensure!(!prog.is_null(), "Waiting for Windows Explorer");
            let (parent, anchor) =
                wallpaper_layer(prog).context("Waiting for Explorer’s wallpaper surface")?;
            ensure!(
                parent == self.parent && IsWindow(self.hwnd) != 0 && GetParent(self.hwnd) == parent,
                "Explorer recreated the wallpaper surface"
            );
            let background = if anchor == HWND_TOP {
                null_mut()
            } else {
                find(parent, anchor, "WorkerW")
            };
            self.maintain_position(desktop, anchor, background)
        }
    }
    fn maintain_position(&self, desktop: Rect, icons: HWND, background: HWND) -> Result<()> {
        unsafe {
            let mut actual: RECT = std::mem::zeroed();
            ensure!(
                GetWindowRect(self.hwnd, &mut actual) != 0,
                "Cannot locate wallpaper"
            );
            let on_monitor = actual.left == desktop.x as i32
                && actual.top == desktop.y as i32
                && actual.right - actual.left == desktop.w as i32
                && actual.bottom - actual.top == desktop.h as i32;
            let visible = GetWindowLongPtrW(self.hwnd, GWL_STYLE) as u32 & WS_VISIBLE != 0;
            let in_layer = if icons == HWND_TOP {
                true
            } else {
                let mut sibling = GetWindow(icons, GW_HWNDNEXT);
                while !sibling.is_null() && sibling != self.hwnd && sibling != background {
                    sibling = GetWindow(sibling, GW_HWNDNEXT);
                }
                sibling == self.hwnd
            };
            if !on_monitor || !visible || !in_layer {
                self.place(desktop, icons)?;
            }
            Ok(())
        }
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

pub fn locked() -> bool {
    unsafe {
        let input = OpenInputDesktop(0, 0, DESKTOP_READOBJECTS);
        if input.is_null() {
            return true;
        }
        let mut name = [0u16; 64];
        let mut needed = 0;
        let ok = GetUserObjectInformationW(
            input,
            UOI_NAME,
            name.as_mut_ptr().cast(),
            size_of_val(&name) as u32,
            &mut needed,
        );
        CloseDesktop(input);
        ok != 0
            && !String::from_utf16_lossy(&name)
                .trim_end_matches('\0')
                .eq_ignore_ascii_case("Default")
    }
}

pub fn battery_saver() -> bool {
    unsafe {
        let mut status = std::mem::zeroed();
        windows_sys::Win32::System::Power::GetSystemPowerStatus(&mut status) != 0
            && status.SystemStatusFlag == 1
    }
}

pub fn foreground_bounds() -> Option<Rect> {
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return None;
        }
        let mut class = [0u16; 128];
        let n = GetClassNameW(fg, class.as_mut_ptr(), 128);
        let class = String::from_utf16_lossy(&class[..n as usize]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return None;
        }
        let mut rect: RECT = std::mem::zeroed();
        (GetWindowRect(fg, &mut rect) != 0).then(|| {
            Rect::new(
                rect.left as f32,
                rect.top as f32,
                (rect.right - rect.left) as f32,
                (rect.bottom - rect.top) as f32,
            )
        })
    }
}
pub fn dispatch(handles: &[HANDLE], timeout: u32) -> u32 {
    unsafe {
        let ready = MsgWaitForMultipleObjectsEx(
            handles.len() as u32,
            handles.as_ptr(),
            timeout,
            QS_ALLINPUT,
            MWMO_INPUTAVAILABLE,
        );
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        ready
    }
}

pub struct Handle(pub HANDLE);
impl Handle {
    pub fn checked(handle: HANDLE) -> Result<Self> {
        (!handle.is_null())
            .then_some(Self(handle))
            .context("Cannot create Windows synchronization object")
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                CloseHandle(self.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restores_the_occluded_monitor_without_reordering_healthy_surfaces() {
        unsafe {
            let window = |parent: HWND, style| {
                let hwnd = CreateWindowExW(
                    WS_EX_NOACTIVATE,
                    wide("STATIC").as_ptr(),
                    null(),
                    style,
                    0,
                    0,
                    600,
                    400,
                    parent,
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                assert!(!hwnd.is_null());
                Surface { hwnd, parent }
            };
            let desk = window(null_mut(), WS_POPUP);
            let child = || window(desk.hwnd, WS_CHILD | WS_VISIBLE);
            let background = child();
            let second = child();
            let first = child();
            let icons = child();
            let reorder = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
            assert_ne!(SetWindowPos(icons.hwnd, HWND_TOP, 0, 0, 0, 0, reorder), 0);
            assert_ne!(
                SetWindowPos(background.hwnd, HWND_BOTTOM, 0, 0, 0, 0, reorder),
                0
            );
            let bounds = Rect::new(35., 50., 240., 120.);
            let repair = |s: &Surface| {
                s.maintain_position(bounds, icons.hwnd, background.hwnd)
                    .unwrap()
            };
            repair(&first);
            repair(&second);
            assert_ne!(
                SetWindowPos(second.hwnd, background.hwnd, 0, 0, 0, 0, reorder),
                0
            );
            assert_eq!(GetWindow(background.hwnd, GW_HWNDNEXT), second.hwnd);
            repair(&second);
            repair(&first);
            assert_eq!(GetWindow(icons.hwnd, GW_HWNDNEXT), second.hwnd);
            assert_eq!(GetWindow(second.hwnd, GW_HWNDNEXT), first.hwnd);
            assert_eq!(GetWindow(first.hwnd, GW_HWNDNEXT), background.hwnd);

            assert_ne!(
                SetWindowPos(
                    second.hwnd,
                    HWND_TOP,
                    0,
                    0,
                    10,
                    10,
                    SWP_NOACTIVATE | SWP_HIDEWINDOW
                ),
                0
            );
            repair(&second);
            assert_ne!(
                GetWindowLongPtrW(second.hwnd, GWL_STYLE) as u32 & WS_VISIBLE,
                0
            );
            let mut rect: RECT = std::mem::zeroed();
            assert_ne!(GetWindowRect(second.hwnd, &mut rect), 0);
            assert_eq!(
                [rect.left, rect.top, rect.right, rect.bottom],
                [35, 50, 275, 170]
            );
            assert_eq!(GetWindow(icons.hwnd, GW_HWNDNEXT), second.hwnd);
        }
    }
}
