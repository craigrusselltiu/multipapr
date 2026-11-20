use crate::{
    desktop::{Handle, wide},
    layout::{Media, Settings},
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
    Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0},
    System::{
        LibraryLoader::{
            FindResourceW, GetModuleHandleW, LoadResource, LockResource, SizeofResource,
        },
        Threading::{CREATE_NO_WINDOW, CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject},
    },
};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

const MPV_SETUP_MUTEX: &str = r"Local\Multipapr.MpvSetup.v1";

pub fn home() -> Result<PathBuf> {
    let path = PathBuf::from(env::var_os("LOCALAPPDATA").context("LOCALAPPDATA is unavailable")?)
        .join("Multipapr");
    fs::create_dir_all(&path)?;
    Ok(path)
}
pub fn command(name: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut cmd = Command::new(name);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}
fn mpv_dir() -> Result<PathBuf> {
    let tools = home()?.join("tools");
    fs::create_dir_all(&tools)?;
    Ok(tools.join(concat!("mpv-", env!("MPV_BUILD"))))
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
// build.rs downloads, verifies and embeds the pinned mpv build as resource 101.
fn mpv_archive() -> Result<&'static [u8]> {
    unsafe {
        let module = GetModuleHandleW(null());
        let res = FindResourceW(module, 101 as _, 10 as _); // 10 = RT_RCDATA
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
    // Windows cannot run an executable from memory, so unpack the bundled build once.
    let dir = mpv_dir()?;
    let staging = dir.with_extension("partial");
    let _ = fs::remove_dir_all(&staging);
    let _ = fs::remove_dir_all(&dir);
    unpack(mpv_archive()?, &staging).context("Cannot unpack the bundled mpv")?;
    fs::rename(&staging, &dir)?;
    mpv()
}
// Unpack mpv and run it once, so the antivirus scan of the new executable happens now
// rather than when the first wallpaper loads.
pub fn warm_up_mpv() {
    if let Ok(mpv) = ensure_mpv() {
        let _ = command(mpv).arg("--version").output();
    }
}
// The bundle is a zstd stream of: file count, then per file its name, size and bytes.
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
    let dir = home()?;
    let mut tmp = tempfile::NamedTempFile::new_in(&dir)?;
    serde_json::to_writer_pretty(&mut tmp, value)?;
    tmp.flush()?;
    tmp.persist(dir.join(name))?;
    Ok(())
}
pub fn load() -> Result<Settings> {
    Ok(serde_json::from_slice(&fs::read(
        home()?.join("layout.json"),
    )?)?)
}
fn parse_mpv_dimensions(stdout: &[u8]) -> Option<(u32, u32)> {
    let text = String::from_utf8_lossy(stdout);
    text.lines().find_map(|line| {
        let (_, rest) = line.split_once("MULTIPAPR:")?;
        let (w, h) = rest.trim().split_once(':')?;
        let (w, h) = (w.parse::<u32>().ok()?, h.parse::<u32>().ok()?);
        (w > 0 && h > 0).then_some((w, h))
    })
}
// One mpv run reads the video size and saves the editor preview frame.
fn probe_with_mpv(path: &Path) -> Result<(u32, u32)> {
    let dir = home()?;
    let script = dir.join("probe.lua");
    fs::write(&script, include_bytes!("../assets/probe.lua"))?;
    let temp = tempfile::tempdir_in(&dir)?;
    let out = command(ensure_mpv()?)
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
        .arg(path)
        .output()
        .context("Cannot probe media with mpv")?;
    let size = parse_mpv_dimensions(&out.stdout).context("Cannot read this media file")?;
    if let Some(frame) = fs::read_dir(temp.path())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("jpg"))
    {
        fs::rename(frame, preview_path(path)?)?;
    }
    Ok(size)
}
fn preview_path(media: &Path) -> Result<PathBuf> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hash = DefaultHasher::new();
    media.hash(&mut hash);
    fs::metadata(media)?.modified()?.hash(&mut hash);
    Ok(home()?.join(format!("preview-{:016x}.jpg", hash.finish())))
}
pub fn probe(path: &Path) -> Result<Media> {
    let path = fs::canonicalize(path).context("Cannot open the selected file")?;
    // Avoid the extended path prefix with multimedia tools that parse paths as URLs.
    let path = std::path::absolute(dunce_path(&path))?;
    if let Ok(reader) = ImageReader::open(&path).and_then(|r| r.with_guessed_format()) {
        if let Ok((w, h)) = reader.into_dimensions() {
            if w > 0 && h > 0 {
                return Ok(Media {
                    path,
                    width: w,
                    height: h,
                });
            }
        }
    }
    let (w, h) = probe_with_mpv(&path)?;
    ensure!(w > 0 && h > 0, "Invalid source dimensions");
    Ok(Media {
        path,
        width: w,
        height: h,
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
    let path = preview_path(&media.path)?;
    if let Ok(image) = image::open(&path) {
        return Ok(image.into_rgba8());
    }
    // Videos get their preview frame from probe(); only still images reach this point.
    let decoded = ImageReader::open(&media.path)?
        .with_guessed_format()?
        .decode()
        .context("Cannot preview this media file")?;
    let scaled = if decoded.width() > 1280 || decoded.height() > 960 {
        decoded.resize(1280, 960, FilterType::Triangle)
    } else {
        decoded
    };
    let _ = DynamicImage::ImageRgb8(scaled.to_rgb8()).save_with_format(&path, ImageFormat::Jpeg);
    Ok(scaled.into_rgba8())
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
    } else if let Err(e) = key.delete_value("Multipapr") {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(e.into());
        }
    }
    Ok(())
}
pub fn apply(settings: &Settings) -> Result<()> {
    ensure_mpv()?;
    ensure!(settings.media.path.is_file(), "Wallpaper file is missing");
    save("layout.json", settings)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let thumb = preview(&media).unwrap();
        assert_eq!((thumb.width(), thumb.height()), (1280, 640));
    }
}
