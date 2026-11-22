use super::{Criteria, Format, Listing, Page, Resolution, SearchCursor, Source, get, trusted_url};
use crate::http::Cancellation;
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use url::Url;

const API: &str = "https://wallhaven.cc/api/v1/search";
const WALLPAPER: &str = "https://wallhaven.cc/w/";
const ORIGINAL: &str = "https://w.wallhaven.cc/full/";
const THUMBNAIL: &str = "https://th.wallhaven.cc/lg/";

fn valid_id(id: &str) -> bool {
    id.len() == 6
        && id
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

pub(super) fn page_url(raw: &str) -> Option<Url> {
    let url = trusted_url(raw, &[WALLPAPER])?;
    valid_id(url.path().strip_prefix("/w/")?).then_some(url)
}

pub(super) fn download_url(raw: &str) -> Option<Url> {
    let url = trusted_url(raw, &[ORIGINAL])?;
    let (folder, file) = url.path().strip_prefix("/full/")?.split_once('/')?;
    let (id, extension) = file.strip_prefix("wallhaven-")?.split_once('.')?;
    (valid_id(id) && folder == &id[..2] && matches!(extension, "jpg" | "png")).then_some(url)
}

pub(super) fn thumbnail_url(raw: &str) -> Option<Url> {
    let url = trusted_url(raw, &[THUMBNAIL])?;
    let (folder, name) = url.path().strip_prefix("/lg/")?.split_once('/')?;
    let id = name.strip_suffix(".jpg")?;
    (valid_id(id) && folder == &id[..2]).then_some(url)
}

fn search_url(criteria: &Criteria, page: u32) -> Result<Url> {
    let mut url = Url::parse(API)?;
    let sorting = if criteria.text.is_empty() {
        "hot"
    } else {
        "relevance"
    };
    let mut query = url.query_pairs_mut();
    query
        .append_pair("sorting", sorting)
        .append_pair("order", "desc")
        .append_pair("purity", "100")
        .append_pair("categories", "111")
        .append_pair("page", &page.to_string());
    if !criteria.text.is_empty() {
        query.append_pair("q", &criteria.text);
    }
    if criteria.resolution != Resolution::Any {
        let (width, height) = criteria.resolution.minimum();
        query.append_pair("atleast", &format!("{width}x{height}"));
    }
    drop(query);
    Ok(url)
}

pub(super) fn search(criteria: &Criteria, page: u32, cancellation: &Cancellation) -> Result<Page> {
    let url = search_url(criteria, page)?;
    let bytes = get(&url, 12_000, Some(cancellation))?.read_all(256 * 1024)?;
    cancellation.check()?;
    parse(&bytes, criteria, page)
}

#[derive(Deserialize)]
struct SearchResponse {
    data: Vec<serde_json::Value>,
    meta: Pagination,
}

#[derive(Deserialize)]
struct Pagination {
    current_page: u32,
    last_page: u32,
}

#[derive(Deserialize)]
struct Wallpaper {
    id: String,
    purity: String,
    path: String,
    file_type: String,
    dimension_x: u32,
    dimension_y: u32,
    thumbs: Thumbnails,
}

#[derive(Deserialize)]
struct Thumbnails {
    large: String,
}

impl Wallpaper {
    fn listing(self) -> Option<Listing> {
        let format = Format::from_mime(&self.file_type)?;
        if self.purity != "sfw" || !valid_id(&self.id) || format.animated() {
            return None;
        }
        let id = &self.id;
        let folder = &id[..2];
        if self.path != format!("{ORIGINAL}{folder}/wallhaven-{id}.{}", format.extension())
            || self.thumbs.large != format!("{THUMBNAIL}{folder}/{id}.jpg")
        {
            return None;
        }
        Listing {
            title: format!("Wallpaper {id}"),
            source: Source::Wallhaven,
            width: self.dimension_x,
            height: self.dimension_y,
            thumbnail: Some(self.thumbs.large),
            preview: None,
            page: format!("{WALLPAPER}{id}"),
            download: self.path,
            original_mp4: None,
            trending: false,
            format,
        }
        .validate()
    }
}

fn parse(bytes: &[u8], criteria: &Criteria, requested_page: u32) -> Result<Page> {
    let response: SearchResponse =
        serde_json::from_slice(bytes).context("Invalid Wallhaven search results")?;
    ensure!(
        response.meta.current_page == requested_page,
        "Wallhaven returned a different page"
    );
    let listings = response
        .data
        .into_iter()
        .filter_map(|item| serde_json::from_value::<Wallpaper>(item).ok()?.listing())
        .filter(|item| criteria.matches_filters(item))
        .collect();
    let next = (requested_page < response.meta.last_page)
        .then(|| SearchCursor::Wallhaven(requested_page + 1));
    Ok(Page { listings, next })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gallery::MediaFilter;
    use serde_json::json;

    #[test]
    fn uses_hot_for_browsing_and_keeps_wallhaven_relevance_order_for_search() {
        let criteria = Criteria {
            text: "space".into(),
            media: MediaFilter::Still,
            resolution: Resolution::Uhd,
        };
        let url = search_url(&criteria, 2).unwrap();
        let params: std::collections::HashMap<_, _> = url.query_pairs().collect();
        for (key, expected) in [
            ("sorting", "relevance"),
            ("purity", "100"),
            ("q", "space"),
            ("atleast", "3840x2160"),
            ("page", "2"),
        ] {
            assert_eq!(params[key], expected);
        }
        let browse = search_url(&Criteria::default(), 1).unwrap();
        let params: std::collections::HashMap<_, _> = browse.query_pairs().collect();
        assert_eq!(params["sorting"], "hot");
        assert_eq!(params["purity"], "100");
        assert!(!params.contains_key("q"));
        assert!(SearchCursor::Wallhaven(1).supports(&criteria));
        assert!(!SearchCursor::Wallhaven(1).supports(&Criteria {
            media: MediaFilter::Animated,
            ..criteria.clone()
        }));

        let wallpaper = |id: &str, purity, width, height| {
            let folder = &id[..2];
            json!({
                "id": id, "purity": purity, "file_type": "image/jpeg",
                "dimension_x": width, "dimension_y": height,
                "path": format!("{ORIGINAL}{folder}/wallhaven-{id}.jpg"),
                "thumbs": {"large": format!("{THUMBNAIL}{folder}/{id}.jpg")}
            })
        };
        let mut external = wallpaper("ab0004", "sfw", 3840, 2160);
        external["path"] = json!("https://example.org/wallpaper.jpg");
        let mut wrong_format = wallpaper("ab0005", "sfw", 3840, 2160);
        wrong_format["file_type"] = json!("image/png");
        let response = json!({
            "meta": {"current_page": 2, "last_page": 3},
            "data": [
                wallpaper("ab0001", "sfw", 2160, 3840),
                wallpaper("ab0002", "sketchy", 3840, 2160),
                wallpaper("ab0003", "sfw", 1920, 1080),
                external,
                wrong_format,
                {"id": "incomplete"},
                wallpaper("ab0006", "sfw", 3840, 2160)
            ]
        })
        .to_string();
        let page = parse(response.as_bytes(), &criteria, 2).unwrap();
        let titles: Vec<_> = page
            .listings
            .iter()
            .map(|item| item.title.as_str())
            .collect();
        assert_eq!(titles, ["Wallpaper ab0001", "Wallpaper ab0006"]);
        assert!(page.listings.iter().all(|item| !item.animated()));
        assert!(matches!(page.next, Some(SearchCursor::Wallhaven(3))));
        assert!(parse(response.as_bytes(), &criteria, 3).is_err());
    }
}
