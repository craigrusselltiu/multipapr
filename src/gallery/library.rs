use super::{Listing, Source, download};
use crate::{layout::Media, storage};
use anyhow::{Context, Result, ensure};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    thread,
    time::Duration,
};
use tempfile::NamedTempFile;

const DETAILS_SUFFIX: &str = ".gallery.json";
static SAVED_FILES: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct SavedWallpaper {
    pub media: Media,
    pub title: String,
    pub origin: Option<String>,
    pub animated: bool,
    pub(crate) bytes: u64,
}

pub struct PreparedWallpaper {
    pub saved: SavedWallpaper,
    pub image: RgbaImage,
}

#[derive(Default)]
pub struct Catalog {
    pub items: Vec<SavedWallpaper>,
    pub bytes: u64,
}

impl Catalog {
    pub fn find(&self, listing: &Listing) -> Option<&SavedWallpaper> {
        self.items
            .iter()
            .find(|item| item.origin.as_deref() == Some(&listing.download))
            .or_else(|| {
                if matches!(listing.source, Source::DesktopHut) {
                    return None;
                }
                let name = file_name(
                    &listing.title,
                    &Sha256::digest(&listing.download),
                    listing.format.extension(),
                );
                self.items.iter().find(|item| {
                    item.origin.is_none() && item.media.path.file_name() == Some(name.as_ref())
                })
            })
    }

    pub fn upsert(&mut self, item: SavedWallpaper) {
        self.bytes = self.bytes.saturating_add(item.bytes);
        if let Some(index) = self
            .items
            .iter()
            .position(|old| old.media.path == item.media.path)
        {
            self.bytes = self.bytes.saturating_sub(self.items[index].bytes);
            self.items.remove(index);
        }
        self.items.insert(0, item);
    }

    pub fn forget(&mut self, paths: &[PathBuf]) {
        self.items.retain(|item| {
            if paths.contains(&item.media.path) {
                self.bytes = self.bytes.saturating_sub(item.bytes);
                false
            } else {
                true
            }
        });
    }
}

#[derive(Serialize, Deserialize)]
struct Details {
    title: String,
    origin: Option<String>,
    animated: bool,
    width: u32,
    height: u32,
}

fn details_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().map_or_else(OsString::new, Into::into);
    name.push(DETAILS_SUFFIX);
    path.with_file_name(name)
}

fn read_details(path: &Path) -> Option<Details> {
    let path = details_path(path);
    let info = fs::symlink_metadata(&path).ok()?;
    if !info.is_file() || info.len() > 32 * 1024 {
        return None;
    }
    let details: Details = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    storage::check_media_size(details.width, details.height).ok()?;
    Some(details)
}

pub(super) fn file_name(title: &str, digest: &[u8], extension: &str) -> String {
    let key: String = digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let title: String = title
        .chars()
        .take(64)
        .map(|ch| {
            if ch.is_alphanumeric() || ch == '-' {
                ch
            } else {
                ' '
            }
        })
        .collect();
    format!("{} - {key}.{extension}", title.trim())
}

fn file_title(path: &Path) -> String {
    let title = path.file_stem().unwrap_or_default().to_string_lossy();
    let title = title
        .rsplit_once(" - ")
        .filter(|(_, suffix)| suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
        .map_or(title.as_ref(), |(title, _)| title);
    if title.trim().is_empty() {
        "Wallpaper".into()
    } else {
        title.into()
    }
}

fn write_details(item: &SavedWallpaper) -> Result<()> {
    let path = &item.media.path;
    let _guard = SAVED_FILES.lock().unwrap_or_else(PoisonError::into_inner);
    regular_file(path)?;
    let mut staged = NamedTempFile::new_in(path.parent().context("Invalid Gallery path")?)?;
    serde_json::to_writer(
        &mut staged,
        &Details {
            title: item.title.clone(),
            origin: item.origin.clone(),
            width: item.media.width,
            height: item.media.height,
            animated: item.animated,
        },
    )?;
    staged.persist(details_path(path))?;
    Ok(())
}

fn regular_file(path: &Path) -> Result<fs::Metadata> {
    let info = fs::symlink_metadata(path)?;
    ensure!(info.is_file(), "Expected a wallpaper file");
    Ok(info)
}

fn new_saved(
    media: Media,
    title: String,
    origin: Option<String>,
    animated: bool,
) -> Result<SavedWallpaper> {
    let bytes = regular_file(&media.path)?.len();
    Ok(SavedWallpaper {
        media,
        title,
        origin,
        animated,
        bytes,
    })
}

fn saved_item(path: &Path, bytes: u64) -> Result<SavedWallpaper> {
    if let Some(details) = read_details(path) {
        return Ok(SavedWallpaper {
            media: Media {
                path: path.to_path_buf(),
                width: details.width,
                height: details.height,
            },
            title: details.title,
            origin: details.origin,
            animated: details.animated,
            bytes,
        });
    }
    let media = storage::probe_read_only(path)?;
    new_saved(media, file_title(path), None, !storage::still_image(path))
}

fn local_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                "png"
                    | "jpg"
                    | "jpeg"
                    | "webp"
                    | "bmp"
                    | "gif"
                    | "ico"
                    | "mp4"
                    | "m4v"
                    | "mov"
                    | "mkv"
                    | "webm"
                    | "ogv"
                    | "media"
            )
        })
        || details_path(path).is_file()
}

fn cache_file(name: &str) -> bool {
    name.ends_with(DETAILS_SUFFIX)
        || name
            .strip_prefix("preview-")
            .and_then(|name| name.strip_suffix(".jpg"))
            .is_some_and(|key| key.len() == 16 && key.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn read_catalog() -> Result<Catalog> {
    read_catalog_in(&storage::wallpaper_dir()?)
}

fn read_catalog_in(dir: &Path) -> Result<Catalog> {
    let mut items = Vec::new();
    for (path, info) in gallery_files(dir)? {
        if !cache_file(&path.file_name().unwrap_or_default().to_string_lossy())
            && local_file(&path)
            && let Ok(item) = saved_item(&path, info.len())
        {
            items.push((info.modified().ok(), item));
        }
    }
    items.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.title.cmp(&b.1.title)));
    let bytes = gallery_files(dir)?
        .filter(|(path, _)| {
            cache_file(&path.file_name().unwrap_or_default().to_string_lossy()) || local_file(path)
        })
        .map(|(_, info)| info.len())
        .sum();
    Ok(Catalog {
        items: items.into_iter().map(|(_, item)| item).collect(),
        bytes,
    })
}

fn gallery_files(dir: &Path) -> Result<impl Iterator<Item = (PathBuf, fs::Metadata)>> {
    let entries = match storage::canonical_path(dir).and_then(|path| Ok(fs::read_dir(path)?)) {
        Ok(entries) => Some(entries),
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    Ok(entries.into_iter().flatten().filter_map(|entry| {
        let entry = entry.ok()?;
        let path = entry.path();
        let info = regular_file(&path).ok()?;
        Some((path, info))
    }))
}

pub fn saved_thumbnail(media: &Media) -> Result<RgbaImage> {
    storage::preview_read_only(media)
}

pub fn prepare(saved: SavedWallpaper) -> Result<PreparedWallpaper> {
    regular_file(&saved.media.path)?;
    let image = storage::preview(&saved.media).or_else(|_| {
        storage::probe(&saved.media.path).and_then(|media| storage::preview(&media))
    })?;
    Ok(PreparedWallpaper { saved, image })
}

pub fn save_download(listing: &Listing) -> Result<PreparedWallpaper> {
    let path = download(listing)?;
    let media = storage::probe(&path)?;
    let saved = new_saved(
        media,
        listing.title.clone(),
        Some(listing.download.clone()),
        listing.animated(),
    )?;
    let wallpaper = prepare(saved)?;
    write_details(&wallpaper.saved)?;
    Ok(wallpaper)
}

pub fn import(path: &Path) -> Result<PreparedWallpaper> {
    let dir = storage::wallpaper_dir()?;
    let mut media = storage::probe(path)?;
    let info = regular_file(&media.path)?;
    if storage::in_wallpaper_dir(&media.path, &dir) {
        let saved = saved_item(&media.path, info.len())?;
        if read_details(&saved.media.path).is_none() {
            write_details(&saved)?;
        }
        return prepare(saved);
    }

    let image = storage::preview(&media)?;
    let animated = !storage::still_image(&media.path);
    let title = media
        .path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let extension = media
        .path
        .extension()
        .and_then(|ext| ext.to_str())
        .filter(|ext| {
            !ext.is_empty() && ext.len() <= 10 && ext.bytes().all(|b| b.is_ascii_alphanumeric())
        })
        .map_or_else(|| "media".to_owned(), str::to_ascii_lowercase);
    fs::create_dir_all(&dir)?;
    let mut file = File::open(&media.path)?;
    let mut staged = NamedTempFile::new_in(&dir)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
        staged.write_all(&buffer[..count])?;
    }
    media.path =
        storage::canonical_path(&dir)?.join(file_name(&title, &digest.finalize(), &extension));
    match staged.persist_noclobber(&media.path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.error.into()),
    }
    let saved = new_saved(media, title, None, animated)?;
    if read_details(&saved.media.path).is_none() {
        write_details(&saved)?;
    }
    let _ = storage::cache_preview(&saved.media, &image);
    Ok(PreparedWallpaper { saved, image })
}

#[derive(Default)]
pub struct Removal {
    pub paths: Vec<PathBuf>,
    pub error: Option<String>,
}

impl Removal {
    fn unlink(&mut self, path: &Path) -> bool {
        let mut result = fs::remove_file(path);
        for _ in 0..3 {
            if !result
                .as_ref()
                .is_err_and(|error| matches!(error.raw_os_error(), Some(32 | 33)))
            {
                break;
            }
            thread::sleep(Duration::from_millis(80));
            result = fs::remove_file(path);
        }
        match result {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Err(error) => {
                if self.error.is_none() {
                    self.error = Some(if matches!(error.raw_os_error(), Some(32 | 33)) {
                        format!(
                            "“{}” is still in use. Close apps using it and try again.",
                            file_title(path)
                        )
                    } else {
                        format!("Could not delete “{}”. {error}", file_title(path))
                    });
                }
                false
            }
        }
    }

    fn remove_wallpaper(&mut self, path: &Path) {
        let preview = storage::preview_path(path).ok();
        if self.unlink(path) {
            self.paths.push(path.to_path_buf());
            self.unlink(&details_path(path));
            if let Some(preview) = preview {
                self.unlink(&preview);
            }
        }
    }
}

pub fn remove(path: &Path) -> Result<Removal> {
    let dir = storage::wallpaper_dir()?;
    ensure!(
        storage::in_wallpaper_dir(path, &dir) && local_file(path),
        "Wallpaper is outside the Gallery"
    );
    let _guard = SAVED_FILES.lock().unwrap_or_else(PoisonError::into_inner);
    let mut result = Removal::default();
    result.remove_wallpaper(path);
    Ok(result)
}

pub fn clear() -> Result<Removal> {
    let dir = storage::wallpaper_dir()?;
    let _guard = SAVED_FILES.lock().unwrap_or_else(PoisonError::into_inner);
    let files: Vec<_> = gallery_files(&dir)?.map(|(path, _)| path).collect();
    let mut result = Removal::default();
    for path in &files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if !cache_file(&name) && local_file(path) {
            result.remove_wallpaper(path);
        }
    }
    for path in files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        // Keep metadata for any wallpaper Windows could not delete.
        if let Some(original) = name.strip_suffix(DETAILS_SUFFIX)
            && path.with_file_name(original).is_file()
        {
            continue;
        }
        if cache_file(&name) {
            result.unlink(&path);
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browsing_a_saved_library_does_not_create_directories_or_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Wallpapers");
        assert!(read_catalog_in(&dir).unwrap().items.is_empty());
        assert!(!dir.exists());

        fs::create_dir(&dir).unwrap();
        let path = dir.join("Photo.png");
        RgbaImage::new(32, 18).save(&path).unwrap();
        let catalog = read_catalog_in(&dir).unwrap();
        assert_eq!(catalog.items.len(), 1);
        assert_eq!(catalog.items[0].media.width, 32);
        assert_eq!(catalog.items[0].media.height, 18);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }
}
