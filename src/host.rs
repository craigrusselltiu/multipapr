use crate::{
    desktop::{self, Handle, Surface, wide},
    displays,
    layout::{Display, Layout, Rect, Settings},
    shader, storage,
};
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    io::Write,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    process::Child,
    ptr::null,
    time::{Duration, Instant, SystemTime},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{JobObjects::*, Threading::*},
};

const INSTANCE: &str = r"Local\Multipapr.Host.v1";
const STOP: &str = r"Local\Multipapr.Stop.v1";
pub fn running() -> bool {
    unsafe {
        let h = OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, wide(INSTANCE).as_ptr());
        if h.is_null() {
            false
        } else {
            CloseHandle(h);
            true
        }
    }
}
pub fn stop() -> Result<()> {
    unsafe {
        let event = OpenEventW(EVENT_MODIFY_STATE, 0, wide(STOP).as_ptr());
        if event.is_null() {
            ensure!(!running(), "Wallpaper is starting; try again");
            return Ok(());
        }
        let event = Handle(event);
        ensure!(SetEvent(event.0) != 0, "Cannot signal wallpaper host");
        Ok(())
    }
}

struct Player {
    child: Child,
    surface: Surface,
    _job: Handle,
    pipe: PathBuf,
    desktop: Rect,
    paused: bool,
}
impl Player {
    fn open(settings: &Settings, displays: &[Display]) -> Result<Self> {
        let layout = Layout::new(displays, &settings.calibration);
        let dir = storage::home()?;
        let shader_path = dir.join("layout.glsl");
        fs::write(&shader_path, shader::generate(displays, &layout, settings))?;
        let surface = Surface::create(layout.desktop)?;
        let pipe = PathBuf::from(format!(r"\\.\pipe\multipapr-player-{}", std::process::id()));
        let job = unsafe {
            let job = Handle::checked(CreateJobObjectW(null(), null()))?;
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            ensure!(
                SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    size_of_val(&limits) as u32
                ) != 0,
                "Cannot configure player lifetime"
            );
            job
        };
        let mut child = storage::command(storage::mpv()?)
            .args([
                "--no-config",
                "--vo=gpu-next",
                "--gpu-api=d3d11",
                "--hwdec=auto",
                "--no-audio",
                "--no-sub",
                "--loop-file=inf",
                "--image-display-duration=inf",
                "--keepaspect=no",
                "--no-osc",
                "--no-input-default-bindings",
                "--input-vo-keyboard=no",
                "--input-cursor=no",
                "--no-terminal",
                "--osd-level=0",
                "--cache=no",
                "--interpolation=no",
                "--d3d11-flip=no",
            ])
            .arg(format!("--wid={}", surface.hwnd as usize))
            .arg(format!("--glsl-shader={}", shader_path.display()))
            .arg(format!("--input-ipc-server={}", pipe.display()))
            .arg(format!("--log-file={}", dir.join("player.log").display()))
            .arg("--msg-level=all=warn")
            .arg("--")
            .arg(&settings.media.path)
            .spawn()
            .context("Cannot launch mpv")?;
        if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) } == 0 {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("Cannot supervise wallpaper player");
        }
        Ok(Self {
            child,
            surface,
            _job: job,
            pipe,
            desktop: layout.desktop,
            paused: false,
        })
    }
    fn update(&mut self) -> Result<()> {
        ensure!(
            self.surface.valid(),
            "Explorer recreated the wallpaper surface"
        );
        if let Some(status) = self.child.try_wait()? {
            anyhow::bail!("Wallpaper player exited ({status}). See player.log");
        }
        let pause = desktop::locked_or_covered(self.desktop);
        if pause != self.paused {
            if let Ok(mut ipc) = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(&self.pipe)
            {
                writeln!(
                    ipc,
                    "{}",
                    serde_json::json!({"command":["set_property","pause",pause]})
                )?;
                self.paused = pause;
            }
        }
        Ok(())
    }
}
impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn version(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}
fn signature(d: &[Display]) -> String {
    serde_json::to_string(d).unwrap_or_default()
}

pub fn run() -> Result<()> {
    let mutex = unsafe { CreateMutexW(null(), 0, wide(INSTANCE).as_ptr()) };
    let existing = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let _mutex = Handle::checked(mutex)?;
    if existing {
        return Ok(());
    }
    let event = Handle::checked(unsafe { CreateEventW(null(), 1, 0, wide(STOP).as_ptr()) })?;
    let dir = storage::home()?;
    let config = dir.join("layout.json");
    let mut stamp = None;
    let mut topology = String::new();
    let mut player: Option<Player> = None;
    let mut next = Instant::now();
    let mut recheck = Instant::now();
    let mut displays = Vec::new();
    let mut last_error = fs::read_to_string(dir.join("host-error.txt")).unwrap_or_default();
    loop {
        if unsafe { WaitForSingleObject(event.0, 0) } == WAIT_OBJECT_0 {
            break;
        }
        let now = Instant::now();
        if now >= next {
            let result = (|| -> Result<()> {
                if now >= recheck || displays.is_empty() {
                    displays = displays::detect()?;
                    recheck = now + Duration::from_secs(4);
                }
                let new_stamp = version(&config);
                let new_topology = signature(&displays);
                if player.is_none() || new_stamp != stamp || topology != new_topology {
                    let settings = storage::load()?;
                    drop(player.take());
                    player = Some(Player::open(&settings, &displays)?);
                    stamp = new_stamp;
                    topology = new_topology;
                }
                player.as_mut().unwrap().update()
            })();
            match result {
                Ok(()) => {
                    if !last_error.is_empty() {
                        let _ = fs::remove_file(dir.join("host-error.txt"));
                        last_error.clear();
                    }
                    next = now + Duration::from_secs(1);
                }
                Err(error) => {
                    drop(player.take());
                    let msg = format!("{error:#}");
                    if last_error != msg {
                        let _ = fs::write(dir.join("host-error.txt"), &msg);
                        last_error = msg;
                    }
                    next = now + Duration::from_secs(4);
                }
            }
        }
        desktop::dispatch(
            next.saturating_duration_since(Instant::now())
                .as_millis()
                .min(1000) as u32,
        );
    }
    Ok(())
}
