use super::{Criteria, Format, Listing, Page, SearchCursor, Source, get, preview, trusted_url};
use crate::http::{Cancellation, HttpRequest};
use anyhow::{Context, Result, bail, ensure};
use scraper::{Html, Selector};
use url::Url;
use windows_sys::Win32::Networking::WinHttp::WINHTTP_QUERY_LOCATION;

const ROOT: &str = "https://motionbgs.com/";
const DOWNLOAD: &str = "https://motionbgs.com/dl/";
pub(super) const THUMBNAIL: &str = "https://motionbgs.com/i/c/";
pub(super) const PREVIEW: &str = "https://motionbgs.com/media/";

pub(super) fn wallpaper_url(raw: &str) -> Option<Url> {
    let url = trusted_url(raw, &[ROOT])?;
    let name = url.path().trim_matches('/');
    (!name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'))
        .then_some(url)
}

pub(super) fn download_url(raw: &str) -> Option<Url> {
    let url = trusted_url(raw, &[DOWNLOAD])?;
    let (quality, id) = url
        .path()
        .strip_prefix("/dl/")?
        .strip_suffix('/')?
        .split_once('/')?;
    (matches!(quality, "hd" | "4k") && media_id(id)).then_some(url)
}

fn media_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())
}

fn search_page(url: &Url) -> bool {
    let path = url.path();
    url.as_str().starts_with(ROOT)
        && (matches!(path, "/" | "/search")
            || path.starts_with("/tag:")
            || media_id(path.trim_matches('/')))
}

pub(super) fn search(
    criteria: &Criteria,
    from: Option<&Url>,
    cancellation: &Cancellation,
) -> Result<Page> {
    let mut url = match from {
        Some(url) => url.clone(),
        None if criteria.text.is_empty() => Url::parse(ROOT)?,
        None => {
            let mut url = Url::parse(ROOT)?.join("search")?;
            url.query_pairs_mut().append_pair("q", &criteria.text);
            url
        }
    };
    // Public searches for a named category redirect to that category's results page.
    for _ in 0..3 {
        ensure!(search_page(&url), "Unexpected MotionBGs search URL");
        let response = HttpRequest::get_cancellable(url.as_str(), 12_000, cancellation)?;
        match response.status {
            200 => {
                let bytes = response.read_all(512 * 1024)?;
                cancellation.check()?;
                let html = String::from_utf8(bytes).context("Invalid MotionBGs response")?;
                return parse(&html, &url, criteria);
            }
            301 | 302 | 303 | 307 | 308 => {
                let location = response
                    .header_string(WINHTTP_QUERY_LOCATION)
                    .context("Missing MotionBGs redirect URL")?;
                url = url.join(&location)?;
            }
            status => bail!("MotionBGs returned HTTP {status}"),
        }
    }
    bail!("Too many MotionBGs redirects")
}

fn parse(html: &str, url: &Url, criteria: &Criteria) -> Result<Page> {
    let document = Html::parse_document(html);
    let selector = |pattern| Selector::parse(pattern).expect("valid MotionBGs selector");
    let title = selector("span.ttl");
    let quality = selector("span.frm");
    let preview = selector("picture img[src]");
    let featured = if url.path() == "/" {
        document
            .select(&selector("main > section:first-of-type > .tmb"))
            .next()
    } else {
        None
    };
    let listings: Vec<_> = document
        .select(&selector("a[title][href]"))
        .filter_map(|card| {
            let title = card.select(&title).next()?.text().collect::<String>();
            let quality = card.select(&quality).next()?.text().collect::<String>();
            let (width, height, kind) = match quality.trim() {
                "4K" => (3840, 2160, "4k"),
                "HD" => (1920, 1080, "hd"),
                _ => return None,
            };
            let page = url.join(card.attr("href")?).ok()?;
            let preview = url.join(card.select(&preview).next()?.attr("src")?).ok()?;
            let preview = Source::MotionBgs.thumbnail(preview.as_str())?;
            let (_, file) = preview.path().split_once("/media/")?;
            let (id, _) = file.split_once('/')?;
            let title = title.trim();
            if title.is_empty() {
                return None;
            }
            Listing {
                source: Source::MotionBgs,
                original_mp4: None,
                title: title.to_owned(),
                width,
                height,
                thumbnail: Some(preview.to_string()),
                preview: None,
                page: page.into(),
                download: format!("{DOWNLOAD}{kind}/{id}/"),
                trending: featured
                    .is_some_and(|grid| card.ancestors().any(|node| node.id() == grid.id())),
                format: Format::Mp4,
                keywords: String::new(),
            }
            .validate()
        })
        .collect();
    ensure!(
        !listings.is_empty()
            || document.select(&selector("h1")).any(|heading| {
                heading
                    .text()
                    .collect::<String>()
                    .trim()
                    .starts_with("No Results Found")
            }),
        "Unrecognized MotionBGs search results"
    );
    let next = document
        .select(&selector("link[rel=next][href]"))
        .filter_map(|element| url.join(element.attr("href")?).ok())
        .find(|next| next != url && search_page(next))
        .map(|next| SearchCursor::MotionBgs(Some(next)));
    Ok(Page {
        listings: listings
            .into_iter()
            .filter(|item| criteria.matches_filters(item))
            .collect(),
        next,
    })
}

pub(super) fn resolve_preview(page: &Url, cancellation: &Cancellation) -> Result<Url> {
    ensure!(
        wallpaper_url(page.as_str()).is_some(),
        "Unexpected MotionBGs wallpaper URL"
    );
    let bytes = get(page, 10_000, Some(cancellation))?.read_all(512 * 1024)?;
    cancellation.check()?;
    let document = Html::parse_document(std::str::from_utf8(&bytes)?);
    let source =
        Selector::parse("video[muted][poster] source[src]").expect("valid MotionBGs selector");
    document
        .select(&source)
        .filter_map(|element| page.join(element.attr("src")?).ok())
        .find_map(|url| preview::video_url(Source::MotionBgs, url.as_str()))
        .context("MotionBGs has no video preview for this wallpaper")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gallery::{MediaFilter, Resolution};

    #[test]
    fn parses_motionbgs_cards_filters_and_pagination() {
        let html = r#"
            <link rel=next href=/tag:rain/2/>
            <a title="Rain &amp; Lightning" href=/rain-lightning>
                <picture><img src=/i/c/364x205/media/1234/rain.jpg></picture>
                <span class=ttl>Rain &amp; Lightning</span><span class=frm> 4K </span>
            </a>
            <a title="Night rain" href=/night-rain>
                <picture><img src=/i/c/364x205/media/5678/night.jpg></picture>
                <span class=ttl>Night Rain</span><span class=frm> HD </span>
            </a>
            <a title="External" href=https://example.org/external>
                <picture><img src=/i/c/364x205/media/9999/test.jpg></picture>
                <span class=ttl>External</span><span class=frm>4K</span>
            </a>
        "#;
        let url = Url::parse("https://motionbgs.com/tag:rain/").unwrap();
        let mut criteria = Criteria::default();
        let page = parse(html, &url, &criteria).unwrap();
        assert_eq!(page.listings.len(), 2);
        assert_eq!(page.listings[0].title, "Rain & Lightning");
        assert_eq!(
            page.listings[0].download,
            "https://motionbgs.com/dl/4k/1234/"
        );
        assert_eq!(
            page.listings[1].download,
            "https://motionbgs.com/dl/hd/5678/"
        );
        assert!(
            matches!(page.next, Some(SearchCursor::MotionBgs(Some(next))) if next.path() == "/tag:rain/2/")
        );
        criteria.resolution = Resolution::Uhd;
        assert_eq!(parse(html, &url, &criteria).unwrap().listings.len(), 1);
        criteria.media = MediaFilter::Still;
        assert!(parse(html, &url, &criteria).unwrap().listings.is_empty());
        assert!(!SearchCursor::MotionBgs(None).supports(&criteria));
    }

    #[test]
    fn distinguishes_empty_searches_from_unrecognized_responses() {
        let url = Url::parse("https://motionbgs.com/search?q=missing").unwrap();
        assert!(
            parse(
                "<h1>No Results Found for 'missing'</h1>",
                &url,
                &Criteria::default()
            )
            .unwrap()
            .listings
            .is_empty()
        );
        assert!(parse("<h1>Service interrupted</h1>", &url, &Criteria::default()).is_err());
    }
}
