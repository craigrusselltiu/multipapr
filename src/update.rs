use crate::http::HttpRequest;
use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    cmp::Ordering,
    env, fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering as AtomicOrdering},
    time::Duration,
};
use tempfile::NamedTempFile;
use windows_sys::Win32::Networking::WinHttp::*;

const ASSET_NAME: &str = "multipapr.exe";
const LATEST_API_URL: &str =
    "https://api.github.com/repos/craigrusselltiu/multipapr/releases/latest";
const LATEST_WEB_URL: &str = "https://github.com/craigrusselltiu/multipapr/releases/latest";
const DOWNLOAD_PREFIX: &str = "https://github.com/craigrusselltiu/multipapr/releases/download/";
const TAG_PATH_SEGMENT: &str = "/releases/tag/";
const MAX_API_BYTES: usize = 2 * 1024 * 1024;

pub const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub prerelease: Option<String>,
}

impl Version {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        let trimmed = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);
        let without_build = trimmed.split('+').next()?;
        let (core, prerelease) = match without_build.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_string())),
            Some(_) => return None,
            None => (without_build, None),
        };
        let mut parts = core.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = parts.next()?.parse().ok()?;
        let patch = parts.next()?.parse().ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            prerelease,
        })
    }

    pub fn display(&self) -> String {
        match &self.prerelease {
            Some(pre) => format!("v{}.{}.{}-{pre}", self.major, self.minor, self.patch),
            None => format!("v{}.{}.{}", self.major, self.minor, self.patch),
        }
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.prerelease, &other.prerelease) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => compare_prerelease(a, b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn compare_prerelease(a: &str, b: &str) -> Ordering {
    for (x, y) in a.split('.').zip(b.split('.')) {
        let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(nx), Ok(ny)) => nx.cmp(&ny),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => x.cmp(y),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.split('.').count().cmp(&b.split('.').count())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    pub download_url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckOutcome {
    Latest,
    Available(Release),
}

pub fn current_version_label() -> String {
    format!("v{CURRENT_VERSION}")
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: Option<u64>,
    digest: Option<String>,
}

fn parse_api_release(body: &[u8], current: &Version) -> Result<CheckOutcome> {
    let api: ApiRelease =
        serde_json::from_slice(body).context("Unexpected response from GitHub")?;
    if api.draft || api.prerelease {
        return Ok(CheckOutcome::Latest);
    }
    let version = Version::parse(&api.tag_name)
        .with_context(|| format!("Invalid release tag {}", api.tag_name))?;
    if version <= *current {
        return Ok(CheckOutcome::Latest);
    }
    let Some(asset) = api
        .assets
        .into_iter()
        .find(|a| a.name.eq_ignore_ascii_case(ASSET_NAME))
    else {
        return Ok(CheckOutcome::Latest);
    };
    ensure!(
        asset.browser_download_url.starts_with(DOWNLOAD_PREFIX),
        "Unexpected download URL from GitHub"
    );
    let sha256 = asset.digest.and_then(|d| {
        d.strip_prefix("sha256:")
            .filter(|hex| hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(|hex| hex.to_ascii_lowercase())
    });
    Ok(CheckOutcome::Available(Release {
        version,
        download_url: asset.browser_download_url,
        size: asset.size,
        sha256,
    }))
}

fn parse_redirect_release(location: &str, current: &Version) -> Result<CheckOutcome> {
    let (_, rest) = location
        .split_once(TAG_PATH_SEGMENT)
        .context("Unexpected redirect from GitHub")?;
    let tag = rest.split(['/', '?', '#']).next().unwrap_or("");
    let version = Version::parse(tag).with_context(|| format!("Invalid release tag {tag}"))?;
    if version <= *current {
        return Ok(CheckOutcome::Latest);
    }
    Ok(CheckOutcome::Available(Release {
        version,
        download_url: format!("{DOWNLOAD_PREFIX}{tag}/{ASSET_NAME}"),
        size: None,
        sha256: None,
    }))
}

pub fn check_latest() -> Result<CheckOutcome> {
    let current = Version::parse(CURRENT_VERSION).context("Invalid application version")?;
    let response = HttpRequest::send(
        "GET",
        LATEST_API_URL,
        &[
            "Accept: application/vnd.github+json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        true,
        15_000,
    )?;
    if response.status == HTTP_STATUS_OK {
        let body = response.read_all(MAX_API_BYTES)?;
        return parse_api_release(&body, &current);
    }
    if response.status == HTTP_STATUS_FORBIDDEN || response.status == 429 {
        let redirect = HttpRequest::send("HEAD", LATEST_WEB_URL, &[], false, 15_000)?;
        if matches!(
            redirect.status,
            HTTP_STATUS_MOVED
                | HTTP_STATUS_REDIRECT
                | HTTP_STATUS_REDIRECT_METHOD
                | HTTP_STATUS_REDIRECT_KEEP_VERB
                | HTTP_STATUS_PERMANENT_REDIRECT
        ) && let Some(location) = redirect.header_string(WINHTTP_QUERY_LOCATION)
        {
            return parse_redirect_release(&location, &current);
        }
    }
    bail!("GitHub returned HTTP {}", response.status)
}

pub fn download_update(
    release: &Release,
    target_dir: &Path,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<NamedTempFile> {
    if !cfg!(target_arch = "x86_64") {
        bail!("Prebuilt updates currently target Windows x64");
    }
    ensure!(
        release.download_url.starts_with(DOWNLOAD_PREFIX),
        "Unexpected update download URL"
    );
    let mut staged =
        NamedTempFile::new_in(target_dir).context("Cannot write to the Multipapr folder")?;
    let mut response = HttpRequest::send("GET", &release.download_url, &[], true, 60_000)?;
    ensure!(
        response.status == HTTP_STATUS_OK,
        "Download failed (HTTP {})",
        response.status
    );
    let total = response.content_length().or(release.size);
    let mut hasher = Sha256::new();
    let mut downloaded: u64 = 0;
    let mut magic = [0u8; 2];
    let mut magic_len = 0usize;
    let mut buf = vec![0u8; 64 * 1024];
    on_progress(0, total);
    loop {
        if cancel.load(AtomicOrdering::Relaxed) {
            bail!("Update cancelled");
        }
        let read = response.read(&mut buf).context("Download interrupted")?;
        if read == 0 {
            break;
        }
        let slice = &buf[..read];
        if magic_len < 2 {
            let take = (2 - magic_len).min(slice.len());
            magic[magic_len..magic_len + take].copy_from_slice(&slice[..take]);
            magic_len += take;
        }
        hasher.update(slice);
        staged
            .write_all(slice)
            .context("Cannot write update file")?;
        downloaded += read as u64;
        on_progress(downloaded, total);
    }
    ensure!(
        magic_len == 2 && magic == *b"MZ",
        "Downloaded file is not a valid Windows executable"
    );
    if let Some(expected_total) = total {
        ensure!(
            downloaded == expected_total,
            "Download was incomplete ({downloaded} of {expected_total} bytes)"
        );
    }
    if let Some(expected) = &release.sha256 {
        let actual: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        ensure!(
            actual.eq_ignore_ascii_case(expected),
            "Downloaded file failed integrity check"
        );
    }
    staged.flush().context("Cannot flush update file")?;
    Ok(staged)
}

fn backup_path_for(exe_path: &Path) -> Result<PathBuf> {
    let dir = exe_path
        .parent()
        .context("Cannot determine Multipapr folder")?;
    let name = exe_path
        .file_name()
        .and_then(|n| n.to_str())
        .context("Cannot determine executable name")?;
    let primary = dir.join(format!("{name}.old"));
    if !primary.exists() || fs::remove_file(&primary).is_ok() {
        return Ok(primary);
    }
    let pid = std::process::id();
    for attempt in 0..100u32 {
        let candidate = dir.join(format!("{name}.old.{pid}.{attempt}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    bail!("Cannot pick backup path for old executable")
}

pub fn replace_executable(exe_path: &Path, staged: NamedTempFile) -> Result<()> {
    let backup = backup_path_for(exe_path)?;
    fs::rename(exe_path, &backup).context("Cannot move the running multipapr.exe aside")?;
    if let Err(err) = staged.persist(exe_path) {
        let _ = fs::rename(&backup, exe_path);
        return Err(err.error).context("Cannot place updated multipapr.exe");
    }
    Ok(())
}

fn remove_old_executables_in(dir: &Path, exe_name: &str) -> bool {
    let prefix = format!("{exe_name}.old");
    let Ok(entries) = fs::read_dir(dir) else {
        return true;
    };
    let mut remaining = false;
    for entry in entries.flatten() {
        let path = entry.path();
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == prefix || n.starts_with(&format!("{prefix}.")))
            && fs::remove_file(&path).is_err()
        {
            remaining = true;
        }
    }
    !remaining
}

pub fn cleanup_old_executables() {
    let Ok(exe) = env::current_exe() else {
        return;
    };
    let Some(dir) = exe.parent().map(Path::to_path_buf) else {
        return;
    };
    let Some(name) = exe.file_name().and_then(|n| n.to_str()).map(str::to_owned) else {
        return;
    };
    if remove_old_executables_in(&dir, &name) {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..16 {
            std::thread::sleep(Duration::from_millis(250));
            if remove_old_executables_in(&dir, &name) {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_numerically_and_handles_v_prefix() {
        let v013 = Version::parse("v0.1.3").unwrap();
        let v014 = Version::parse("0.1.4").unwrap();
        let v0110 = Version::parse("v0.1.10").unwrap();
        let v014_rc = Version::parse("v0.1.4-rc.1").unwrap();
        assert_eq!(v013.display(), "v0.1.3");
        assert!(v014 > v013);
        assert!(v0110 > v014);
        assert!(v014 > v014_rc);
        assert!(v014_rc > v013);
        assert_eq!(Version::parse("v0.1.3+build.7"), Some(v013));
        assert_eq!(Version::parse("not-a-version"), None);
    }

    #[test]
    fn parses_github_api_release_response() {
        let current = Version::parse("0.1.3").unwrap();
        let same = br#"{
            "tag_name": "v0.1.3",
            "draft": false,
            "prerelease": false,
            "assets": [{
                "name": "multipapr.exe",
                "browser_download_url": "https://github.com/craigrusselltiu/multipapr/releases/download/v0.1.3/multipapr.exe",
                "size": 1234,
                "digest": "sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
            }]
        }"#;
        assert_eq!(
            parse_api_release(same, &current).unwrap(),
            CheckOutcome::Latest
        );

        let newer = br#"{
            "tag_name": "v0.1.4",
            "draft": false,
            "prerelease": false,
            "assets": [{
                "name": "multipapr.exe",
                "browser_download_url": "https://github.com/craigrusselltiu/multipapr/releases/download/v0.1.4/multipapr.exe",
                "size": 52000000,
                "digest": "sha256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"
            }]
        }"#;
        assert_eq!(
            parse_api_release(newer, &current).unwrap(),
            CheckOutcome::Available(Release {
                version: Version::parse("0.1.4").unwrap(),
                download_url: "https://github.com/craigrusselltiu/multipapr/releases/download/v0.1.4/multipapr.exe".into(),
                size: Some(52_000_000),
                sha256: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into()),
            })
        );

        let missing_exe = br#"{
            "tag_name": "v0.1.4",
            "draft": false,
            "prerelease": false,
            "assets": [{"name": "GPL-3.0.txt", "browser_download_url": "https://github.com/craigrusselltiu/multipapr/releases/download/v0.1.4/GPL-3.0.txt"}]
        }"#;
        assert_eq!(
            parse_api_release(missing_exe, &current).unwrap(),
            CheckOutcome::Latest
        );

        let wrong_host = br#"{
            "tag_name": "v0.1.4",
            "draft": false,
            "prerelease": false,
            "assets": [{"name": "multipapr.exe", "browser_download_url": "https://evil.example/multipapr.exe"}]
        }"#;
        assert!(parse_api_release(wrong_host, &current).is_err());
    }

    #[test]
    fn parses_redirect_location_fallback() {
        let current = Version::parse("0.1.3").unwrap();
        assert_eq!(
            parse_redirect_release(
                "https://github.com/craigrusselltiu/multipapr/releases/tag/v0.1.3",
                &current
            )
            .unwrap(),
            CheckOutcome::Latest
        );
        assert_eq!(
            parse_redirect_release(
                "https://github.com/craigrusselltiu/multipapr/releases/tag/v0.2.0",
                &current
            )
            .unwrap(),
            CheckOutcome::Available(Release {
                version: Version::parse("0.2.0").unwrap(),
                download_url:
                    "https://github.com/craigrusselltiu/multipapr/releases/download/v0.2.0/multipapr.exe"
                        .into(),
                size: None,
                sha256: None,
            })
        );
    }

    #[test]
    fn replaces_executable_and_cleans_up_old_backups() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("multipapr.exe");
        fs::write(&exe, b"old-binary").unwrap();

        let mut staged = NamedTempFile::new_in(dir.path()).unwrap();
        staged.write_all(b"new-binary").unwrap();
        staged.flush().unwrap();

        replace_executable(&exe, staged).unwrap();
        assert_eq!(fs::read(&exe).unwrap(), b"new-binary");
        assert_eq!(
            fs::read(dir.path().join("multipapr.exe.old")).unwrap(),
            b"old-binary"
        );

        assert!(remove_old_executables_in(dir.path(), "multipapr.exe"));
        assert!(!dir.path().join("multipapr.exe.old").exists());
        assert_eq!(fs::read(&exe).unwrap(), b"new-binary");
    }

    #[test]
    fn replaces_actively_running_windows_executable() {
        let dir = tempfile::tempdir().unwrap();
        let running_exe = dir.path().join("runner.exe");
        let system_exe = Path::new(r"C:\Windows\System32\ping.exe");
        if !system_exe.is_file() {
            return;
        }
        fs::copy(system_exe, &running_exe).unwrap();

        let mut child = crate::process::spawn(crate::storage::command(&running_exe).args([
            "127.0.0.1",
            "-n",
            "4",
        ]))
        .unwrap();

        let mut staged = NamedTempFile::new_in(dir.path()).unwrap();
        staged.write_all(b"MZ-updated").unwrap();
        staged.flush().unwrap();

        let swap = replace_executable(&running_exe, staged);
        let _ = child.kill();
        let _ = child.wait();

        swap.unwrap();
        assert_eq!(fs::read(&running_exe).unwrap(), b"MZ-updated");
        assert!(remove_old_executables_in(dir.path(), "runner.exe"));
    }

    #[test]
    #[ignore = "requires network access to GitHub"]
    fn live_github_release_check_succeeds() -> Result<()> {
        check_latest()?;
        Ok(())
    }
}
