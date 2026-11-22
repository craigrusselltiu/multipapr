use super::{Criteria, Format, Listing, Page, SearchCursor, Source, get, preview, trusted_url};
use crate::http::Cancellation;
use anyhow::{Result, ensure};
use scraper::{Html, Selector};
use std::{
    collections::HashSet,
    sync::{Mutex, mpsc::channel},
};
use url::Url;

const ROOT: &str = "https://www.desktophut.com/";
const POPULAR_PATH: &str = "/popular-live-wallpapers";
const DETAIL_WORKERS: usize = 8;
pub(super) const PREVIEWS: &[&str] = &[
    "https://www.desktophut.com/previews/",
    "https://media.desktophut.com/previews/",
];
const VIDEOS: &[&str] = &[
    "https://www.desktophut.com/files/",
    "https://media.desktophut.com/files/",
];
pub(super) const THUMBNAIL: &str = "https://www.desktophut.com/images/";

pub(super) fn wallpaper_url(raw: &str) -> Option<Url> {
    let url = trusted_url(raw, &[ROOT])?;
    let slug = url.path().trim_matches('/');
    (!slug.is_empty() && slug.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .then_some(url)
}

fn search_page(url: &Url) -> bool {
    url.as_str().starts_with(ROOT)
        && (url.path() == POPULAR_PATH
            || url
                .path()
                .strip_prefix("/search/")
                .is_some_and(|term| !term.is_empty() && !term.contains('/')))
}

fn document(url: &Url, cancellation: &Cancellation) -> Result<Html> {
    let bytes = get(url, 12_000, Some(cancellation))?.read_all(768 * 1024)?;
    cancellation.check()?;
    Ok(Html::parse_document(std::str::from_utf8(&bytes)?))
}

pub(super) fn search(
    criteria: &Criteria,
    from: Option<&Url>,
    pages: u8,
    cancellation: &Cancellation,
) -> Result<Page> {
    let mut url = Url::parse(ROOT)?;
    if let Some(from) = from {
        url = from.clone();
    } else if criteria.text.is_empty() {
        url.set_path(POPULAR_PATH);
    } else {
        url.path_segments_mut()
            .expect("DesktopHut URL has a path")
            .push("search")
            .push(&criteria.text);
    }
    ensure!(search_page(&url), "Unexpected DesktopHut search URL");
    let first = parse(&document(&url, cancellation)?, &url, criteria)?;
    let mut next = first.next.clone();
    let mut visited = HashSet::from([url]);
    let more = std::iter::from_fn(|| {
        let SearchCursor::DesktopHut(Some(url)) = next.take()? else {
            return None;
        };
        if !visited.insert(url.clone()) {
            return None;
        }
        let mut page = parse(&document(&url, cancellation).ok()?, &url, criteria).ok()?;
        page.next = page.next.filter(|cursor| {
            matches!(cursor, SearchCursor::DesktopHut(Some(url)) if !visited.contains(url))
        });
        next = page.next.clone();
        Some(page)
    });
    let batch = std::iter::once(first).chain(more).take(usize::from(pages));
    confirm_downloads(batch, cancellation, |url| document(url, cancellation))
}

fn confirm_downloads(
    pages: impl IntoIterator<Item = Page>,
    cancellation: &Cancellation,
    read_details: impl Fn(&Url) -> Result<Html> + Sync,
) -> Result<Page> {
    let (submit, waiting) = channel::<(usize, Listing)>();
    let waiting = Mutex::new(waiting);
    let (confirmed, finished) = channel();
    let next = std::thread::scope(|scope| {
        for _ in 0..DETAIL_WORKERS {
            scope.spawn(|| {
                while cancellation.check().is_ok() {
                    let task = waiting.lock().expect("detail queue").recv();
                    let Ok((position, mut listing)) = task else {
                        break;
                    };
                    if let Some(url) = wallpaper_url(&listing.page)
                        && let Ok(html) = read_details(&url)
                        && let Some(download) = download_link(&html, &url)
                    {
                        listing.original_mp4 = Some(download);
                        if listing.preview.is_none() {
                            listing.preview = preview_link(&html, &url);
                        }
                        let _ = confirmed.send((position, listing));
                    }
                }
            });
        }
        let mut position = 0;
        let mut next = None;
        // Checks run as each index page supplies candidates to the workers.
        for page in pages {
            next = page.next;
            for listing in page.listings {
                let _ = submit.send((position, listing));
                position += 1;
            }
        }
        drop(submit);
        next
    });
    cancellation.check()?;
    let mut listings: Vec<_> = finished.try_iter().collect();
    listings.sort_unstable_by_key(|(position, _)| *position);
    let listings = listings.into_iter().map(|(_, listing)| listing).collect();
    Ok(Page { listings, next })
}

fn parse(document: &Html, url: &Url, criteria: &Criteria) -> Result<Page> {
    let selector = |pattern| Selector::parse(pattern).expect("valid DesktopHut selector");
    if document.select(&selector("h1")).any(|heading| {
        heading
            .text()
            .collect::<String>()
            .trim()
            .starts_with("No matches for")
    }) {
        return Ok(Page {
            listings: Vec::new(),
            next: None,
        });
    }
    let cards = selector("a.wallpaper-card[href]");
    let name = selector(".card-title");
    let dimensions = selector(".badge-res");
    let image = selector("img[src]");
    let video = selector("video.card-preview-video");
    let mut cards = document.select(&cards).peekable();
    ensure!(
        cards.peek().is_some(),
        "Unrecognized DesktopHut search results"
    );
    let listings = cards
        .filter_map(|card| {
            let video = card.select(&video).next();
            if video.is_none()
                && !card
                    .attr("aria-label")
                    .is_some_and(|label| label.ends_with(" animated wallpaper"))
            {
                return None;
            }
            let resolution = card.select(&dimensions).next()?.text().collect::<String>();
            let (width, height) = resolution.trim().split_once(['x', '×'])?;
            let title = card.select(&name).next()?.text().collect::<String>();
            let title = title
                .trim()
                .strip_suffix(" Live Wallpaper")
                .unwrap_or(title.trim());
            if title.is_empty() {
                return None;
            }
            let page = url.join(card.attr("href")?).ok()?;
            let thumbnail = url.join(card.select(&image).next()?.attr("src")?).ok()?;
            let thumbnail = Source::DesktopHut.thumbnail(thumbnail.as_str())?;
            let preview = video
                .and_then(|video| video.attr("data-src").or_else(|| video.attr("src")))
                .and_then(|raw| url.join(raw).ok())
                .map(Into::into);
            Listing {
                title: title.to_owned(),
                width: width.trim().parse().ok()?,
                height: height.trim().parse().ok()?,
                source: Source::DesktopHut,
                page: page.to_string(),
                download: page.into(),
                original_mp4: None,
                thumbnail: Some(thumbnail.into()),
                preview,
                trending: url.path() == POPULAR_PATH && url.query().is_none(),
                format: Format::Mp4,
            }
            .validate()
            .filter(|item| criteria.matches_filters(item))
        })
        .collect();
    let next = document
        .select(&selector("a[rel=next][href]"))
        .filter_map(|element| url.join(element.attr("href")?).ok())
        .find(|next| next != url && search_page(next))
        .map(|url| SearchCursor::DesktopHut(Some(url)));
    Ok(Page { listings, next })
}

fn preview_link(document: &Html, page: &Url) -> Option<String> {
    let source = Selector::parse("video#nativePlayer[src], video#nativePlayer source[src]")
        .expect("valid DesktopHut selector");
    document
        .select(&source)
        .filter_map(|element| page.join(element.attr("src")?).ok())
        .find_map(|url| preview::video_url(Source::DesktopHut, url.as_str()))
        .map(Into::into)
}

fn download_link(document: &Html, page: &Url) -> Option<Url> {
    let original =
        Selector::parse("a#primaryDownloadBtn[download][href]").expect("valid DesktopHut selector");
    let raw = document.select(&original).next()?.attr("href")?;
    let url = page.join(raw).ok()?;
    original_mp4(url.as_str())
}

pub(super) fn original_mp4(raw: &str) -> Option<Url> {
    let mut url = trusted_url(raw, VIDEOS)?;
    let name = url.path().strip_prefix("/files/")?.strip_suffix(".mp4")?;
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return None;
    }
    url.set_query(None);
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_omits_interactive_scenes_and_external_images() {
        let html = Html::parse_document(
            r#"
          <a class=wallpaper-card href=/night-forest>
            <img src=/images/forest.webp><video class=card-preview-video></video>
            <h3 class=card-title>Night Forest Live Wallpaper</h3><span class=badge-res>3840x2160</span>
          </a>
          <a class=wallpaper-card href=/interactive>
            <img src=/images/interactive.webp><video class=card-preview-video></video>
            <h3 class=card-title>Interactive scene</h3><span class=badge-res>Scene</span>
          </a>
          <a class=wallpaper-card href=/unexpected-thumbnail>
            <img src=https://example.org/images/forest.webp><video class=card-preview-video></video>
            <h3 class=card-title>Unexpected image</h3><span class=badge-res>1920x1080</span>
          </a>
          <a rel=next href=/search/forest?page=2>Next</a>
        "#,
        );
        let url = Url::parse("https://www.desktophut.com/search/forest").unwrap();
        let page = parse(&html, &url, &Criteria::default()).unwrap();
        assert_eq!(page.listings.len(), 1);
        assert_eq!(page.listings[0].title, "Night Forest");
        assert_eq!(page.listings[0].width, 3840);
        assert!(
            matches!(page.next, Some(SearchCursor::DesktopHut(Some(url))) if url.query() == Some("page=2"))
        );
    }

    #[test]
    fn search_requires_a_first_party_original_even_with_an_mp4_preview() {
        let cards = ["preview-only", "night-forest", "youtube", "scene", "city"]
            .into_iter()
            .map(|slug| {
                format!(
                    r#"<a class=wallpaper-card href="/{slug}">
                        <img src="/images/{slug}.webp">
                        <video class=card-preview-video></video>
                        <h3 class=card-title>{slug}</h3><span class=badge-res>1920x1080</span>
                    </a>"#
                )
            })
            .collect::<String>();
        let url = Url::parse("https://www.desktophut.com/popular-live-wallpapers").unwrap();
        let mut page = parse(&Html::parse_document(&cards), &url, &Criteria::default()).unwrap();
        let more = Page {
            listings: page.listings.split_off(3),
            next: None,
        };
        let checked = confirm_downloads([page, more], &Cancellation::default(), |url| {
            let detail = match url.path() {
                "/preview-only" => "<video id=nativePlayer src=/previews/promo.mp4></video>",
                "/night-forest" => r#"
                    <video id=nativePlayer><source src="/previews/forest.mp4"></video>
                    <a id=primaryDownloadBtn download href="https://media.desktophut.com/files/forest.mp4">Download</a>
                "#,
                "/youtube" => r#"
                    <iframe src="https://www.youtube.com/embed/clip"></iframe>
                    <a id=primaryDownloadBtn download href="https://www.youtube.com/watch?v=clip">Download</a>
                "#,
                "/scene" => r#"
                    <video id=nativePlayer src="/previews/scene.mp4"></video>
                    <a id=primaryDownloadBtn download href="/files/scene.zip">Download</a>
                "#,
                "/city" => "<a id=primaryDownloadBtn download href=/files/city.mp4>Download</a>",
                _ => panic!("Unexpected wallpaper page"),
            };
            Ok(Html::parse_document(detail))
        })
        .unwrap();
        let titles: Vec<_> = checked
            .listings
            .iter()
            .map(|listing| listing.title.as_str())
            .collect();
        assert_eq!(titles, ["night-forest", "city"]);
        let forest = &checked.listings[0];
        assert_eq!(forest.download, "https://www.desktophut.com/night-forest");
        assert_eq!(
            forest.original_mp4.as_ref().unwrap().host_str(),
            Some("media.desktophut.com")
        );
        assert_eq!(
            forest.preview.as_deref(),
            Some("https://www.desktophut.com/previews/forest.mp4")
        );
    }

    #[test]
    fn original_download_must_be_a_first_party_mp4() {
        let page = Url::parse("https://www.desktophut.com/night-forest").unwrap();
        for href in [
            "/files/forest.exe",
            "/files/forest.zip",
            "/files/%2fdownload%2fforest.mp4",
            "/download/forest.mp4",
            "https://example.org/files/forest.mp4",
            "https://www.desktophut.com.example.org/files/forest.mp4",
        ] {
            let html = Html::parse_document(&format!(
                "<a id=primaryDownloadBtn download href='{href}'>Download</a>"
            ));
            assert!(download_link(&html, &page).is_none());
        }
        let html = Html::parse_document(
            "<a id=primaryDownloadBtn download href='/files/forest.mp4'>Download</a>",
        );
        assert_eq!(
            download_link(&html, &page).unwrap().path(),
            "/files/forest.mp4"
        );
    }
}
