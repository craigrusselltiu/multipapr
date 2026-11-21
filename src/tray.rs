use crate::desktop::wide;
use anyhow::{Result, ensure};
use std::{
    cell::Cell,
    io,
    ptr::{null, null_mut},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{
        LibraryLoader::GetModuleHandleW,
        Threading::{AttachThreadInput, GetCurrentThreadId},
    },
    UI::{Shell::*, WindowsAndMessaging::*},
};

const WINDOW_CLASS: &str = "Multipapr.Tray";
const CALLBACK: u32 = WM_APP + 1;
const KEY_SELECT: u32 = NIN_SELECT | NINF_KEY;
const CLOSE_ITEM: usize = 1;

#[derive(Clone, Copy)]
pub enum Command {
    Open,
    Close,
}

struct State {
    taskbar_created: u32,
    refresh: Cell<bool>,
    version4: Cell<bool>,
    command: Cell<Option<Command>>,
}

pub struct Tray {
    state: Box<State>,
    icon: NOTIFYICONDATAW,
    registered: bool,
}

impl Tray {
    pub fn new() -> Result<Self> {
        unsafe {
            let class_name = wide(WINDOW_CLASS);
            let instance = GetModuleHandleW(null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class_name.as_ptr(),
                ..std::mem::zeroed()
            };
            ensure!(
                RegisterClassW(&class) != 0,
                "Cannot register tray window: {}",
                io::Error::last_os_error()
            );

            let taskbar_created = RegisterWindowMessageW(wide("TaskbarCreated").as_ptr());
            ensure!(taskbar_created != 0, "Cannot register taskbar notification");
            let state = Box::new(State {
                taskbar_created,
                refresh: Cell::new(false),
                version4: Cell::new(false),
                command: Cell::new(None),
            });
            // Explorer broadcasts TaskbarCreated to top-level windows.
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                class_name.as_ptr(),
                null(),
                WS_POPUP,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                std::ptr::from_ref(state.as_ref()).cast(),
            );
            ensure!(
                !hwnd.is_null(),
                "Cannot create tray window: {}",
                io::Error::last_os_error()
            );

            let resource = 1usize as *const u16;
            let mut icon = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
                uCallbackMessage: CALLBACK,
                hIcon: LoadIconW(instance, resource),
                ..std::mem::zeroed()
            };
            for (dest, code) in icon.szTip.iter_mut().zip("Multipapr".encode_utf16()) {
                *dest = code;
            }
            Ok(Self {
                state,
                icon,
                registered: false,
            })
        }
    }

    pub fn update(&mut self) {
        if self.state.refresh.replace(false) {
            // A DPI change can also broadcast TaskbarCreated with the icon intact.
            self.registered = unsafe { Shell_NotifyIconW(NIM_MODIFY, &self.icon) != 0 };
        }
        if !self.registered {
            self.registered = unsafe { Shell_NotifyIconW(NIM_ADD, &self.icon) != 0 };
            if self.registered {
                self.icon.Anonymous.uVersion = NOTIFYICON_VERSION_4;
                let v4 = unsafe { Shell_NotifyIconW(NIM_SETVERSION, &self.icon) != 0 };
                self.state.version4.set(v4);
            }
        }
    }

    pub fn command(&self) -> Option<Command> {
        self.state.command.take()
    }

    pub fn allow_focus() {
        unsafe {
            let hwnd = FindWindowW(wide(WINDOW_CLASS).as_ptr(), null());
            if !hwnd.is_null() {
                let mut pid = 0;
                GetWindowThreadProcessId(hwnd, &mut pid);
                if pid != 0 {
                    AllowSetForegroundWindow(pid);
                }
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            Shell_NotifyIconW(NIM_DELETE, &self.icon);
            DestroyWindow(self.icon.hWnd);
        }
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lparam as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State;
        if let Some(state) = state.as_ref() {
            if message == state.taskbar_created {
                state.refresh.set(true);
                return 0;
            }
            if message == WM_CLOSE || (message == WM_ENDSESSION && wparam != 0) {
                state.command.set(Some(Command::Close));
                return 0;
            }
            if message == CALLBACK {
                let event = (lparam & 0xffff) as u32;
                let open_context_menu =
                    event == WM_CONTEXTMENU || (event == WM_RBUTTONUP && !state.version4.get());
                if open_context_menu {
                    let mut anchor = POINT {
                        x: (wparam as i16) as i32,
                        y: ((wparam >> 16) as i16) as i32,
                    };
                    if event == WM_RBUTTONUP || (anchor.x == -1 && anchor.y == -1) {
                        GetCursorPos(&mut anchor);
                    }
                    if !matches!(state.command.get(), Some(Command::Close))
                        && show_menu(hwnd, anchor)
                    {
                        state.command.set(Some(Command::Close));
                    }
                } else if matches!(event, NIN_SELECT | KEY_SELECT)
                    || (event == WM_LBUTTONUP && !state.version4.get())
                {
                    if !matches!(state.command.get(), Some(Command::Close)) {
                        state.command.set(Some(Command::Open));
                    }
                }
                return 0;
            }
        }
        DefWindowProcW(hwnd, message, wparam, lparam)
    }
}

fn show_menu(hwnd: HWND, anchor: POINT) -> bool {
    unsafe {
        let menu = CreatePopupMenu();
        if menu.is_null() {
            return false;
        }
        AppendMenuW(menu, MF_STRING, CLOSE_ITEM, wide("Close").as_ptr());
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
        if SetForegroundWindow(hwnd) == 0 {
            let fg = GetForegroundWindow();
            let fg_thread = GetWindowThreadProcessId(fg, null_mut());
            let my_thread = GetCurrentThreadId();
            if fg_thread != 0 && fg_thread != my_thread {
                AttachThreadInput(my_thread, fg_thread, 1);
                SetForegroundWindow(hwnd);
                AttachThreadInput(my_thread, fg_thread, 0);
            }
        }
        let align = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 {
            TPM_RIGHTALIGN
        } else {
            TPM_LEFTALIGN
        };
        let selected = TrackPopupMenuEx(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN | align,
            anchor.x,
            anchor.y,
            hwnd,
            null(),
        );
        // Let Explorer dismiss the menu normally on the next interaction.
        PostMessageW(hwnd, WM_NULL, 0, 0);
        DestroyMenu(menu);
        if selected == 0 {
            let icon = NOTIFYICONDATAW {
                cbSize: size_of::<NOTIFYICONDATAW>() as u32,
                hWnd: hwnd,
                uID: 1,
                ..std::mem::zeroed()
            };
            Shell_NotifyIconW(NIM_SETFOCUS, &icon);
        }
        selected == CLOSE_ITEM as i32
    }
}
