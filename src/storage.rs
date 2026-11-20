use crate::{
    desktop::{Handle, wide},
    layout::{Media, Settings},
};
use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageFormat, ImageReader, imageops::FilterType};
use std::{
    env, fs,
    io::Write,
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::Command,
    ptr::null,
};
use windows_sys::Win32::{
    Foundation::{WAIT_ABANDONED, WAIT_OBJECT_0},
    System::Threading::{
        CREATE_NO_WINDOW, CreateMutexW, INFINITE, ReleaseMutex, WaitForSingleObject,
    },
};
use winreg::{RegKey, enums::HKEY_CURRENT_USER};

const MPV_URL: &str = "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261111/mpv-x86_64-20261111-git-bcde6b93cc.7z";
const MPV_SHA256: &str = "f259b5fd4b5c395617c948a4fadcdf55bba1c5282fb7c13e88626b3f71072baf";
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
pub fn ensure_mpv() -> Result<PathBuf> {
    if let Ok(path) = mpv() {
        return Ok(path);
    }
    if env::var("PROCESSOR_ARCHITECTURE").as_deref() != Ok("AMD64") {
        return mpv();
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
    let mpv_dir = home()?.join("tools").join("mpv");
    let script = format!(
        r#"$ErrorActionPreference = 'Stop'
$mpvDir = '{dir}'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("multipapr-mpv-" + [guid]::NewGuid() + ".7z")
try {{
    Invoke-WebRequest -UseBasicParsing '{url}' -OutFile $tmp
    $hash = [BitConverter]::ToString([Security.Cryptography.SHA256]::Create().ComputeHash([IO.File]::ReadAllBytes($tmp))).Replace('-', '')
    if ($hash -ne '{sha}') {{
        throw 'mpv archive checksum mismatch'
    }}
    New-Item -ItemType Directory -Force -Path $mpvDir | Out-Null
    tar.exe -xf $tmp -C $mpvDir 2>$null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath (Join-Path $mpvDir 'mpv.exe'))) {{
        $seven = Get-Command 7z.exe -ErrorAction SilentlyContinue
        if (-not $seven) {{ throw 'Could not extract mpv archive with tar.exe' }}
        & $seven.Source x -y "-o$mpvDir" $tmp | Out-Null
        if ($LASTEXITCODE -ne 0) {{ throw 'mpv extraction failed' }}
    }}
}} finally {{
    if (Test-Path -LiteralPath $tmp) {{ Remove-Item -LiteralPath $tmp -Force }}
}}"#,
        dir = mpv_dir.display().to_string().replace('\'', "''"),
        url = MPV_URL,
        sha = MPV_SHA256,
    );
    let out = command("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .context("Cannot launch PowerShell to download mpv")?;
    ensure!(
        out.status.success(),
        "Could not download mpv: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    mpv()
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
fn probe_with_mpv(path: &Path) -> Result<(u32, u32)> {
    let dir = home()?;
    let script = dir.join("probe.lua");
    fs::write(&script, include_bytes!("../assets/probe.lua"))?;
    let out = command(ensure_mpv()?)
        .args([
            "--no-config",
            "--terminal=yes",
            "--msg-level=all=no,probe=info",
            "--vo=null",
            "--ao=null",
            "--no-audio",
            "--no-sub",
            "--image-display-duration=0",
        ])
        .arg(format!("--script={}", script.display()))
        .arg("--")
        .arg(path)
        .output()
        .context("Cannot probe media with mpv")?;
    parse_mpv_dimensions(&out.stdout).context("Cannot read this media file")
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
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hash = DefaultHasher::new();
    media.path.hash(&mut hash);
    fs::metadata(&media.path)?.modified()?.hash(&mut hash);
    let dir = home()?;
    let path = dir.join(format!("preview-{:016x}.jpg", hash.finish()));
    if let Ok(image) = image::open(&path) {
        return Ok(image.into_rgba8());
    }
    if let Ok(reader) = ImageReader::open(&media.path).and_then(|r| r.with_guessed_format()) {
        if let Ok(decoded) = reader.decode() {
            let scaled = if decoded.width() > 1280 || decoded.height() > 960 {
                decoded.resize(1280, 960, FilterType::Triangle)
            } else {
                decoded
            };
            let _ = DynamicImage::ImageRgb8(scaled.to_rgb8())
                .save_with_format(&path, ImageFormat::Jpeg);
            return Ok(scaled.into_rgba8());
        }
    }
    let temp = tempfile::tempdir_in(&dir)?;
    let out = command(ensure_mpv()?)
        .args([
            "--no-config",
            "--vo=image",
            "--vo-image-format=jpg",
            "--vo-image-jpeg-quality=85",
            "--vf=lavfi=[scale=1280:960:force_original_aspect_ratio=decrease]",
            "--frames=1",
            "--no-audio",
            "--no-sub",
            "--image-display-duration=0",
            "--msg-level=all=error",
        ])
        .arg(format!("--vo-image-outdir={}", temp.path().display()))
        .arg("--")
        .arg(&media.path)
        .output()
        .context("Cannot generate media preview with mpv")?;
    ensure!(
        out.status.success(),
        "Preview failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let frame = fs::read_dir(temp.path())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("jpg"))
        .context("Preview did not produce a frame")?;
    let bytes = fs::read(&frame)?;
    let image = image::load_from_memory(&bytes)?.into_rgba8();
    let _ = fs::write(&path, bytes);
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
