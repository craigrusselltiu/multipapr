mod playback;
use crate::{
    desktop::{self, Handle, Surface, wide},
    displays,
    layout::{Display, Layout, Rect, Settings, display_set, follow_displays},
    shader::{self, Part},
    storage::{self, PlaybackAction, Preferences},
    tray::{self, Tray},
    update,
};
use anyhow::{Context, Result, ensure};
use playback::{PlaybackPolicy, Players};
use std::{
    collections::BTreeMap,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, Write},
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
    time::{Duration, Instant, SystemTime},
};
use windows_sys::Win32::{
    Foundation::*,
    System::{JobObjects::*, Threading::*},
    UI::WindowsAndMessaging::*,
};

const INSTANCE: &str = r"Local\Multipapr.Host.v1";
const STOP: &str = r"Local\Multipapr.Stop.v1";
const OPEN: &str = r"Local\Multipapr.OpenEditor.v1";
const RELAUNCH: &str = r"Local\Multipapr.Relaunch.v1";

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
pub fn relaunch() -> Result<bool> {
    unsafe {
        let event = OpenEventW(EVENT_MODIFY_STATE, 0, wide(RELAUNCH).as_ptr());
        if event.is_null() {
            ensure!(!running(), "Wallpaper is starting; try again");
            return Ok(false);
        }
        let event = Handle(event);
        ensure!(SetEvent(event.0) != 0, "Cannot signal wallpaper host");
        Ok(true)
    }
}

pub fn start() -> Result<()> {
    if !running() {
        storage::command(std::env::current_exe()?)
            .arg("--background")
            .spawn()
            .context("Cannot start Multipapr")?;
    }
    Ok(())
}

struct ManagedChild {
    child: Child,
    _job: Handle,
}

impl ManagedChild {
    fn spawn(command: &mut Command) -> Result<Self> {
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
                "Cannot configure child process lifetime"
            );
            job
        };
        let mut child = command.spawn()?;
        if unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle()) } == 0 {
            let error = io::Error::last_os_error();
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("Cannot supervise child process");
        }
        Ok(Self { child, _job: job })
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct EditorProcess(Option<ManagedChild>);

impl EditorProcess {
    fn open(&mut self, exe: &Path) -> Result<()> {
        if let Some(editor) = self.0.as_mut()
            && editor.child.try_wait()?.is_none()
        {
            let hwnd = editor_window(editor.child.id());
            if !hwnd.is_null() {
                unsafe {
                    if IsIconic(hwnd) != 0 {
                        ShowWindowAsync(hwnd, SW_RESTORE);
                    }
                    SetForegroundWindow(GetLastActivePopup(hwnd));
                }
            }
            return Ok(());
        }
        let editor = ManagedChild::spawn(
            storage::command(exe)
                .arg("--editor")
                .arg(std::process::id().to_string()),
        )
        .context("Cannot open Multipapr editor")?;
        unsafe {
            AllowSetForegroundWindow(editor.child.id());
        }
        self.0 = Some(editor);
        Ok(())
    }

    fn running(&self) -> bool {
        self.0.is_some()
    }

    fn reap(&mut self) -> Result<()> {
        if let Some(editor) = self.0.as_mut()
            && editor.child.try_wait()?.is_some()
        {
            self.0 = None;
        }
        Ok(())
    }
}

impl Drop for EditorProcess {
    fn drop(&mut self) {
        if let Some(editor) = self.0.as_ref() {
            let hwnd = editor_window(editor.child.id());
            if !hwnd.is_null() {
                unsafe {
                    PostMessageW(hwnd, WM_CLOSE, 0, 0);
                    WaitForSingleObject(editor.child.as_raw_handle(), 500);
                }
            }
        }
    }
}

fn editor_window(pid: u32) -> HWND {
    struct Search {
        pid: u32,
        hwnd: HWND,
    }
    unsafe extern "system" fn find(hwnd: HWND, data: LPARAM) -> BOOL {
        unsafe {
            let search = &mut *(data as *mut Search);
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, &mut pid);
            if pid == search.pid
                && IsWindowVisible(hwnd) != 0
                && GetWindow(hwnd, GW_OWNER).is_null()
            {
                search.hwnd = hwnd;
                return 0;
            }
            1
        }
    }
    let mut search = Search {
        pid,
        hwnd: null_mut(),
    };
    unsafe {
        EnumWindows(Some(find), std::ptr::from_mut(&mut search) as LPARAM);
    }
    search.hwnd
}

pub fn player_pipe(host_id: u32, multi: bool, part: &Part, displays: &[Display]) -> PathBuf {
    let mut hash = DefaultHasher::new();
    multi.hash(&mut hash);
    part.media.hash(&mut hash);
    for &i in &part.displays {
        let d = &displays[i];
        d.id.hash(&mut hash);
        [d.px.x, d.px.y, d.px.w, d.px.h]
            .map(f32::to_bits)
            .hash(&mut hash);
    }
    PathBuf::from(format!(
        r"\\.\pipe\multipapr-player-{host_id}-{:016x}",
        hash.finish()
    ))
}

pub fn update_mapping(pipe: &Path, parameters: &str) -> Result<()> {
    set_player_property(pipe, "glsl-shader-opts", parameters)
}
fn set_player_property(pipe: &Path, name: &str, value: impl serde::Serialize) -> Result<()> {
    let mut ipc = fs::OpenOptions::new().read(true).write(true).open(pipe)?;
    writeln!(
        ipc,
        "{}",
        serde_json::json!({"command":["set_property", name, value]})
    )?;
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq)]
pub struct PlaybackReport {
    pub host: u32,
    pub layout_modified: Option<SystemTime>,
    #[serde(default)]
    pub loading: bool,
    pub displays: BTreeMap<String, (PathBuf, String)>,
}
impl PlaybackReport {
    pub fn switch_finished(&self, requested: SystemTime) -> bool {
        !self.loading
            && self
                .layout_modified
                .is_none_or(|modified| modified >= requested)
    }

    fn record(&mut self, part: &Part, displays: &[Display], state: &str) {
        for &i in &part.displays {
            self.displays.insert(
                displays[i].id.clone(),
                (part.media.clone(), state.to_owned()),
            );
        }
    }
}

struct Player {
    part: Part,
    started: Instant,
    still: Option<bool>,
    process: ManagedChild,
    surface: Surface,
    pipe: PathBuf,
    desktop: Rect,
    covered: bool,
    pending_mapping: Option<String>,
    work: tempfile::TempDir,
}
impl Player {
    fn open(part: &Part, settings: &Settings, displays: &[Display]) -> Result<Self> {
        ensure!(
            part.media.is_file(),
            "Cannot find wallpaper {}",
            part.media.display()
        );
        let layout = Layout::new(displays, &settings.calibration);
        let dir = storage::home()?;
        let work = tempfile::Builder::new()
            .prefix("player-")
            .tempdir_in(&dir)?;
        let shader_path = work.path().join("layout.glsl");
        fs::write(&shader_path, shader::generate(displays, part))?;
        let profile = dir.join("playback.conf");
        let desktop = part.desktop(displays);
        let surface = Surface::create(desktop)?;
        let pipe = player_pipe(std::process::id(), settings.multi, part, displays);
        let log = fs::File::create(work.path().join("mpv.log"))?;
        let mut command = storage::command(storage::ensure_mpv()?);
        command
            .current_dir(work.path())
            .args([
                "--no-config",
                "--player-operation-mode=cplayer",
                "--vo=gpu-next",
                "--gpu-api=d3d11",
                "--hwdec=auto",
                "--cscale=bilinear",
                "--no-audio",
                "--no-sub",
                "--loop-file=inf",
                "--image-display-duration=inf",
                "--keepaspect=no",
                "--no-osc",
                "--no-input-default-bindings",
                "--input-vo-keyboard=no",
                "--input-cursor=no",
                "--terminal=yes",
                "--input-terminal=no",
                "--term-status-msg=",
                "--osd-level=0",
                "--cache=no",
                "--interpolation=no",
                "--d3d11-flip=no",
            ])
            .arg(format!("--wid={}", surface.hwnd as usize))
            .arg(format!(
                "--script={}",
                dir.join("first-frame.lua").display()
            ))
            .arg(format!("--include={}", profile.display()))
            .arg(format!("--glsl-shader={}", shader_path.display()))
            .arg(format!(
                "--glsl-shader-opts={}",
                shader::parameters(&layout, settings, displays, part)
            ))
            .arg(format!("--input-ipc-server={}", pipe.display()))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .arg("--msg-level=all=warn")
            .arg("--")
            .arg(std::path::absolute(&part.media)?);
        let process = ManagedChild::spawn(&mut command).context("Cannot launch mpv")?;
        Ok(Self {
            part: part.clone(),
            work,
            started: Instant::now(),
            still: None,
            process,
            surface,
            pipe,
            desktop,
            covered: false,
            pending_mapping: None,
        })
    }
    fn check(&mut self) -> Result<()> {
        if let Some(status) = self.process.child.try_wait()? {
            let log = self.work.path().join("mpv.log");
            let name = format!("player-{}.log", self.part.displays[0]);
            fs::copy(log, storage::home()?.join(&name))?;
            anyhow::bail!("Player exited ({status}). See {name}");
        }
        if self.still.is_none() {
            if let Ok(frame) = fs::read(self.work.path().join("first-frame")) {
                self.still = match frame.as_slice() {
                    b"still" => Some(true),
                    b"video" => Some(false),
                    _ => None,
                };
            }
            ensure!(
                self.still.is_some() || self.started.elapsed() < Duration::from_secs(30),
                "Timed out waiting for a rendered frame"
            );
        }
        Ok(())
    }
    fn update(&mut self, covered: bool) -> Result<()> {
        self.surface.maintain(self.desktop)?;
        if let Some(parameters) = &self.pending_mapping
            && update_mapping(&self.pipe, parameters).is_ok()
        {
            self.pending_mapping = None;
        }
        if covered != self.covered
            && set_player_property(&self.pipe, "user-data/multipapr/covered", covered).is_ok()
        {
            self.covered = covered;
        }
        Ok(())
    }
}

fn version(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}
pub fn run(mut show_editor: bool) -> Result<()> {
    let exe = std::env::current_exe()?;
    let should_relaunch = {
        let open = Handle::checked(unsafe { CreateEventW(null(), 0, 0, wide(OPEN).as_ptr()) })?;
        let mutex = unsafe { CreateMutexW(null(), 0, wide(INSTANCE).as_ptr()) };
        let existing = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let _mutex = Handle::checked(mutex)?;
        if existing {
            if show_editor {
                Tray::allow_focus();
                ensure!(
                    unsafe { SetEvent(open.0) } != 0,
                    "Cannot signal editor request"
                );
            }
            return Ok(());
        }
        update::cleanup_old_executables();
        let stop = Handle::checked(unsafe { CreateEventW(null(), 1, 0, wide(STOP).as_ptr()) })?;
        let relaunch =
            Handle::checked(unsafe { CreateEventW(null(), 1, 0, wide(RELAUNCH).as_ptr()) })?;
        let mut tray = Tray::new()?;
        let mut editor = EditorProcess::default();
        let dir = storage::home()?;
        fs::write(
            dir.join("playback.conf"),
            include_bytes!("../assets/playback.conf"),
        )?;
        fs::write(
            dir.join("first-frame.lua"),
            include_bytes!("../assets/first-frame.lua"),
        )?;
        let config = dir.join("layout.json");
        let mut stamp = None;
        let mut topology = Vec::new();
        let mut players = Players::default();
        let mut current = None;
        let mut last_report = None;
        let preferences_path = dir.join("preferences.json");
        let mut preferences = storage::load_preferences();
        let mut pref_stamp = version(&preferences_path);
        let mut next = Instant::now();
        let mut recheck = Instant::now();
        let mut displays = Vec::new();
        let mut last_error = fs::read_to_string(dir.join("host-error.txt")).unwrap_or_default();
        loop {
            match tray.command() {
                Some(tray::Command::Close) => break false,
                Some(tray::Command::Open) => show_editor = true,
                None => {}
            }
            tray.update();
            if tray.displays_changed() {
                recheck = Instant::now();
                next = recheck;
            }
            if show_editor {
                editor.open(&exe)?;
            }
            let now = Instant::now();
            if now >= next {
                let editor_was_open = editor.running();
                editor.reap()?;
                let result = (|| -> Result<()> {
                    let mut new_stamp = version(&config);
                    if new_stamp.is_none() {
                        players = Players::default();
                        let _ = fs::remove_file(dir.join("playback.json"));
                        last_report = None;
                        stamp = None;
                        return Ok(());
                    }
                    if now >= recheck || displays.is_empty() {
                        displays = displays::detect()?;
                        recheck = now + Duration::from_secs(4);
                    }
                    let follow = !editor.running() && (editor_was_open || topology != displays);
                    let changed = new_stamp != stamp || topology != displays;
                    let mut error = None;
                    if follow || changed {
                        let mut settings = storage::load()?;
                        if !editor.running() {
                            let mut next = settings.clone();
                            let layout = Layout::new(&displays, &next.calibration);
                            if let Ok(mut profiles) = storage::load_profiles() {
                                let before = profiles.clone();
                                follow_displays(&mut profiles, &mut next, &display_set(&displays));
                                if profiles != before
                                    && let Err(e) = storage::save_profiles(&profiles)
                                {
                                    error = Some(e.context("Cannot save profiles"));
                                }
                            } else {
                                next.display_set = display_set(&displays);
                            }
                            if !shader::plan(&next, &displays).is_empty() {
                                layout.freeze(&displays, &mut next);
                            }
                            if next != settings && error.is_none() {
                                if let Err(e) = storage::save("layout.json", &next) {
                                    error = Some(e.context("Cannot save layout"));
                                } else {
                                    new_stamp = version(&config);
                                }
                            }
                            settings = next;
                        }
                        current = Some(settings);
                        stamp = new_stamp;
                        topology.clone_from(&displays);
                    }
                    let observed = version(&preferences_path);
                    if observed != pref_stamp {
                        preferences = storage::load_preferences();
                        pref_stamp = observed;
                    }
                    let policy = PlaybackPolicy::detect(&displays, &preferences);
                    let mut report = players.reconcile(
                        current.as_ref().context("Waiting for layout")?,
                        &displays,
                        &policy,
                        changed || follow,
                    )?;
                    report.layout_modified = stamp;
                    report.loading = players.is_loading();
                    if last_report.as_ref() != Some(&report) {
                        storage::save("playback.json", &report)?;
                        last_report = Some(report);
                    }
                    error.map_or(Ok(()), Err)
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
                        stamp = None;
                        let msg = format!("{error:#}");
                        if last_error != msg {
                            let _ = fs::write(dir.join("host-error.txt"), &msg);
                            last_error = msg;
                        }
                        next = now + Duration::from_secs(4);
                    }
                }
            }
            let was_loading = players.is_loading();
            if let Err(e) = players.advance() {
                players.discard_replacements();
                let msg = format!("{e:#}");
                if msg != last_error {
                    let _ = fs::write(dir.join("host-error.txt"), &msg);
                    last_error = msg;
                }
                next = Instant::now() + Duration::from_secs(1);
            } else if was_loading && !players.is_loading() {
                next = Instant::now();
            }
            let event = desktop::dispatch(
                &[relaunch.0, stop.0, open.0],
                next.saturating_duration_since(Instant::now())
                    .as_millis()
                    .min(players.interval()) as u32,
            );
            if event == WAIT_OBJECT_0 {
                break true;
            }
            if event == WAIT_OBJECT_0 + 1 {
                break false;
            }
            ensure!(
                event != WAIT_FAILED,
                "Cannot wait for tray events: {}",
                io::Error::last_os_error()
            );
            show_editor = event == WAIT_OBJECT_0 + 2;
        }
    };
    if should_relaunch {
        let child = storage::command(&exe)
            .spawn()
            .context("Cannot relaunch Multipapr")?;
        unsafe {
            AllowSetForegroundWindow(child.id());
        }
    }
    Ok(())
}
