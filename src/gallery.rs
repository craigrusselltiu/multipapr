use crate::{
    http::{Cancellation, HttpRequest},
    storage,
};
use anyhow::{Context, Result, bail, ensure};
use image::{ImageFormat, ImageReader, RgbaImage};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    fmt, fs,
    io::{self, Cursor, Read, Write},
    path::{Path, PathBuf},
};
use tempfile::NamedTempFile;
use url::Url;
use windows_sys::Win32::Networking::WinHttp::WINHTTP_QUERY_CONTENT_TYPE;

mod desktophut;
mod library;
mod motionbgs;
mod preview;

pub use library::{
    Catalog, PreparedWallpaper, Removal, SavedWallpaper, clear, import, prepare, read_catalog,
    remove, save_download, saved_thumbnail,
};
pub use preview::{PreviewFrames, local_motion_preview, motion_preview};

const COMMONS_API: &str = "https://commons.wikimedia.org/w/api.php";
const COMMONS_MEDIA: &str = "https://upload.wikimedia.org/wikipedia/commons/";
const COMMONS_TRANSCODES: &str = "https://upload.wikimedia.org/wikipedia/commons/transcoded/";
const COMMONS_PAGE: &str = "https://commons.wikimedia.org/wiki/File:";
const NASA_PAGE: &str = "https://images.nasa.gov/details/";
const NASA_MEDIA: &[&str] = &[
    "https://images-assets.nasa.gov/image/",
    "https://images-assets.nasa.gov/video/",
];
const MAX_DOWNLOAD: u64 = 256 * 1024 * 1024;

#[derive(Debug)]
pub struct UnavailableWallpaper;

impl fmt::Display for UnavailableWallpaper {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("This wallpaper is no longer available")
    }
}

impl std::error::Error for UnavailableWallpaper {}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Commons,
    Nasa,
    MotionBgs,
    DesktopHut,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::Commons => "Wikimedia Commons",
            Self::Nasa => "NASA Image and Video Library",
            Self::MotionBgs => "MotionBGs",
            Self::DesktopHut => "DesktopHut",
        }
    }

    fn download(self, raw: &str) -> Option<Url> {
        let mut url = match self {
            Self::Commons => trusted_url(raw, &[COMMONS_MEDIA]),
            Self::Nasa => trusted_url(raw, NASA_MEDIA),
            Self::MotionBgs => motionbgs::download_url(raw),
            Self::DesktopHut => desktophut::wallpaper_url(raw),
        }?;
        url.set_query(None);
        Some(url)
    }

    fn thumbnail(self, raw: &str) -> Option<Url> {
        let prefixes: &[_] = match self {
            Self::Commons => &[
                "https://thumb.wikimedia.org/wikipedia/commons/thumb/",
                "https://upload.wikimedia.org/wikipedia/commons/thumb/",
            ],
            Self::Nasa => NASA_MEDIA,
            Self::MotionBgs => &[motionbgs::THUMBNAIL],
            Self::DesktopHut => &[desktophut::THUMBNAIL],
        };
        let mut url = trusted_url(raw, prefixes)?;
        url.set_query(None);
        Some(url)
    }

    fn page(self, raw: &str) -> Option<Url> {
        let prefix = match self {
            Self::Commons => COMMONS_PAGE,
            Self::Nasa => NASA_PAGE,
            Self::MotionBgs => return motionbgs::wallpaper_url(raw),
            Self::DesktopHut => return desktophut::wallpaper_url(raw),
        };
        trusted_url(raw, &[prefix])
    }
}

fn trusted_url(raw: &str, prefixes: &[&str]) -> Option<Url> {
    let mut url = Url::parse(raw).ok()?;
    url.set_fragment(None);
    prefixes
        .iter()
        .any(|prefix| url.as_str().starts_with(prefix))
        .then_some(url)
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Format {
    Jpg,
    Png,
    Mp4,
    Webm,
    Ogv,
}

impl Format {
    fn from_mime(mime: &str) -> Option<Self> {
        Some(match mime {
            "image/jpeg" => Self::Jpg,
            "image/png" => Self::Png,
            "video/mp4" => Self::Mp4,
            "video/webm" => Self::Webm,
            "video/ogg" | "application/ogg" => Self::Ogv,
            _ => return None,
        })
    }

    fn animated(self) -> bool {
        matches!(self, Self::Mp4 | Self::Webm | Self::Ogv)
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Jpg => "jpg",
            Self::Png => "png",
            Self::Mp4 => "mp4",
            Self::Webm => "webm",
            Self::Ogv => "ogv",
        }
    }

    fn matches_header(self, header: &[u8; 12]) -> bool {
        match self {
            Self::Jpg => header.starts_with(b"\xff\xd8\xff"),
            Self::Png => header.starts_with(b"\x89PNG\r\n\x1a\n"),
            Self::Mp4 => &header[4..8] == b"ftyp",
            Self::Webm => header.starts_with(b"\x1a\x45\xdf\xa3"),
            Self::Ogv => header.starts_with(b"OggS"),
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct Listing {
    pub title: String,
    pub source: Source,
    pub width: u32,
    pub height: u32,
    pub thumbnail: Option<String>,
    #[serde(default)]
    preview: Option<String>,
    pub page: String,
    pub download: String,
    #[serde(skip)]
    original_mp4: Option<Url>,
    #[serde(skip)]
    pub trending: bool,
    format: Format,
    keywords: String,
}

impl Listing {
    pub fn animated(&self) -> bool {
        self.format.animated()
    }

    pub fn motion_preview_key(&self) -> Option<&str> {
        if !self.animated() {
            return None;
        }
        self.preview
            .as_deref()
            .or_else(|| matches!(self.source, Source::MotionBgs).then_some(self.page.as_str()))
    }

    fn validate(mut self) -> Option<Self> {
        self.download = self.source.download(&self.download)?.into();
        self.page = self.source.page(&self.page)?.into();
        self.thumbnail = self
            .thumbnail
            .and_then(|raw| self.source.thumbnail(&raw).map(Into::into));
        self.preview = self
            .preview
            .and_then(|raw| preview::video_url(self.source, &raw).map(Into::into));
        (self.width > 0 && self.height > 0).then_some(self)
    }
}

pub fn featured() -> Vec<Listing> {
    let catalog: Vec<Listing> = serde_json::from_str(include_str!("../assets/gallery.json"))
        .expect("invalid bundled Gallery catalog");
    catalog.into_iter().filter_map(Listing::validate).collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MediaFilter {
    #[default]
    All,
    Still,
    Animated,
    None,
}

impl MediaFilter {
    pub fn includes(self, animated: bool) -> bool {
        matches!(
            (self, animated),
            (Self::All, _) | (Self::Still, false) | (Self::Animated, true)
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resolution {
    #[default]
    Any,
    Hd,
    FullHd,
    Qhd,
    Uhd,
}

impl Resolution {
    pub const OPTIONS: [Self; 5] = [Self::Any, Self::Hd, Self::FullHd, Self::Qhd, Self::Uhd];

    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "Any resolution",
            Self::Hd => "720p+",
            Self::FullHd => "1080p+",
            Self::Qhd => "1440p+",
            Self::Uhd => "4K+",
        }
    }

    fn minimum(self) -> (u32, u32) {
        match self {
            Self::Any => (1, 1),
            Self::Hd => (1280, 720),
            Self::FullHd => (1920, 1080),
            Self::Qhd => (2560, 1440),
            Self::Uhd => (3840, 2160),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Criteria {
    pub text: String,
    pub media: MediaFilter,
    pub resolution: Resolution,
}

impl Criteria {
    pub fn matches_featured(&self, listing: &Listing) -> bool {
        if !self.matches_filters(listing) {
            return false;
        }
        if self.text.is_empty() {
            return true;
        }
        let title = listing.title.to_lowercase();
        let keywords = listing.keywords.to_lowercase();
        self.text
            .split_whitespace()
            .all(|word| title.contains(word) || keywords.contains(word))
    }

    fn matches_filters(&self, listing: &Listing) -> bool {
        self.matches_media(listing.width, listing.height, listing.animated())
    }

    pub fn matches_saved(&self, item: &SavedWallpaper) -> bool {
        let title = item.title.to_lowercase();
        self.matches_media(item.media.width, item.media.height, item.animated)
            && self
                .text
                .split_whitespace()
                .all(|word| title.contains(word))
    }

    fn matches_media(&self, width: u32, height: u32, animated: bool) -> bool {
        let (long, short) = self.resolution.minimum();
        width.max(height) >= long && width.min(height) >= short && self.media.includes(animated)
    }
}

pub fn search_text(text: &str) -> String {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Deserialize)]
pub struct CommonsContinuation {
    gsroffset: u32,
    #[serde(rename = "continue")]
    token: String,
}

#[derive(Clone)]
pub enum SearchCursor {
    Commons(Option<CommonsContinuation>),
    MotionBgs(Option<Url>),
    DesktopHut(Option<Url>),
}

impl SearchCursor {
    pub fn supports(&self, criteria: &Criteria) -> bool {
        match self {
            Self::Commons(_) => criteria.media.includes(false) && !criteria.text.is_empty(),
            Self::MotionBgs(_) | Self::DesktopHut(_) => criteria.media.includes(true),
        }
    }

    pub fn fetch(&self, criteria: &Criteria, cancellation: &Cancellation) -> Result<Page> {
        match self {
            Self::Commons(from) => search_commons(criteria, from.as_ref(), cancellation),
            Self::MotionBgs(from) => motionbgs::search(criteria, from.as_ref(), cancellation),
            Self::DesktopHut(from) => desktophut::search(criteria, from.as_ref(), cancellation),
        }
    }
}

pub struct Page {
    pub listings: Vec<Listing>,
    pub next: Option<SearchCursor>,
}

#[derive(Deserialize)]
struct CommonsResponse {
    #[serde(rename = "continue")]
    next: Option<CommonsContinuation>,
    query: Option<CommonsQuery>,
    error: Option<CommonsError>,
}

#[derive(Deserialize)]
struct CommonsError {
    info: String,
}

#[derive(Deserialize)]
struct CommonsQuery {
    pages: Vec<CommonsPage>,
}

#[derive(Deserialize)]
struct CommonsPage {
    title: String,
    #[serde(default)]
    index: u32,
    #[serde(default)]
    imageinfo: Vec<CommonsFile>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct CommonsFile {
    url: String,
    thumburl: Option<String>,
    descriptionurl: String,
    mime: String,
    width: u32,
    height: u32,
    size: u64,
}

fn search_commons(
    criteria: &Criteria,
    from: Option<&CommonsContinuation>,
    cancellation: &Cancellation,
) -> Result<Page> {
    if !criteria.media.includes(false) {
        return Ok(Page {
            listings: Vec::new(),
            next: None,
        });
    }
    let min_edge = criteria.resolution.minimum().1 - 1;
    let max_kib = MAX_DOWNLOAD / 1024;
    let query = format!(
        "{} filetype:bitmap filew:>{min_edge} fileh:>{min_edge} filesize:<{max_kib}",
        criteria.text
    );
    let mut url = Url::parse(COMMONS_API)?;
    url.query_pairs_mut().extend_pairs([
        ("action", "query"),
        ("format", "json"),
        ("formatversion", "2"),
        ("maxlag", "5"),
        ("generator", "search"),
        ("gsrnamespace", "6"),
        ("gsrlimit", "24"),
        ("gsrsearch", &query),
        ("prop", "imageinfo"),
        ("iiprop", "url|size|mime"),
        ("iiurlwidth", "500"),
    ]);
    if let Some(from) = from {
        url.query_pairs_mut()
            .append_pair("gsroffset", &from.gsroffset.to_string())
            .append_pair("continue", &from.token);
    }
    let bytes = get(&url, 15_000, Some(cancellation))?.read_all(512 * 1024)?;
    cancellation.check()?;
    parse_results(&bytes, criteria)
}

fn parse_results(bytes: &[u8], criteria: &Criteria) -> Result<Page> {
    let response: CommonsResponse =
        serde_json::from_slice(bytes).context("Invalid Commons response")?;
    if let Some(error) = response.error {
        bail!("{}", error.info);
    }
    let mut pages = response.query.map_or_else(Vec::new, |query| query.pages);
    pages.sort_by_key(|page| page.index);
    let listings = pages
        .into_iter()
        .filter_map(|page| {
            let file = page.imageinfo.into_iter().next()?;
            if !(1..=MAX_DOWNLOAD).contains(&file.size) {
                return None;
            }
            let title = page.title.strip_prefix("File:")?;
            let (title, _) = title.rsplit_once('.')?;
            let format = Format::from_mime(&file.mime).filter(|format| !format.animated())?;
            Listing {
                title: title.replace('_', " "),
                source: Source::Commons,
                width: file.width,
                height: file.height,
                thumbnail: file.thumburl,
                preview: None,
                page: file.descriptionurl,
                download: file.url,
                original_mp4: None,
                trending: false,
                format,
                keywords: String::new(),
            }
            .validate()
            .filter(|listing| criteria.matches_filters(listing))
        })
        .collect();
    Ok(Page {
        listings,
        next: response.next.map(|next| SearchCursor::Commons(Some(next))),
    })
}

fn get(url: &Url, timeout_ms: i32, cancellation: Option<&Cancellation>) -> Result<HttpRequest> {
    let response = match cancellation {
        Some(cancellation) => HttpRequest::get_cancellable(url.as_str(), timeout_ms, cancellation),
        None => HttpRequest::send("GET", url.as_str(), &[], false, timeout_ms),
    }?;
    ensure!(
        response.status == 200,
        "Server returned HTTP {}",
        response.status
    );
    Ok(response)
}

pub fn thumbnail(source: Source, raw: &str, cancellation: &Cancellation) -> Result<RgbaImage> {
    let url = source.thumbnail(raw).context("Unrecognized preview URL")?;
    let bytes = get(&url, 10_000, Some(cancellation))?.read_all(2 * 1024 * 1024)?;
    cancellation.check()?;
    let format = image::guess_format(&bytes)?;
    ensure!(
        matches!(
            format,
            ImageFormat::Jpeg | ImageFormat::Png | ImageFormat::WebP
        ),
        "Unsupported preview format"
    );
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    reader.limits(limits);
    Ok(reader.decode()?.thumbnail(500, 360).into_rgba8())
}

fn download(listing: &Listing) -> Result<PathBuf> {
    let desktop_hut = matches!(listing.source, Source::DesktopHut);
    let media_url = if desktop_hut {
        listing
            .original_mp4
            .as_ref()
            .and_then(|url| desktophut::original_mp4(url.as_str()))
            .ok_or(UnavailableWallpaper)?
    } else {
        listing
            .source
            .download(&listing.download)
            .context("Unrecognized wallpaper URL")?
    };
    let dir = storage::wallpaper_dir()?;
    fs::create_dir_all(&dir)?;
    let path = dir.join(library::file_name(
        &listing.title,
        &Sha256::digest(media_url.as_str()),
        listing.format.extension(),
    ));
    if path.is_file() {
        validate_cached(&path, listing.format)?;
        return Ok(path);
    }

    let report_download_error = |error: anyhow::Error| {
        if desktop_hut {
            UnavailableWallpaper.into()
        } else {
            error
        }
    };
    let (response, size) = (|| {
        let response = get(&media_url, 30_000, None)?;
        if matches!(listing.source, Source::MotionBgs | Source::DesktopHut) {
            let content_type = response.header_string(WINHTTP_QUERY_CONTENT_TYPE);
            ensure!(
                content_type.as_deref().is_some_and(is_mp4_content_type),
                "{} returned something other than an MP4 video",
                listing.source.name()
            );
        }
        let size = response.content_length();
        ensure!(
            size.is_none_or(|size| size <= MAX_DOWNLOAD),
            "Wallpaper is too large"
        );
        Ok((response, size))
    })()
    .map_err(report_download_error)?;
    let mut response = response.take(MAX_DOWNLOAD + 1);
    let header =
        read_wallpaper_header(listing.format, &mut response).map_err(report_download_error)?;
    let mut file = NamedTempFile::new_in(&dir)?;
    file.write_all(&header)?;
    let bytes = header.len() as u64 + io::copy(&mut response, &mut file)?;
    ensure!(bytes <= MAX_DOWNLOAD, "Wallpaper is too large");
    ensure!(
        size.is_none_or(|size| size == bytes),
        "Wallpaper download was incomplete"
    );
    file.flush()?;
    match file.persist_noclobber(&path) {
        Ok(_) => Ok(path),
        Err(e) if e.error.kind() == io::ErrorKind::AlreadyExists => {
            validate_cached(&path, listing.format)?;
            Ok(path)
        }
        Err(e) => Err(e.error.into()),
    }
}

fn is_mp4_content_type(value: &str) -> bool {
    let media_type = value
        .split_once(';')
        .map_or(value, |(media_type, _)| media_type);
    media_type.trim().eq_ignore_ascii_case("video/mp4")
}

fn read_wallpaper_header(format: Format, reader: &mut impl Read) -> Result<[u8; 12]> {
    let mut header = [0; 12];
    reader
        .read_exact(&mut header)
        .context("Incomplete wallpaper file")?;
    ensure!(
        format.matches_header(&header),
        "Unexpected wallpaper format"
    );
    Ok(header)
}

fn validate_cached(path: &Path, format: Format) -> Result<()> {
    let info = fs::symlink_metadata(path)?;
    ensure!(
        info.is_file() && info.len() <= MAX_DOWNLOAD,
        "Invalid cached wallpaper"
    );
    let mut file = fs::File::open(path)?;
    read_wallpaper_header(format, &mut file).context("Invalid cached wallpaper")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mp4_downloads_require_video_content_and_a_matching_file_header() {
        assert!(is_mp4_content_type("video/mp4"));
        assert!(!is_mp4_content_type("text/html"));
        assert!(!is_mp4_content_type("application/x-msdownload"));
        for data in [
            b"<!doctype ht".as_slice(),
            b"MZ\0\0\0\0\0\0\0\0\0\0",
            b"PK\x03\x04\0\0\0\0\0\0\0\0",
            b"ftyp",
        ] {
            assert!(read_wallpaper_header(Format::Mp4, &mut Cursor::new(data)).is_err());
        }
        assert!(
            read_wallpaper_header(Format::Mp4, &mut Cursor::new(b"\0\0\0\x18ftypmp42")).is_ok()
        );
    }

    #[test]
    fn featured_catalog_contains_both_sources_and_filters_by_text_type_and_resolution() {
        let catalog = featured();
        assert!(catalog.iter().all(|item| item.thumbnail.is_some()));
        assert!(
            catalog
                .iter()
                .any(|item| matches!(item.source, Source::Commons) && !item.animated())
        );
        assert!(
            catalog
                .iter()
                .any(|item| matches!(item.source, Source::Nasa) && item.animated())
        );
        let criteria = Criteria {
            text: search_text("  Night-Sky  "),
            media: MediaFilter::Animated,
            resolution: Resolution::Uhd,
        };
        assert!(
            catalog
                .iter()
                .any(|item| item.title == "Northern lights over Iceland"
                    && criteria.matches_featured(item))
        );
        assert!(
            catalog
                .iter()
                .any(|item| item.title == "Milky Way over Glen Canyon"
                    && !criteria.matches_featured(item))
        );
    }

    #[test]
    fn search_results_keep_portrait_matches_and_reject_untrusted_or_unsupported_files() {
        let page = |index, title, url, mime, width, height| {
            serde_json::json!({
                "title": title, "index": index, "imageinfo": [{
                    "url": url, "descriptionurl": format!("{COMMONS_PAGE}Example"),
                    "mime": mime, "width": width, "height": height, "size": 4096,
                }],
            })
        };
        let response = serde_json::json!({
            "continue": {"gsroffset": 24, "continue": "gsroffset||"},
            "query": {"pages": [
                page(2, "File:Landscape.jpg", format!("{COMMONS_MEDIA}landscape.jpg?utm_source=commons"), "image/jpeg", 3840, 2160),
                page(1, "File:Portrait.png", format!("{COMMONS_MEDIA}portrait.png"), "image/png", 2160, 3840),
                page(3, "File:Too small.jpg", format!("{COMMONS_MEDIA}small.jpg"), "image/jpeg", 3800, 2160),
                page(4, "File:Unsupported.svg", format!("{COMMONS_MEDIA}vector.svg"), "image/svg+xml", 3840, 2160),
                page(5, "File:Untrusted.jpg", "https://upload.wikimedia.org.example.com/wikipedia/commons/fake.jpg".into(), "image/jpeg", 3840, 2160),
                {"title": "File:Missing.jpg", "index": 6, "imageinfo": [{}]},
                page(7, "File:Gameplay.webm", format!("{COMMONS_MEDIA}gameplay.webm"), "video/webm", 3840, 2160),
            ]},
        });
        let parsed = parse_results(
            &serde_json::to_vec(&response).unwrap(),
            &Criteria {
                resolution: Resolution::Uhd,
                ..Criteria::default()
            },
        )
        .unwrap();
        let titles: Vec<_> = parsed
            .listings
            .iter()
            .map(|item| item.title.as_str())
            .collect();
        assert_eq!(titles, ["Portrait", "Landscape"]);
        assert_eq!(
            parsed.listings[1].download,
            format!("{COMMONS_MEDIA}landscape.jpg")
        );
        assert!(
            matches!(parsed.next, Some(SearchCursor::Commons(Some(next))) if next.gsroffset == 24)
        );
        let cursor = SearchCursor::Commons(None);
        let mut criteria = Criteria {
            text: "minecraft".into(),
            media: MediaFilter::Animated,
            ..Criteria::default()
        };
        assert!(!cursor.supports(&criteria));
        criteria.media = MediaFilter::Still;
        assert!(cursor.supports(&criteria));
    }
}
