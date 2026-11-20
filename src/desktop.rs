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
unsafe fn find(parent: HWND, after: HWND, class: &str) -> HWND {
    unsafe { FindWindowExW(parent, after, wide(class).as_ptr(), null()) }
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
            let mut reply = 0;
            SendMessageTimeoutW(prog, 0x052c, 0xd, 0, SMTO_ABORTIFHUNG, 1000, &mut reply);
            SendMessageTimeoutW(prog, 0x052c, 0xd, 1, SMTO_ABORTIFHUNG, 1000, &mut reply);
            let view = find(prog, null_mut(), "SHELLDLL_DefView");
            let raised = GetWindowLongPtrW(prog, GWL_EXSTYLE) as u32 & WS_EX_NOREDIRECTIONBITMAP
                != 0
                && !view.is_null();
            let parent = if raised {
                prog
            } else {
                let mut top = GetTopWindow(null_mut());
                let mut worker = null_mut();
                while !top.is_null() {
                    if !find(top, null_mut(), "SHELLDLL_DefView").is_null() {
                        worker = find(null_mut(), top, "WorkerW");
                        break;
                    }
                    top = GetWindow(top, GW_HWNDNEXT);
                }
                worker
            };
            ensure!(
                !parent.is_null(),
                "Waiting for Explorer’s wallpaper surface"
            );
            let mut pt = POINT {
                x: r.x as i32,
                y: r.y as i32,
            };
            windows_sys::Win32::Graphics::Gdi::ScreenToClient(parent, &mut pt);
            let ex = WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | if raised { WS_EX_LAYERED } else { 0 };
            let hwnd = CreateWindowExW(
                ex,
                wide("STATIC").as_ptr(),
                wide("Multipapr Wallpaper").as_ptr(),
                WS_CHILD | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
                pt.x,
                pt.y,
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
            if raised {
                ensure!(
                    SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA) != 0,
                    "Cannot configure desktop composition"
                );
            }
            ensure!(
                SetWindowPos(
                    hwnd,
                    if raised { view } else { HWND_TOP },
                    pt.x,
                    pt.y,
                    r.w as i32,
                    r.h as i32,
                    SWP_NOACTIVATE | SWP_SHOWWINDOW
                ) != 0,
                "Cannot position wallpaper"
            );
            Ok(surface)
        }
    }
    pub fn valid(&self) -> bool {
        unsafe { IsWindow(self.hwnd) != 0 && GetParent(self.hwnd) == self.parent }
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}

pub fn locked_or_covered(desktop: Rect) -> bool {
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
        if ok != 0
            && !String::from_utf16_lossy(&name)
                .trim_end_matches('\0')
                .eq_ignore_ascii_case("Default")
        {
            return true;
        }
        let fg = GetForegroundWindow();
        if fg.is_null() {
            return false;
        }
        let mut class = [0u16; 128];
        let n = GetClassNameW(fg, class.as_mut_ptr(), 128);
        let class = String::from_utf16_lossy(&class[..n as usize]);
        if matches!(class.as_str(), "Progman" | "WorkerW" | "Shell_TrayWnd") {
            return false;
        }
        let mut rect: RECT = std::mem::zeroed();
        GetWindowRect(fg, &mut rect) != 0
            && rect.left <= desktop.x as i32
            && rect.top <= desktop.y as i32
            && rect.right >= desktop.right() as i32
            && rect.bottom >= desktop.bottom() as i32
    }
}
pub fn dispatch(timeout: u32) {
    unsafe {
        MsgWaitForMultipleObjects(0, null(), 0, timeout, QS_ALLINPUT);
        let mut msg: MSG = std::mem::zeroed();
        while PeekMessageW(&mut msg, null_mut(), 0, 0, PM_REMOVE) != 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
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
