use crate::desktop::Handle;
use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsStr,
    fs::File,
    io::{self, PipeReader, PipeWriter, Read},
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    path,
    process::{Child, Command},
    ptr::{null, null_mut},
    sync::{Mutex, MutexGuard, PoisonError},
    thread,
};
use windows_sys::Win32::{
    Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation, WAIT_OBJECT_0},
    System::Threading::{
        CREATE_NO_WINDOW, CreateProcessW, DeleteProcThreadAttributeList,
        EXTENDED_STARTUPINFO_PRESENT, GetExitCodeProcess, InitializeProcThreadAttributeList,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROCESS_INFORMATION,
        STARTF_FORCEOFFFEEDBACK, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOEXW,
        TerminateProcess, UpdateProcThreadAttribute, WaitForSingleObject,
    },
    UI::WindowsAndMessaging::SW_HIDE,
};

static LAUNCH_LOCK: Mutex<()> = Mutex::new(());

fn lock_launches() -> MutexGuard<'static, ()> {
    LAUNCH_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn spawn(command: &mut Command) -> io::Result<Child> {
    let _guard = lock_launches();
    command.spawn()
}

pub fn decoder_output(command: &Command) -> Result<Vec<u8>> {
    let (child, input, output) = spawn_preview_decoder(command)?;
    drop(input);
    thread::scope(|scope| {
        let reader = scope.spawn(|| {
            let mut bytes = Vec::new();
            output.take(64 * 1024).read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        });
        ensure!(
            decoder_succeeded(&child, 30_000),
            "Could not decode the wallpaper"
        );
        reader
            .join()
            .map_err(|_| anyhow::anyhow!("Wallpaper decoder stopped"))?
            .context("Cannot read the decoder output")
    })
}

pub fn decoder_succeeded(child: &Handle, timeout_ms: u32) -> bool {
    unsafe {
        if WaitForSingleObject(child.0, timeout_ms) == WAIT_OBJECT_0 {
            let mut exit = 0;
            GetExitCodeProcess(child.0, &mut exit) != 0 && exit == 0
        } else {
            TerminateProcess(child.0, 1);
            false
        }
    }
}

fn quote_argument(command_line: &mut Vec<u16>, argument: &OsStr) -> Result<()> {
    let slash = u16::from(b'\\');
    let quote = u16::from(b'"');
    command_line.push(quote);
    let mut slashes = 0;
    for unit in argument.encode_wide() {
        ensure!(unit != 0, "Invalid wallpaper decoder argument");
        if unit == slash {
            slashes += 1;
        } else {
            let escapes = if unit == quote {
                slashes * 2 + 1
            } else {
                slashes
            };
            command_line.extend(std::iter::repeat_n(slash, escapes));
            command_line.push(unit);
            slashes = 0;
        }
    }
    command_line.extend(std::iter::repeat_n(slash, slashes * 2));
    command_line.push(quote);
    Ok(())
}

pub fn spawn_preview_decoder(command: &Command) -> Result<(Handle, PipeWriter, PipeReader)> {
    let program = path::absolute(command.get_program())?;
    let mut arguments = Vec::new();
    quote_argument(&mut arguments, program.as_os_str())?;
    for argument in command.get_args() {
        arguments.push(u16::from(b' '));
        quote_argument(&mut arguments, argument)?;
    }
    arguments.push(0);
    let program: Vec<_> = program.as_os_str().encode_wide().chain([0]).collect();

    // Keep other launches from inheriting the decoder's pipes while these ends are inheritable.
    let _guard = lock_launches();
    let (child_input, input) = io::pipe()?;
    let (output, child_output) = io::pipe()?;
    let child_error = File::options().write(true).open(r"\\.\NUL")?;
    let handles = [
        child_input.as_raw_handle(),
        child_output.as_raw_handle(),
        child_error.as_raw_handle(),
    ];
    unsafe {
        for handle in handles {
            ensure!(
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) != 0,
                "Cannot prepare decoder pipe: {}",
                io::Error::last_os_error()
            );
        }
        let mut size = 0;
        InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        ensure!(size > 0, "Cannot size decoder process attributes");
        let mut buffer = vec![0usize; size.div_ceil(size_of::<usize>())];
        let attributes = buffer.as_mut_ptr().cast();
        ensure!(
            InitializeProcThreadAttributeList(attributes, 1, 0, &mut size) != 0,
            "Cannot initialize decoder process attributes: {}",
            io::Error::last_os_error()
        );
        struct AttributeList(LPPROC_THREAD_ATTRIBUTE_LIST);
        impl Drop for AttributeList {
            fn drop(&mut self) {
                unsafe { DeleteProcThreadAttributeList(self.0) };
            }
        }
        let _attributes = AttributeList(attributes);
        ensure!(
            UpdateProcThreadAttribute(
                attributes,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                size_of_val(&handles),
                null_mut(),
                null(),
            ) != 0,
            "Cannot set decoder process handles: {}",
            io::Error::last_os_error()
        );
        let mut startup: STARTUPINFOEXW = std::mem::zeroed();
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags =
            STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW | STARTF_FORCEOFFFEEDBACK;
        startup.StartupInfo.wShowWindow = SW_HIDE as u16;
        startup.StartupInfo.hStdInput = handles[0];
        startup.StartupInfo.hStdOutput = handles[1];
        startup.StartupInfo.hStdError = handles[2];
        startup.lpAttributeList = attributes;
        let mut child: PROCESS_INFORMATION = std::mem::zeroed();
        ensure!(
            CreateProcessW(
                program.as_ptr(),
                arguments.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_NO_WINDOW | EXTENDED_STARTUPINFO_PRESENT,
                null(),
                null(),
                &startup.StartupInfo,
                &mut child,
            ) != 0,
            "{}",
            io::Error::last_os_error()
        );
        let _thread = Handle(child.hThread);
        Ok((Handle(child.hProcess), input, output))
    }
}
