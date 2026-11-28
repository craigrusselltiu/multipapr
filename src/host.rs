mod playback;
use crate::{
    desktop::{self, Handle, Surface, SurfaceKind, wide},
    displays,
    layout::{Display, Layout, Rect, Settings, display_set, follow_displays},
    process, render,
    shader::{self, Part},
    storage::{self, PlaybackAction, Preferences},
    tray::{self, Tray},
    update, windows_bg,
};
use anyhow::{Context, Result, ensure};
use playback::{PlaybackPolicy, Players};
use std::{
    collections::BTreeMap,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::{self, Write},
    os::windows::{ffi::OsStringExt, fs::OpenOptionsExt, io::AsRawHandle},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    ptr::{null, null_mut},
    sync::{
        Arc,
        mpsc::{Receiver, TryRecvError, channel},
    },
    time::{Duration, Instant, SystemTime},
};
use windows_sys::Wdk::System::Threading::{NtQueryInformationProcess, ProcessBasicInformation};
use windows_sys::Win32::{
    Foundation::*,
    Storage::FileSystem::SECURITY_IDENTIFICATION,
    System::{JobObjects::*, Pipes::GetNamedPipeServerProcessId, Threading::*},
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
        process::spawn(storage::command(std::env::current_exe()?).arg("--background"))
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
        let mut child = process::spawn(command)?;
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
    part.paused.hash(&mut hash);
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

enum PlayerPeer {
    Child(u32),
    OfHost(u32),
}

impl PlayerPeer {
    fn verify(&self, server: u32) -> Result<()> {
        let host = match *self {
            Self::Child(pid) => {
                ensure!(pid == server, "Unexpected player process");
                return Ok(());
            }
            Self::OfHost(host) => host,
        };
        // The editor knows its host PID. Verify the server's parent and executable with Windows.
        let process =
            Handle::checked(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, server) })?;
        let mut info: PROCESS_BASIC_INFORMATION = unsafe { std::mem::zeroed() };
        let status = unsafe {
            NtQueryInformationProcess(
                process.0,
                ProcessBasicInformation,
                std::ptr::from_mut(&mut info).cast(),
                size_of_val(&info) as u32,
                null_mut(),
            )
        };
        ensure!(
            status >= 0 && info.InheritedFromUniqueProcessId == host as usize,
            "Unexpected player parent"
        );
        let mut path = vec![0u16; 32768];
        let mut len = path.len() as u32;
        ensure!(
            unsafe { QueryFullProcessImageNameW(process.0, 0, path.as_mut_ptr(), &mut len) } != 0,
            "Cannot identify the player executable"
        );
        let server_path = PathBuf::from(std::ffi::OsString::from_wide(&path[..len as usize]));
        ensure!(
            fs::canonicalize(server_path)? == fs::canonicalize(storage::mpv()?)?,
            "Unexpected player executable"
        );
        Ok(())
    }
}

pub fn update_mapping(pipe: &Path, host: u32, parameters: &str) -> Result<()> {
    set_player_property(
        pipe,
        PlayerPeer::OfHost(host),
        "glsl-shader-opts",
        parameters,
    )
}
fn set_player_property(
    pipe: &Path,
    peer: PlayerPeer,
    name: &str,
    value: impl serde::Serialize,
) -> Result<()> {
    let mut ipc = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .security_qos_flags(SECURITY_IDENTIFICATION)
        .open(pipe)?;
    let mut server = 0;
    ensure!(
        unsafe { GetNamedPipeServerProcessId(ipc.as_raw_handle(), &mut server) } != 0,
        "Cannot identify the player pipe"
    );
    peer.verify(server)?;
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
    pub fn layout_finished(&self, requested: SystemTime) -> bool {
        !self.loading
            && self
                .layout_modified
                .is_some_and(|modified| modified >= requested)
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

pub fn wait_for_layout(modified: SystemTime) {
    let Ok(dir) = storage::home() else {
        return;
    };
    let report = dir.join("playback.json");
    let deadline = Instant::now() + Duration::from_secs(3);
    while running() && Instant::now() < deadline {
        if fs::read(&report)
            .ok()
            .and_then(|data| serde_json::from_slice::<PlaybackReport>(&data).ok())
            .is_some_and(|report| report.layout_finished(modified))
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

struct Player {
    part: Part,
    started: Instant,
    still: Option<bool>,
    backend: PlayerBackend,
    surface: Surface,
    pipe: PathBuf,
    desktop: Rect,
    covered: bool,
    pending_mapping: Option<String>,
}

enum PlayerBackend {
    Black,
    Mpv(MediaPlayback),
    Static(Box<StaticPlayback>),
}

struct MediaPlayback {
    process: ManagedChild,
    work: tempfile::TempDir,
}

type FrameLoadResult = Result<(Arc<render::SourceImage>, u32, u32, Vec<u8>)>;
type RenderResult = Result<(u32, u32, Vec<u8>)>;

struct StaticPlayback {
    frame: Option<Arc<render::SourceImage>>,
    pending_frame: Option<Receiver<FrameLoadResult>>,
    pending_render: Option<Receiver<RenderResult>>,
    stale_render: Option<(Settings, Vec<Display>, Part)>,
}

impl StaticPlayback {
    fn start_render(&mut self, settings: &Settings, displays: &[Display], part: &Part) {
        let Some(frame) = self.frame.as_ref().map(Arc::clone) else {
            self.stale_render = Some((settings.clone(), displays.to_vec(), part.clone()));
            return;
        };
        if self.pending_render.is_some() {
            self.stale_render = Some((settings.clone(), displays.to_vec(), part.clone()));
            return;
        }
        let settings = settings.clone();
        let displays = displays.to_vec();
        let part = part.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let layout = Layout::new(&displays, &settings.calibration);
            let _ = tx.send(render::render_part_bgra(
                &frame, &layout, &settings, &displays, &part,
            ));
        });
        self.pending_render = Some(rx);
    }
}

impl Player {
    fn is_black(&self) -> bool {
        matches!(self.backend, PlayerBackend::Black)
    }

    fn has_pending_render(&self) -> bool {
        matches!(
            &self.backend,
            PlayerBackend::Static(st) if st.pending_render.is_some()
        )
    }

    fn open(part: &Part, settings: &Settings, displays: &[Display]) -> Result<Self> {
        let black = part.media.as_os_str().is_empty();
        ensure!(
            black || part.media.is_file(),
            "{}",
            part.missing_wallpaper(settings.multi)
        );
        let desktop = part.desktop(displays);
        let kind = if black {
            SurfaceKind::Black
        } else if part.paused {
            SurfaceKind::Still
        } else {
            SurfaceKind::Video
        };
        let mut player = Self {
            part: part.clone(),
            started: Instant::now(),
            still: black.then_some(true),
            backend: PlayerBackend::Black,
            surface: Surface::create(desktop, kind)?,
            pipe: player_pipe(std::process::id(), settings.multi, part, displays),
            desktop,
            covered: false,
            pending_mapping: None,
        };
        if black {
            return Ok(player);
        }
        let media = std::path::absolute(&part.media)?;
        if part.paused {
            let settings = settings.clone();
            let displays = displays.to_vec();
            let part = part.clone();
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let result = render::decode_source(&media).and_then(|source| {
                    let frame = Arc::new(source);
                    let layout = Layout::new(&displays, &settings.calibration);
                    let (w, h, bgra) =
                        render::render_part_bgra(&frame, &layout, &settings, &displays, &part)?;
                    Ok((frame, w, h, bgra))
                });
                let _ = tx.send(result);
            });
            player.backend = PlayerBackend::Static(Box::new(StaticPlayback {
                frame: None,
                pending_frame: Some(rx),
                pending_render: None,
                stale_render: None,
            }));
            return Ok(player);
        }
        let layout = Layout::new(displays, &settings.calibration);
        let dir = storage::home()?;
        let work = tempfile::Builder::new()
            .prefix("player-")
            .tempdir_in(&dir)?;
        let shader_path = work.path().join("layout.glsl");
        fs::write(&shader_path, shader::generate(displays, part))?;
        let profile = dir.join("playback.conf");
        let log = fs::File::create(work.path().join("mpv.log"))?;
        let mut command = storage::wallpaper_command(&media)?;
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
            .arg(format!("--wid={}", player.surface.hwnd as usize))
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
            .arg(format!("--input-ipc-server={}", player.pipe.display()))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .arg("--msg-level=all=warn")
            .arg("--")
            .arg(&media);
        let process = ManagedChild::spawn(&mut command).context("Cannot launch mpv")?;
        player.backend = PlayerBackend::Mpv(MediaPlayback { process, work });
        Ok(player)
    }

    fn update_mapping_for(
        &mut self,
        layout: &Layout,
        settings: &Settings,
        displays: &[Display],
        part: &Part,
    ) {
        match &mut self.backend {
            PlayerBackend::Black => {}
            PlayerBackend::Mpv(_) => {
                self.pending_mapping = Some(shader::parameters(layout, settings, displays, part));
            }
            PlayerBackend::Static(st) => {
                st.start_render(settings, displays, part);
            }
        }
    }

    fn check(&mut self) -> Result<()> {
        match &mut self.backend {
            PlayerBackend::Black => Ok(()),
            PlayerBackend::Mpv(playback) => {
                if let Some(status) = playback.process.child.try_wait()? {
                    let log = playback.work.path().join("mpv.log");
                    let name = format!("player-{}.log", self.part.displays[0]);
                    fs::copy(log, storage::home()?.join(&name))?;
                    anyhow::bail!("Player exited ({status}). See {name}");
                }
                if self.still.is_none() {
                    if let Ok(frame) = fs::read(playback.work.path().join("first-frame")) {
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
            PlayerBackend::Static(st) => {
                if let Some(rx) = &st.pending_frame {
                    match rx.try_recv() {
                        Ok(Ok((frame, w, h, bgra))) => {
                            st.pending_frame = None;
                            self.surface.set_bitmap(w, h, &bgra)?;
                            st.frame = Some(frame);
                            self.still = Some(true);
                            if let Some((settings, displays, part)) = st.stale_render.take() {
                                st.start_render(&settings, &displays, &part);
                            }
                        }
                        Ok(Err(error)) => {
                            st.pending_frame = None;
                            return Err(error);
                        }
                        Err(TryRecvError::Disconnected) => {
                            st.pending_frame = None;
                            anyhow::bail!("Static wallpaper decoder stopped");
                        }
                        Err(TryRecvError::Empty) => {
                            ensure!(
                                self.started.elapsed() < Duration::from_secs(30),
                                "Timed out waiting for a rendered frame"
                            );
                        }
                    }
                }
                if let Some(rx) = &st.pending_render {
                    match rx.try_recv() {
                        Ok(Ok((w, h, bgra))) => {
                            st.pending_render = None;
                            self.surface.set_bitmap(w, h, &bgra)?;
                            if let Some((settings, displays, part)) = st.stale_render.take() {
                                st.start_render(&settings, &displays, &part);
                            }
                        }
                        Ok(Err(_)) | Err(TryRecvError::Disconnected) => {
                            st.pending_render = None;
                        }
                        Err(TryRecvError::Empty) => {}
                    }
                }
                Ok(())
            }
        }
    }
    fn update(&mut self, covered: bool) -> Result<()> {
        self.surface.maintain(self.desktop)?;
        let PlayerBackend::Mpv(playback) = &self.backend else {
            return Ok(());
        };
        let pid = playback.process.child.id();
        if let Some(parameters) = &self.pending_mapping
            && set_player_property(
                &self.pipe,
                PlayerPeer::Child(pid),
                "glsl-shader-opts",
                parameters,
            )
            .is_ok()
        {
            self.pending_mapping = None;
        }
        if covered != self.covered
            && set_player_property(
                &self.pipe,
                PlayerPeer::Child(pid),
                "user-data/multipapr/covered",
                covered,
            )
            .is_ok()
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
        let mut windows_bg = windows_bg::Syncer::default();
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
                        windows_bg.disable();
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
                        let saved = storage::load()?;
                        let mut settings = saved.clone();
                        let layout = Layout::new(&displays, &settings.calibration);
                        settings.restore_multi_wallpapers(&displays, &layout, Path::is_file);
                        if !editor.running() {
                            if let Ok(mut profiles) = storage::load_profiles() {
                                let before = profiles.clone();
                                follow_displays(
                                    &mut profiles,
                                    &mut settings,
                                    &display_set(&displays),
                                );
                                if profiles != before
                                    && let Err(e) = storage::save_profiles(&profiles)
                                {
                                    error = Some(e.context("Cannot save profiles"));
                                }
                            } else {
                                settings.display_set = display_set(&displays);
                            }
                            if !shader::plan(&settings, &displays).is_empty() {
                                layout.freeze(&displays, &mut settings);
                            }
                            if settings != saved && error.is_none() {
                                if let Err(e) = storage::save("layout.json", &settings) {
                                    error = Some(e.context("Cannot save layout"));
                                } else {
                                    new_stamp = version(&config);
                                }
                            }
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
                    let settings = current.as_ref().context("Waiting for layout")?;
                    let policy = PlaybackPolicy::detect(&displays, &preferences);
                    if preferences.windows_background {
                        if !policy.is_locked() {
                            windows_bg.sync(settings, &displays);
                        }
                    } else {
                        windows_bg.disable();
                    }
                    let mut report = players.reconcile(
                        settings,
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
            if windows_bg.take_completed() {
                players.maintain_surfaces();
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
        let child =
            process::spawn(&mut storage::command(&exe)).context("Cannot relaunch Multipapr")?;
        unsafe {
            AllowSetForegroundWindow(child.id());
        }
    }
    Ok(())
}
