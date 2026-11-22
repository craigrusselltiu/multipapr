use crate::{
    desktop::{Handle, make_int_resource, wide},
    displays,
    layout::{Display, Layout, Media, Profile, Settings},
    process,
};
use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageFormat, ImageReader, imageops::FilterType};
use std::{
    env, fs,
    io::{self, Read, Write},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    ptr::null,
};
use windows_sys::Win32::{
    Foundation::{ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION, WAIT_ABANDONED, WAIT_OBJECT_0},
    System::{
        LibraryLoader::{
            FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource,
        },
        Threading::{CREATE_NO_WINDOW, CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject},
    },
};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

const MPV_SETUP_MUTEX: &str = r"Local\Multipapr.MpvSetup.v1";
const MPV_RESOURCE_ID: u16 = 101;
const RT_RCDATA: u16 = 10;

fn app_dir() -> Result<PathBuf> {
    Ok(
        PathBuf::from(env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is unavailable")?)
            .join("Multipapr"),
    )
}
pub fn home() -> Result<PathBuf> {
    let path = app_dir()?;
    fs::create_dir_all(&path)?;
    Ok(path)
}
pub fn command(name: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(name);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}
pub fn wallpaper_dir() -> Result<PathBuf> {
    Ok(app_dir()?.join("Wallpapers"))
}

pub fn in_wallpaper_dir(path: &Path, dir: &Path) -> bool {
    path.parent().is_some_and(|parent| {
        parent == dir || fs::canonicalize(dir).is_ok_and(|dir| dunce_path(&dir) == parent)
    })
}

pub fn wallpaper_command(path: &Path) -> Result<Command> {
    let managed = wallpaper_dir()?;
    if in_wallpaper_dir(path, &managed) {
        let demuxer = path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("mp4"))
            .then_some("mov");
        return restricted_media_command(demuxer);
    }
    Ok(command(ensure_mpv()?))
}

pub fn restricted_media_command(demuxer: Option<&str>) -> Result<Command> {
    Ok(media_command(ensure_mpv()?, demuxer))
}

pub fn preview_media_command(demuxer: Option<&str>) -> Result<Command> {
    let mut cmd = media_command(mpv()?, demuxer);
    cmd.args(["--cache-on-disk=no", "--save-position-on-quit=no"]);
    Ok(cmd)
}

fn media_command(program: PathBuf, demuxer: Option<&str>) -> Command {
    let mut cmd = command(program);
    cmd.args(["--access-references=no", "--autoload-files=no", "--no-ytdl"]);
    if let Some(demuxer) = demuxer {
        cmd.args([
            "--demuxer=lavf",
            &format!("--demuxer-lavf-format={demuxer}"),
        ]);
    }
    cmd
}

fn mpv_dir() -> Result<PathBuf> {
    Ok(app_dir()?
        .join("tools")
        .join(concat!("mpv-", env!("MPV_BUILD"))))
}
pub fn mpv() -> Result<PathBuf> {
    let mut paths = vec![
        mpv_dir()?.join("mpv.exe"),
        env::current_exe()?.with_file_name("mpv.exe"),
    ];
    paths.extend(
        env::split_paths(&env::var_os("PATH").unwrap_or_default()).map(|p| p.join("mpv.exe")),
    );
    paths
        .into_iter()
        .find(|p| p.is_file())
        .context("Install mpv or place mpv.exe beside multipapr.exe")
}
fn mpv_archive() -> Result<&'static [u8]> {
    unsafe {
        let module = GetModuleHandleW(null());
        let res = FindResourceW(
            module,
            make_int_resource(MPV_RESOURCE_ID),
            make_int_resource(RT_RCDATA),
        );
        ensure!(!res.is_null(), "The bundled mpv is missing");
        let data = LockResource(LoadResource(module, res)) as *const u8;
        ensure!(!data.is_null(), "Cannot load the bundled mpv");
        Ok(std::slice::from_raw_parts(
            data,
            SizeofResource(module, res) as usize,
        ))
    }
}
pub fn ensure_mpv() -> Result<PathBuf> {
    if let Ok(path) = mpv() {
        return Ok(path);
    }
    let mutex =
        Handle::checked(unsafe { CreateMutexW(null(), 0, wide(MPV_SETUP_MUTEX).as_ptr()) })?;
    let wait = unsafe { WaitForSingleObject(mutex.0, INFINITE) };
    ensure!(
        wait == WAIT_OBJECT_0 || wait == WAIT_ABANDONED,
        "Cannot coordinate mpv setup"
    );
    struct MutexGuard(windows_sys::Win32::Foundation::HANDLE);
    impl Drop for MutexGuard {
        fn drop(&mut self) {
            unsafe {
                ReleaseMutex(self.0);
            }
        }
    }
    let _guard = MutexGuard(mutex.0);
    if let Ok(path) = mpv() {
        return Ok(path);
    }
    let dir = mpv_dir()?;
    let staging = dir.with_extension("partial");
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_dir_all(&dir);
    unpack(mpv_archive()?, &staging).context("Cannot unpack the bundled mpv")?;
    fs::rename(&staging, &dir)?;
    mpv()
}
pub fn warm_up_mpv() {
    if let Ok(mpv) = ensure_mpv() {
        let _ = process::decoder_output(command(mpv).arg("--version"));
    }
}
fn unpack(bundle: &[u8], dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    let mut z = zstd::Decoder::with_buffer(bundle)?;
    z.window_log_max(31)?;
    let mut u32_buf = [0u8; 4];
    z.read_exact(&mut u32_buf)?;
    for _ in 0..u32::from_le_bytes(u32_buf) {
        z.read_exact(&mut u32_buf)?;
        let mut name = vec![0; u32::from_le_bytes(u32_buf) as usize];
        z.read_exact(&mut name)?;
        let mut size = [0u8; 8];
        z.read_exact(&mut size)?;
        let mut file = fs::File::create(dest.join(String::from_utf8(name)?))?;
        io::copy(&mut (&mut z).take(u64::from_le_bytes(size)), &mut file)?;
    }
    Ok(())
}
pub fn save<T: serde::Serialize>(name: &str, value: &T) -> Result<()> {
    save_in(&home()?, name, value)
}
fn save_in<T: serde::Serialize>(dir: &Path, name: &str, value: &T) -> Result<()> {
    let destination = dir.join(name);
    let mut tmp = tempfile::Builder::new().make_in(dir, |path| fs::File::create_new(path))?;
    serde_json::to_writer_pretty(&mut tmp, value)?;
    tmp.flush()?;
    let staged = tmp.into_temp_path();
    for attempt in 1.. {
        match fs::rename(&staged, &destination) {
            Ok(()) => break,
            Err(e)
                if attempt < 10
                    && (e.kind() == io::ErrorKind::PermissionDenied
                        || matches!(
                            e.raw_os_error().map(|code| code as u32),
                            Some(ERROR_SHARING_VIOLATION | ERROR_LOCK_VIOLATION)
                        )) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(30));
            }
            Err(e) => {
                return Err(e).with_context(|| format!("Cannot replace {}", destination.display()));
            }
        }
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub close_to_tray: bool,
    pub maintain_ratio: bool,
    pub fullscreen: PlaybackAction,
    pub battery_saver: PlaybackAction,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackAction {
    Continue,
    Pause,
    Stop,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            maintain_ratio: true,
            fullscreen: PlaybackAction::Continue,
            battery_saver: PlaybackAction::Continue,
        }
    }
}
fn load_preferences_from(path: &Path) -> Preferences {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
pub fn load_preferences() -> Preferences {
    home()
        .map(|dir| load_preferences_from(&dir.join("preferences.json")))
        .unwrap_or_default()
}
pub fn save_preferences(preferences: &Preferences) -> Result<()> {
    save("preferences.json", preferences)
}
pub fn load() -> Result<Settings> {
    load_optional()?.context("Layout file is missing")
}
pub fn load_optional() -> Result<Option<Settings>> {
    let path = home()?.join("layout.json");
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).context("Cannot read layout.json"),
    };
    Ok(Some(parse_settings(&bytes, displays::detect)?))
}
fn parse_settings(
    bytes: &[u8],
    displays: impl FnOnce() -> Result<Vec<Display>>,
) -> Result<Settings> {
    let mut value: serde_json::Value = serde_json::from_slice(bytes)?;
    let legacy = value
        .as_object_mut()
        .filter(|o| !o.contains_key("center"))
        .and_then(|o| o.remove("pan"));
    let mut settings: Settings = serde_json::from_value(value)?;
    if let Some(pan) = legacy {
        let pan: [f32; 2] = serde_json::from_value(pan)?;
        let displays = displays()?;
        let layout = Layout::legacy(&displays, &settings.calibration);
        let [x, y] = layout.bounds.center();
        settings.wallpaper.center = [x + pan[0], y + pan[1]];
        layout.freeze(&displays, &mut settings);
    }
    Ok(settings)
}
pub fn load_profiles() -> Result<Vec<Profile>> {
    match fs::read(home()?.join("profiles.json")) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.into()),
    }
}
pub fn save_profiles(profiles: &[Profile]) -> Result<()> {
    save("profiles.json", &profiles)
}
fn parse_mpv_dimensions(stdout: &[u8]) -> Option<(u32, u32)> {
    let text = String::from_utf8_lossy(stdout);
    text.lines().find_map(|line| {
        let (_, rest) = line.split_once("MULTIPAPR:")?;
        let mut values = rest.trim().split(':');
        let w = values.next()?.parse::<u32>().ok()?;
        let h = values.next()?.parse::<u32>().ok()?;
        let rotation = values
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        let size = if rotation % 180 == 90 { (h, w) } else { (w, h) };
        (w > 0 && h > 0).then_some(size)
    })
}
fn probe_with_mpv(path: &Path) -> Result<(u32, u32)> {
    let dir = home()?;
    let temp = tempfile::tempdir_in(&dir)?;
    let script = temp.path().join("probe.lua");
    fs::write(&script, include_bytes!("../assets/probe.lua"))?;
    let mut command = wallpaper_command(path)?;
    command
        .args([
            "--no-config",
            "--terminal=yes",
            "--msg-level=all=no,probe=info",
            "--vo=image",
            "--vo-image-format=jpg",
            "--vo-image-jpeg-quality=85",
            "--vf=lavfi=[scale=1280:960:force_original_aspect_ratio=decrease]",
            "--frames=1",
            "--ao=null",
            "--no-audio",
            "--no-sub",
            "--image-display-duration=0",
        ])
        .arg(format!("--script={}", script.display()))
        .arg(format!("--vo-image-outdir={}", temp.path().display()))
        .arg("--")
        .arg(path);
    let out = process::decoder_output(&command).context("Cannot probe media with mpv")?;
    let size = parse_mpv_dimensions(&out).context("Cannot read this media file")?;
    if let Some(frame) = fs::read_dir(temp.path())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("jpg"))
    {
        fs::rename(frame, preview_path(path)?)?;
    }
    Ok(size)
}
pub fn preview_path(media: &Path) -> Result<PathBuf> {
    let home = app_dir()?;
    let library = home.join("Wallpapers");
    let dir = if in_wallpaper_dir(media, &library) {
        &library
    } else {
        &home
    };
    preview_path_in(media, dir)
}

fn preview_path_in(media: &Path, dir: &Path) -> Result<PathBuf> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hash = DefaultHasher::new();
    media.hash(&mut hash);
    fs::metadata(media)?.modified()?.hash(&mut hash);
    Ok(dir.join(format!("preview-{:016x}.jpg", hash.finish())))
}
pub fn still_image(path: &Path) -> bool {
    ImageReader::open(path)
        .and_then(ImageReader::with_guessed_format)
        .is_ok_and(|reader| {
            reader
                .format()
                .is_some_and(|format| format != ImageFormat::Gif)
        })
}
pub fn canonical_path(path: &Path) -> Result<PathBuf> {
    let path = fs::canonicalize(path).context("Cannot open the selected file")?;
    Ok(std::path::absolute(dunce_path(&path))?)
}

fn probe_media(
    path: &Path,
    probe_video: impl FnOnce(&Path) -> Result<(u32, u32)>,
) -> Result<Media> {
    let path = canonical_path(path)?;
    if let Ok(reader) = ImageReader::open(&path).and_then(|r| r.with_guessed_format())
        && let Ok((w, h)) = reader.into_dimensions()
        && w > 0
        && h > 0
    {
        return Ok(Media {
            path,
            width: w,
            height: h,
        });
    }
    let (w, h) = probe_video(&path)?;
    ensure!(w > 0 && h > 0, "Invalid source dimensions");
    Ok(Media {
        path,
        width: w,
        height: h,
    })
}

pub fn probe(path: &Path) -> Result<Media> {
    probe_media(path, probe_with_mpv)
}

pub fn probe_read_only(path: &Path) -> Result<Media> {
    probe_media(path, |path| {
        let mut command = preview_media_command(None)?;
        command
            .args([
                "--no-config",
                "--terminal=yes",
                "--input-terminal=no",
                "--vo=null",
                "--no-audio",
                "--no-sub",
                "--vd-lavc-o=max_pixels=33177600",
                "--frames=1",
                "--term-status-msg=MULTIPAPR:${video-params/dw}:${video-params/dh}:${video-params/rotate}",
                "--",
            ])
            .arg(path);
        let output = process::decoder_output(&command).context("Cannot inspect media with mpv")?;
        parse_mpv_dimensions(&output).context("Cannot read this media file")
    })
}
fn dunce_path(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    s.strip_prefix(r"\\?\")
        .map(PathBuf::from)
        .unwrap_or_else(|| path.into())
}
pub fn preview(media: &Media) -> Result<image::RgbaImage> {
    preview_at(media, &preview_path(&media.path)?, true)
}
pub fn preview_read_only(media: &Media) -> Result<image::RgbaImage> {
    preview_at(media, &preview_path(&media.path)?, false)
}
#[cfg(test)]
fn preview_in(media: &Media, dir: &Path) -> Result<image::RgbaImage> {
    preview_at(media, &preview_path_in(&media.path, dir)?, true)
}
fn preview_at(media: &Media, path: &Path, cache: bool) -> Result<image::RgbaImage> {
    if let Ok(image) = image::open(path) {
        return Ok(image.into_rgba8());
    }
    let decoded = ImageReader::open(&media.path)?
        .with_guessed_format()?
        .decode()
        .context("Cannot preview this media file")?;
    let scaled = if decoded.width() > 1280 || decoded.height() > 960 {
        decoded.resize(1280, 960, FilterType::Triangle)
    } else {
        decoded
    };
    if cache {
        let _ = DynamicImage::ImageRgb8(scaled.to_rgb8()).save_with_format(path, ImageFormat::Jpeg);
    }
    Ok(scaled.into_rgba8())
}

pub fn cache_preview(media: &Media, image: &image::RgbaImage) -> Result<()> {
    let path = preview_path(&media.path)?;
    let temp = tempfile::NamedTempFile::new_in(path.parent().context("Invalid preview path")?)?;
    DynamicImage::ImageRgba8(image.clone())
        .into_rgb8()
        .save_with_format(temp.path(), ImageFormat::Jpeg)?;
    match temp.persist_noclobber(path) {
        Ok(_) => Ok(()),
        Err(err) if err.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(err) => Err(err.error.into()),
    }
}
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
pub fn startup_enabled() -> bool {
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(RUN)
        .and_then(|k| k.get_value::<String, _>("Multipapr"))
        .is_ok()
}
pub fn startup(enabled: bool) -> Result<()> {
    let (key, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey(RUN)?;
    if enabled {
        key.set_value(
            "Multipapr",
            &format!("\"{}\" --background", env::current_exe()?.display()),
        )?;
    } else if let Err(e) = key.delete_value("Multipapr")
        && e.kind() != io::ErrorKind::NotFound
    {
        return Err(e.into());
    }
    Ok(())
}
pub fn apply(settings: &Settings) -> Result<()> {
    ensure_mpv()?;
    ensure!(
        settings.wallpaper.media.path.is_file(),
        "Wallpaper file is missing"
    );
    save("layout.json", settings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rapid_saves_replace_json_while_the_host_reads_the_previous_version() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["layout.json", "profiles.json"] {
            save_in(dir.path(), name, &0).unwrap();
            let path = dir.path().join(name);
            let mut reader = fs::File::open(&path).unwrap();
            for value in 1..=12 {
                save_in(dir.path(), name, &value).unwrap();
                assert_eq!(fs::read_to_string(&path).unwrap(), value.to_string());
            }
            let mut previous = String::new();
            reader.read_to_string(&mut previous).unwrap();
            assert_eq!(previous, "0");
        }
    }

    #[test]
    fn parses_mpv_probe_output() {
        let out = b"[probe] MULTIPAPR:3840:2160\r\n";
        assert_eq!(parse_mpv_dimensions(out), Some((3840, 2160)));
        assert_eq!(parse_mpv_dimensions(b"nothing here"), None);
    }

    #[test]
    fn probes_and_previews_png_without_external_tools() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sample.png");
        let img = image::RgbaImage::from_pixel(2000, 1000, image::Rgba([20, 40, 80, 255]));
        img.save(&file).unwrap();
        let media = probe(&file).unwrap();
        assert_eq!((media.width, media.height), (2000, 1000));
        let thumb = preview_in(&media, dir.path()).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (1280, 640));
    }

    #[test]
    fn migrates_the_wallpaper_offset_from_older_versions() {
        let display = || {
            Ok(vec![Display {
                id: "a".into(),
                name: "a".into(),
                primary: true,
                px: crate::layout::Rect::new(0., 0., 1920., 1080.),
                mm: [480., 270.],
                estimated: false,
                mirrors: Vec::new(),
                mirror_ids: Vec::new(),
            }])
        };
        let old = br#"{"media":{"path":"a.mp4","width":1920,"height":1080},"scale":1.0,"pan":[10.0,-5.0],"calibration":{}}"#;
        let settings = parse_settings(old, display).unwrap();
        assert_eq!(settings.wallpaper.center, [250., 130.]);
        assert_eq!(settings.pitch, Some(0.25));
        assert_eq!(settings.calibration["a"].position, Some([0., 0.]));
        let saved = serde_json::to_vec(&settings).unwrap();
        assert!(parse_settings(&saved, || unreachable!()).unwrap() == settings);
    }

    #[test]
    fn preferences_default_to_close_to_tray_and_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.json");
        assert_eq!(load_preferences_from(&path), Preferences::default());
        assert!(load_preferences_from(&path).close_to_tray);

        fs::write(&path, r#"{"close_to_tray":false}"#).unwrap();
        let mut previous = load_preferences_from(&path);
        assert!(!previous.close_to_tray);
        assert_eq!(previous.fullscreen, PlaybackAction::Continue);
        assert_eq!(previous.battery_saver, PlaybackAction::Continue);
        previous.fullscreen = PlaybackAction::Stop;
        previous.battery_saver = PlaybackAction::Pause;
        save_in(dir.path(), "preferences.json", &previous).unwrap();
        assert_eq!(load_preferences_from(&path), previous);

        fs::write(&path, r#"{}"#).unwrap();
        assert!(load_preferences_from(&path).close_to_tray);
    }
}
