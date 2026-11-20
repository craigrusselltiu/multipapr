use crate::layout::{Media, Settings};
use anyhow::{Context, Result, ensure};
use std::{
    env, fs,
    io::Write,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
};
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

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
pub fn mpv() -> Result<PathBuf> {
    let mut paths = vec![
        home()?.join("tools/mpv/mpv.exe"),
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
pub fn probe(path: &Path) -> Result<Media> {
    let path = fs::canonicalize(path).context("Cannot open the selected file")?;
    // Avoid the extended path prefix with multimedia tools that parse paths as URLs.
    let path = std::path::absolute(dunce_path(&path))?;
    let out = command("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,sample_aspect_ratio:stream_side_data=rotation",
            "-of",
            "json",
        ])
        .arg(&path)
        .output()
        .context("ffprobe is required to import media. Install FFmpeg and reopen Multipapr.")?;
    ensure!(out.status.success(), "Cannot read this media file");
    let info: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let stream = &info["streams"][0];
    let mut w = stream["width"]
        .as_u64()
        .context("No image or video track")? as u32;
    let mut h = stream["height"].as_u64().context("Missing source height")? as u32;
    if let Some((a, b)) = stream["sample_aspect_ratio"]
        .as_str()
        .and_then(|v| v.split_once(':'))
    {
        if let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) {
            if a > 0 && b > 0 {
                w = (w as f64 * a as f64 / b as f64).round() as u32;
            }
        }
    }
    if stream["side_data_list"].as_array().is_some_and(|list| {
        list.iter()
            .any(|s| s["rotation"].as_i64().unwrap_or(0).rem_euclid(180) == 90)
    }) {
        std::mem::swap(&mut w, &mut h);
    }
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
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hash = DefaultHasher::new();
    media.path.hash(&mut hash);
    fs::metadata(&media.path)?.modified()?.hash(&mut hash);
    let dir = home()?;
    let path = dir.join(format!("preview-{:016x}.jpg", hash.finish()));
    if let Ok(image) = image::open(&path) {
        return Ok(image.into_rgba8());
    }
    let out = command("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(&media.path)
        .args([
            "-map",
            "0:v:0",
            "-frames:v",
            "1",
            "-vf",
            "scale=1280:960:force_original_aspect_ratio=decrease",
            "-c:v",
            "mjpeg",
            "-q:v",
            "3",
            "-f",
            "image2pipe",
            "pipe:1",
        ])
        .output()
        .context("FFmpeg is required for the preview")?;
    ensure!(
        out.status.success(),
        "Preview failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let image = image::load_from_memory(&out.stdout)?.into_rgba8();
    fs::write(path, out.stdout)?;
    Ok(image)
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
    mpv()?;
    ensure!(settings.media.path.is_file(), "Wallpaper file is missing");
    save("layout.json", settings)
}
