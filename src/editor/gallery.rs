use super::{
    ACCENT, FIELD, INK, LINE, MUTED, TEXT, heading_font, icons::Icon, popup, popup::Hint,
    preferences::hairline, texture,
};
use crate::gallery::{
    self, Catalog, Criteria, Listing, MediaFilter, Page, PreparedWallpaper, PreviewFrames, Removal,
    Resolution, SavedWallpaper, SearchCursor, Source,
};
use crate::http::Cancellation;
use crate::layout::Display;
use anyhow::{Result, anyhow};
use eframe::egui::*;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError, channel},
    task::Poll,
    time::{Duration, SystemTime},
};

const GAP: f32 = 12.;
const MORE_PAGES_PER_SOURCE: u8 = 2;
const REVEAL_ROWS: f32 = 3.;
const PREVIEW_DELAY: f64 = 0.65;
const SEARCH_DELAY: f64 = 1.0;
const CARD_FADE: f64 = 0.18;
const FILTER_WIDTH: f32 = 246.;
const FILTER_PADDING: Margin = Margin::symmetric(16, 13);
const CLOSE_AFTER_APPLY: f64 = 1.;
const FILTERS_POPUP: &str = "gallery filters";
pub(super) const DIALOG_ID: &str = "gallery dialog";

type Job<T> = Receiver<Result<T>>;

fn background<T: Send + 'static>(
    ctx: &Context,
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Job<T> {
    let (tx, rx) = channel();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        if tx.send(work()).is_ok() {
            ctx.request_repaint();
        }
    });
    rx
}

fn completed<T>(job: &Job<T>) -> Option<Result<T>> {
    match job.try_recv() {
        Ok(result) => Some(result),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Err(anyhow!("Gallery request stopped"))),
    }
}

#[derive(Default)]
struct Thumbnails {
    browsing: Option<Cancellation>,
    loaded: VecDeque<(String, Option<TextureHandle>)>,
    pending: Vec<(String, Job<image::RgbaImage>)>,
    animated: VecDeque<(String, Option<AnimatedPreview>)>,
    pending_video: Vec<(String, Job<PreviewFrames>)>,
    visible_slots: usize,
    published_slots: usize,
    preload_slots: usize,
    appeared: HashMap<CardKey, f64>,
}

struct AnimatedPreview {
    frames: PreviewFrames,
    texture: TextureHandle,
    current_frame: usize,
}

impl AnimatedPreview {
    fn new(ctx: &Context, name: &str, frames: PreviewFrames) -> Self {
        let texture = ctx.load_texture(
            name,
            ColorImage::from_rgb(PreviewFrames::SIZE, frames.frame(0).1),
            TextureOptions::LINEAR,
        );
        Self {
            frames,
            texture,
            current_frame: 0,
        }
    }

    fn texture(&mut self, ctx: &Context) -> TextureHandle {
        let elapsed_frames = ctx.input(|input| input.time) * PreviewFrames::FPS;
        let (index, frame) = self.frames.frame(elapsed_frames as usize);
        if index != self.current_frame {
            self.texture.set(
                ColorImage::from_rgb(PreviewFrames::SIZE, frame),
                TextureOptions::LINEAR,
            );
            self.current_frame = index;
        }
        ctx.request_repaint_after(Duration::from_secs_f64(
            (1. - elapsed_frames.fract()) / PreviewFrames::FPS,
        ));
        self.texture.clone()
    }
}

impl Thumbnails {
    fn still_limit(&self) -> usize {
        self.visible_slots.max(64) + self.published_slots + self.preload_slots
    }

    fn poll(&mut self, ctx: &Context) {
        let still_limit = self.still_limit();
        let video_limit = self.visible_slots.max(12);
        self.pending.retain(|(url, job)| {
            let Some(result) = completed(job) else {
                return true;
            };
            self.loaded
                .push_back((url.clone(), result.ok().map(|image| texture(ctx, &image))));
            if self.loaded.len() > still_limit {
                self.loaded.pop_front();
            }
            false
        });
        self.pending_video.retain(|(key, job)| {
            let Some(result) = completed(job) else {
                return true;
            };
            self.animated.push_back((
                key.clone(),
                result
                    .ok()
                    .map(|frames| AnimatedPreview::new(ctx, key, frames)),
            ));
            if self.animated.len() > video_limit {
                self.animated.pop_front();
            }
            false
        });
    }

    fn reserve_visible_slots(&mut self, slots: usize) {
        self.visible_slots = slots;
        let still_limit = self.still_limit();
        self.animated
            .drain(..self.animated.len().saturating_sub(slots.max(12)));
        self.loaded
            .drain(..self.loaded.len().saturating_sub(still_limit));
    }

    fn still(&mut self, ctx: &Context, card: Card<'_>) -> Option<TextureHandle> {
        match self.still_status(ctx, card, 2) {
            Poll::Ready(image) => image,
            Poll::Pending => None,
        }
    }

    fn preload<'a>(
        &mut self,
        ctx: &Context,
        cards: impl Iterator<Item = Card<'a>> + Clone,
    ) -> bool {
        self.preload_slots = cards.clone().count();
        let mut ready = true;
        for card in cards {
            ready &= self.still_status(ctx, card, 8).is_ready();
        }
        ready
    }

    fn still_status(
        &mut self,
        ctx: &Context,
        card: Card<'_>,
        workers: usize,
    ) -> Poll<Option<TextureHandle>> {
        let key = match card {
            Card::Online(listing) => listing.thumbnail.as_deref().map(Cow::Borrowed),
            Card::Saved(saved, _) => Some(saved.media.path.to_string_lossy()),
        };
        let Some(key) = key.as_deref() else {
            return Poll::Ready(None);
        };
        if let Some((_, image)) = self.loaded.iter().find(|(loaded, _)| loaded == key) {
            return Poll::Ready(image.clone());
        }
        let Some(cancellation) = self.browsing.clone() else {
            return Poll::Ready(None);
        };
        if self.pending.len() < workers && !self.pending.iter().any(|(pending, _)| pending == key) {
            let job = match card {
                Card::Online(listing) => {
                    let source = listing.source;
                    let raw = key.to_owned();
                    background(ctx, move || gallery::thumbnail(source, &raw, &cancellation))
                }
                Card::Saved(saved, _) => {
                    let media = saved.media.clone();
                    background(ctx, move || {
                        cancellation.check()?;
                        Ok(
                            image::DynamicImage::ImageRgba8(gallery::saved_thumbnail(&media)?)
                                .thumbnail(580, 400)
                                .into_rgba8(),
                        )
                    })
                }
            };
            self.pending.push((key.to_owned(), job));
        }
        Poll::Pending
    }

    fn opacity(&mut self, ctx: &Context, card: Card<'_>) -> f32 {
        let now = ctx.input(|input| input.time);
        let started = *self.appeared.entry(card.order_key()).or_insert(now);
        let progress = ((now - started) / CARD_FADE) as f32;
        if progress < 1. {
            ctx.request_repaint();
        }
        0.2 + 0.8 * emath::easing::quadratic_out(progress.clamp(0., 1.))
    }

    fn animated(&mut self, ctx: &Context, card: Card<'_>, animate: bool) -> Option<TextureHandle> {
        let key = match card {
            Card::Online(listing) => listing.motion_preview_key()?,
            Card::Saved(item, _) if item.animated => item.media.path.to_str()?,
            Card::Saved(..) => return None,
        };
        if let Some(index) = self.animated.iter().position(|(loaded, _)| loaded == key) {
            let (key, mut preview) = self.animated.remove(index).expect("cached preview");
            let texture = preview.as_mut().map(|preview| {
                if animate {
                    preview.texture(ctx)
                } else {
                    preview.texture.clone()
                }
            });
            self.animated.push_back((key, preview));
            return texture;
        }
        if animate
            && self.pending_video.len() < 2
            && !self.pending_video.iter().any(|(url, _)| url == key)
            && let Some(cancellation) = self.browsing.clone()
        {
            let job = match card {
                Card::Online(listing) => {
                    let listing = listing.clone();
                    background(ctx, move || {
                        gallery::motion_preview(&listing, &cancellation)
                    })
                }
                Card::Saved(saved, _) => {
                    let path = saved.media.path.clone();
                    background(ctx, move || {
                        cancellation.check()?;
                        gallery::local_motion_preview(&path)
                    })
                }
            };
            self.pending_video.push((key.to_owned(), job));
        }
        None
    }

    fn forget(&mut self, paths: &[PathBuf]) {
        for path in paths {
            let key = path.to_string_lossy();
            self.loaded.retain(|(loaded, _)| loaded != key.as_ref());
            self.animated.retain(|(loaded, _)| loaded != key.as_ref());
            self.pending.retain(|(pending, _)| pending != key.as_ref());
            self.pending_video
                .retain(|(pending, _)| pending != key.as_ref());
        }
    }
}

#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub(super) enum CardKey {
    Online(String),
    Saved(PathBuf),
}

pub(super) enum Selection {
    Online(Listing),
    Saved(SavedWallpaper),
}

impl Selection {
    pub(super) fn key(&self) -> CardKey {
        match self {
            Self::Online(listing) => CardKey::Online(listing.download.clone()),
            Self::Saved(saved) => CardKey::Saved(saved.media.path.clone()),
        }
    }

    pub(super) fn prepare(self) -> Result<PreparedWallpaper> {
        match self {
            Self::Online(listing) => gallery::save_download(&listing),
            Self::Saved(saved) => gallery::prepare(saved),
        }
    }
}

#[derive(Clone, Copy)]
enum Card<'a> {
    Online(&'a Listing),
    Saved(&'a SavedWallpaper, Option<&'a Listing>),
}

impl<'a> Card<'a> {
    fn key(self) -> CardKey {
        match self {
            Self::Online(listing) => CardKey::Online(listing.download.clone()),
            Self::Saved(saved, _) => CardKey::Saved(saved.media.path.clone()),
        }
    }

    fn order_key(self) -> CardKey {
        match self {
            Self::Saved(saved, listing) => listing
                .map(|listing| &listing.download)
                .or(saved.origin.as_ref())
                .map_or_else(|| self.key(), |url| CardKey::Online(url.clone())),
            Self::Online(_) => self.key(),
        }
    }

    fn selection(self) -> Selection {
        match self {
            Self::Online(listing) => Selection::Online(listing.clone()),
            Self::Saved(saved, _) => Selection::Saved(saved.clone()),
        }
    }

    fn label(self) -> (&'a str, u32, u32) {
        match self {
            Self::Online(listing) => (&listing.title, listing.width, listing.height),
            Self::Saved(saved, _) => (&saved.title, saved.media.width, saved.media.height),
        }
    }

    fn metadata(self) -> String {
        let (_, width, height) = self.label();
        let (animated, source) = match self {
            Self::Online(listing) => (listing.animated(), Some(listing.source)),
            Self::Saved(saved, listing) => (
                saved.animated,
                listing
                    .map(|item| item.source)
                    .or_else(|| saved.origin.as_deref().and_then(Source::from_origin)),
            ),
        };
        let kind = if animated { "Animated" } else { "Still" };
        let source = source.map_or("Local", Source::name);
        format!("{width} × {height} · {kind} · {source}")
    }
}

#[derive(Clone, Copy)]
pub(super) struct View<'a> {
    pub displays: &'a [Display],
    pub selected: usize,
    pub multi: bool,
    pub downloading: &'a HashSet<&'a CardKey>,
    pub importing_local: bool,
}

impl View<'_> {
    fn selected_target(self) -> Option<String> {
        self.displays
            .get(self.selected)
            .filter(|_| self.multi)
            .map(|display| display.id.clone())
    }
}

pub(super) enum Action {
    None,
    Close,
    Apply(Selection, Option<String>),
    Delete(PathBuf),
}

struct RemoteSearch {
    initial: SearchCursor,
    results: Vec<Listing>,
    request: Option<(u64, Job<Page>)>,
    pages_left: u8,
    next: Option<SearchCursor>,
    visited: HashSet<SearchCursor>,
}

impl RemoteSearch {
    fn new(initial: SearchCursor) -> Self {
        Self {
            initial,
            results: Vec::new(),
            request: None,
            pages_left: 0,
            next: None,
            visited: HashSet::new(),
        }
    }

    fn reset(&mut self, criteria: &Criteria) {
        self.results.clear();
        self.visited.clear();
        self.next = self
            .initial
            .supports(criteria)
            .then(|| self.initial.clone());
        self.queue_pages(1);
    }

    fn queue_pages(&mut self, count: u8) {
        self.pages_left = if self.next.is_some() { count } else { 0 };
    }

    fn poll(&mut self, generation: u64) {
        let finished = self
            .request
            .as_ref()
            .and_then(|(started, job)| completed(job).map(|result| (*started, result)));
        let Some((started, result)) = finished else {
            return;
        };
        self.request = None;
        if started != generation {
            return;
        }
        self.visited.extend(self.next.take());
        let Ok(page) = result else {
            self.pages_left = 0;
            return;
        };
        self.next = page.next.filter(|page| !self.visited.contains(page));
        let fetched = self.initial.pages_per_fetch(self.pages_left);
        self.queue_pages(self.pages_left.saturating_sub(fetched));
        let mut seen: HashSet<_> = self
            .results
            .iter()
            .map(|item| item.download.clone())
            .collect();
        self.results.extend(
            page.listings
                .into_iter()
                .filter(|item| seen.insert(item.download.clone())),
        );
    }

    fn start_queued(
        &mut self,
        ctx: &Context,
        generation: u64,
        criteria: &Criteria,
        cancellation: &Cancellation,
    ) {
        if self.pages_left > 0
            && self.request.is_none()
            && let Some(cursor) = self.next.clone()
        {
            let criteria = criteria.clone();
            let cancellation = cancellation.clone();
            let pages = cursor.pages_per_fetch(self.pages_left);
            self.request = Some((
                generation,
                background(ctx, move || cursor.fetch(&criteria, pages, &cancellation)),
            ));
        }
    }

    fn loading(&self, generation: u64) -> bool {
        self.pages_left > 0
            || self
                .request
                .as_ref()
                .is_some_and(|(started, _)| *started == generation)
    }
}

enum SpanApply {
    Pending(CardKey),
    CloseAt(f64),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Batch {
    Initial,
    Next,
}

pub(super) struct Gallery {
    searches: Vec<RemoteSearch>,
    draft: String,
    search_at: Option<f64>,
    criteria: Criteria,
    generation: u64,
    batch: Option<Batch>,
    page_order: HashMap<CardKey, usize>,
    reset_scroll: bool,
    thumbnails: Thumbnails,
    downloaded: bool,
    saved: Catalog,
    revision: u64,
    scan: Option<(u64, Job<Catalog>)>,
    removal: Option<Job<Removal>>,
    span_apply: Option<SpanApply>,
    error: Option<String>,
    hovered: Option<(CardKey, Option<f64>)>,
    scrolling_until: f64,
}

impl Default for Gallery {
    fn default() -> Self {
        Self {
            searches: vec![
                RemoteSearch::new(SearchCursor::MotionBgs(None)),
                RemoteSearch::new(SearchCursor::DesktopHut(None)),
                RemoteSearch::new(SearchCursor::Wallhaven(1)),
            ],
            draft: String::new(),
            search_at: None,
            criteria: Criteria::default(),
            generation: 0,
            batch: None,
            page_order: HashMap::new(),
            reset_scroll: false,
            thumbnails: Thumbnails::default(),
            downloaded: false,
            saved: Catalog::default(),
            revision: 0,
            scan: None,
            removal: None,
            span_apply: None,
            error: None,
            hovered: None,
            scrolling_until: 0.,
        }
    }
}

impl Gallery {
    pub(super) fn refresh_saved(&mut self, ctx: &Context) {
        if self.scan.is_none() {
            self.scan = Some((self.revision, background(ctx, gallery::read_catalog)));
        }
    }

    pub(super) fn record_saved(&mut self, saved: SavedWallpaper) {
        self.revision += 1;
        self.saved.upsert(saved);
    }

    pub(super) fn omit_unavailable(&mut self, key: &CardKey) {
        if let CardKey::Online(download) = key {
            for search in &mut self.searches {
                search
                    .results
                    .retain(|listing| &listing.download != download);
            }
        }
        if matches!(&self.span_apply, Some(SpanApply::Pending(pending)) if pending == key) {
            self.span_apply = None;
        }
    }

    pub(super) fn finish_span_apply(&mut self, ctx: &Context, key: &CardKey) {
        if matches!(&self.span_apply, Some(SpanApply::Pending(pending)) if pending == key) {
            let close_at = ctx.input(|input| input.time) + CLOSE_AFTER_APPLY;
            self.span_apply = Some(SpanApply::CloseAt(close_at));
        }
    }

    pub(super) fn bytes(&self) -> u64 {
        self.saved.bytes
    }

    pub(super) fn is_saved_animated(&self, path: &std::path::Path) -> Option<bool> {
        self.saved
            .items
            .iter()
            .find(|item| item.media.path == path)
            .map(|item| item.animated)
    }

    pub(super) fn scanning(&self) -> bool {
        self.scan.is_some()
    }

    pub(super) fn removing(&self) -> bool {
        self.removal.is_some()
    }

    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub(super) fn clear_error(&mut self) {
        self.error = None;
    }

    pub(super) fn set_error(&mut self, error: String) {
        self.error = Some(error);
    }

    pub(super) fn remove(
        &mut self,
        ctx: &Context,
        path: Option<PathBuf>,
        after: Option<SystemTime>,
    ) {
        if !self.removing() {
            self.clear_error();
            self.removal = Some(background(ctx, move || {
                if let Some(modified) = after {
                    crate::host::wait_for_layout(modified);
                }
                match path {
                    Some(path) => gallery::remove(&path),
                    None => gallery::clear(),
                }
            }));
        }
    }

    pub(super) fn poll_storage(&mut self, ctx: &Context) -> Vec<PathBuf> {
        if let Some((revision, result)) = self
            .scan
            .as_ref()
            .and_then(|(revision, job)| completed(job).map(|result| (*revision, result)))
        {
            self.scan = None;
            if revision != self.revision {
                self.refresh_saved(ctx);
            } else if let Ok(saved) = result {
                self.saved = saved;
            } else if let Err(error) = result {
                self.set_error(format!("Could not read the Gallery. {error}"));
            }
        }
        let Some(result) = self.removal.as_ref().and_then(completed) else {
            return Vec::new();
        };
        self.removal = None;
        self.revision += 1;
        self.refresh_saved(ctx);
        match result {
            Ok(removed) => {
                self.thumbnails.forget(&removed.paths);
                self.saved.forget(&removed.paths);
                self.error = removed.error;
                removed.paths
            }
            Err(error) => {
                self.set_error(format!("Could not delete Gallery wallpapers. {error}"));
                Vec::new()
            }
        }
    }

    fn start_search(&mut self, criteria: Criteria) {
        if self.thumbnails.browsing.is_some() {
            self.stop_browsing();
            self.thumbnails.browsing = Some(Cancellation::default());
        }
        self.criteria = criteria;
        self.generation += 1;
        self.batch = (!self.downloaded).then_some(Batch::Initial);
        self.page_order.clear();
        self.thumbnails.appeared.clear();
        self.thumbnails.published_slots = 0;
        self.thumbnails.preload_slots = 0;
        for search in &mut self.searches {
            search.reset(&self.criteria);
            if self.downloaded {
                search.next = None;
                search.pages_left = 0;
            }
        }
        self.reset_scroll = true;
        self.hovered = None;
    }

    fn stop_browsing(&mut self) {
        if let Some(cancellation) = self.thumbnails.browsing.take() {
            cancellation.cancel();
        }
        self.thumbnails.pending.clear();
        self.thumbnails.pending_video.clear();
        for search in &mut self.searches {
            search.request = None;
        }
    }

    fn close(&mut self, ctx: &Context) -> Action {
        self.span_apply = None;
        self.search_at = None;
        self.stop_browsing();
        ctx.memory_mut(|memory| {
            if memory.is_popup_open(Id::new(FILTERS_POPUP)) {
                memory.close_popup();
            }
        });
        Action::Close
    }

    pub(super) fn show(
        &mut self,
        ctx: &Context,
        opening: bool,
        opacity: f32,
        view: View<'_>,
    ) -> Action {
        if opening {
            self.thumbnails.browsing = Some(Cancellation::default());
            self.refresh_saved(ctx);
            self.hovered = None;
            self.span_apply = None;
            self.search_at = None;
            self.thumbnails.loaded.retain(|(_, image)| image.is_some());
            self.thumbnails
                .animated
                .retain(|(_, preview)| preview.is_some());
            if self.generation == 0 {
                self.start_search(self.criteria.clone());
            }
        }
        let timed_close = if let Some(SpanApply::CloseAt(close_at)) = self.span_apply {
            let remaining = close_at - ctx.input(|input| input.time);
            if remaining > 0. {
                ctx.request_repaint_after(Duration::from_secs_f64(remaining));
            }
            remaining <= 0.
        } else {
            false
        };
        for search in &mut self.searches {
            search.poll(self.generation);
        }
        self.thumbnails.poll(ctx);
        let screen = ctx.screen_rect().size();
        let gutter = ((screen - vec2(760., 450.)) * vec2(0.16, 0.12) + vec2(16., 16.))
            .clamp(vec2(16., 16.), vec2(120., 86.));
        let outer = screen - 2. * gutter;
        let size = (outer - Vec2::splat(2.)).max(vec2(1., 1.));
        let dialog =
            popup::dialog(Id::new(DIALOG_ID), outer, Margin::ZERO, opacity).show(ctx, |ui| {
                ui.multiply_opacity(opacity);
                ui.set_width(size.x);
                ui.set_min_height(size.y);
                ui.spacing_mut().item_spacing = vec2(8., 0.);
                let bounds = Rect::from_min_size(ui.min_rect().min, size);
                let close = self.controls(ui, opening, bounds);
                hairline(ui);
                let chosen = Frame::new()
                    .inner_margin(Margin::symmetric(18, 14))
                    .show(ui, |ui| {
                        if let Some(error) = self.error() {
                            ui.add(Label::new(RichText::new(error).small().color(ACCENT)).wrap());
                            ui.add_space(8.);
                        }
                        let height = bounds.bottom() - ui.next_widget_position().y - 14.;
                        self.grid(ui, height.max(1.), view)
                    })
                    .inner;
                if close { Action::Close } else { chosen }
            });
        if timed_close || dialog.should_close() || matches!(dialog.inner, Action::Close) {
            return self.close(ctx);
        }
        if !view.multi
            && let Action::Apply(item, None) = &dialog.inner
        {
            self.span_apply = Some(SpanApply::Pending(item.key()));
        }
        if !self.downloaded
            && let Some(cancellation) = &self.thumbnails.browsing
        {
            for search in &mut self.searches {
                search.start_queued(ctx, self.generation, &self.criteria, cancellation);
            }
        }
        dialog.inner
    }

    fn controls(&mut self, ui: &mut Ui, opening: bool, bounds: Rect) -> bool {
        let mut criteria = self.criteria.clone();
        let mut downloaded = self.downloaded;
        let open = ui.memory(|m| m.is_popup_open(Id::new(FILTERS_POPUP)));
        let filtered = criteria.media != MediaFilter::All
            || criteria.resolution != Resolution::Any
            || downloaded;
        let ((submit, changed), filter, close) = popup::header(ui, "Gallery", |ui| {
            let room = ui.available_width() - 2. * (32. + ui.spacing().item_spacing.x);
            let width = room.min(420.);
            ui.add_space((room - width).max(0.));
            let search = self.search_field(ui, opening, width);
            let filter = Icon::Filter
                .button(ui, "Filter wallpapers", open || filtered)
                .hint("Filter wallpapers");
            let close = Icon::Close.button(ui, "Close gallery", false).clicked();
            (search, filter, close)
        });
        filters(ui, bounds, &filter, &mut criteria, &mut downloaded);
        let now = ui.input(|i| i.time);
        if changed {
            let pending = gallery::search_text(&self.draft);
            if self.draft.trim().is_empty() || pending == self.criteria.text {
                self.search_at = None;
            } else {
                self.search_at = Some(now + SEARCH_DELAY);
            }
        }
        let auto_submit = if let Some(at) = self.search_at {
            let remaining = at - now;
            if remaining > 0. {
                ui.ctx()
                    .request_repaint_after(Duration::from_secs_f64(remaining));
                false
            } else {
                self.search_at = None;
                true
            }
        } else {
            false
        };
        if submit || auto_submit || criteria != self.criteria || downloaded != self.downloaded {
            self.search_at = None;
            criteria.text = gallery::search_text(&self.draft);
            if criteria != self.criteria
                || downloaded != self.downloaded
                || (submit && self.batch != Some(Batch::Initial))
            {
                self.downloaded = downloaded;
                if downloaded {
                    self.refresh_saved(ui.ctx());
                }
                self.start_search(criteria);
            }
        } else if self.draft.trim().is_empty() && !self.criteria.text.is_empty() {
            self.search_at = None;
            criteria.text.clear();
            self.start_search(criteria);
        }
        close
    }

    fn search_field(&mut self, ui: &mut Ui, opening: bool, width: f32) -> (bool, bool) {
        Frame::new()
            .fill(INK)
            .stroke(Stroke::new(1.0_f32, LINE))
            .corner_radius(7)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    let field = ui.add_sized(
                        [((width - 34.).max(80.)), 34.],
                        TextEdit::singleline(&mut self.draft)
                            .hint_text("Search wallpapers")
                            .char_limit(120)
                            .vertical_align(Align::Center)
                            .frame(false)
                            .margin(Margin::symmetric(11, 5))
                            .min_size(vec2(0., 34.)),
                    );
                    field.widget_info(|| {
                        WidgetInfo::labeled(WidgetType::TextEdit, true, "Search wallpapers")
                    });
                    let changed = field.changed();
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                    if opening || enter {
                        field.request_focus();
                    }
                    (Icon::Search.button(ui, "Search", false).clicked() || enter, changed)
                })
                .inner
            })
            .inner
    }

    fn grid(&mut self, ui: &mut Ui, height: f32, view: View<'_>) -> Action {
        let mut items = cards(
            &self.searches,
            &self.saved,
            &self.criteria,
            self.downloaded,
            &self.page_order,
        );
        let batch_ready = if self.batch.is_some() {
            let fetching = self
                .searches
                .iter()
                .any(|search| search.loading(self.generation));
            let incoming = items.iter().copied().filter(|item| {
                !matches!(item, Card::Saved(_, None))
                    && !self.page_order.contains_key(&item.order_key())
            });
            let posters_ready = self.thumbnails.preload(ui.ctx(), incoming);
            !fetching && posters_ready
        } else {
            false
        };
        if self.batch == Some(Batch::Initial) && batch_ready {
            self.batch = None;
            self.thumbnails.published_slots = self.thumbnails.preload_slots;
        }
        let loading = self.batch.is_some() && !batch_ready
            || (self.downloaded && self.scanning() && items.is_empty());
        if loading && self.page_order.is_empty() {
            let message = if self.downloaded {
                "Loading downloaded wallpapers…"
            } else {
                "Loading popular wallpapers…"
            };
            loading_status(ui, height, message);
            return Action::None;
        }
        if self.batch == Some(Batch::Next) || !self.page_order.is_empty() {
            keep_page_order(
                &mut items,
                &self.page_order,
                self.batch == Some(Batch::Next),
            );
        }
        let has_more = !self.downloaded && self.searches.iter().any(|search| search.next.is_some());
        let removing = self.removing();
        let (now, scroll_input) = ui.input(|i| {
            (
                i.time,
                i.smooth_scroll_delta.y.abs() > 0.25
                    || (i.pointer.primary_down() && i.pointer.delta().length_sq() > 0.25),
            )
        });
        if scroll_input {
            self.scrolling_until = now + 0.16;
        }
        let moving = now < self.scrolling_until;
        if moving {
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(self.scrolling_until - now));
        }

        ui.spacing_mut().scroll = style::ScrollStyle {
            bar_width: 5.,
            handle_min_length: 28.,
            bar_inner_margin: 12.,
            bar_outer_margin: 1.,
            ..style::ScrollStyle::solid()
        };
        ui.visuals_mut().extreme_bg_color = INK;
        ui.spacing_mut().item_spacing = Vec2::splat(GAP);
        let width = (ui.available_width() - ui.spacing().scroll.allocated_width()).max(1.);
        let columns = (((width + GAP) / (202. + GAP)) as usize).clamp(1, 6);
        let card_width = (width - (columns - 1) as f32 * GAP) / columns as f32;
        let card_size = vec2(card_width, card_width * 9. / 16.);
        let total_rows = items.len().div_ceil(columns);
        let visible_rows = (height / (card_size.y + GAP)).ceil() as usize + 2;
        self.thumbnails
            .reserve_visible_slots(total_rows.min(visible_rows) * columns);
        let footer = loading || (!has_more && self.batch.is_none() && items.is_empty());
        let mut scroll = ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(height)
            .id_salt("gallery results");
        if std::mem::take(&mut self.reset_scroll) {
            scroll = scroll.vertical_scroll_offset(0.);
        }

        let mut action = Action::None;
        let mut hovered = None;
        let scrolled = scroll.show_rows(
            ui,
            card_size.y,
            total_rows + usize::from(footer),
            |ui, rows| {
                ui.set_width(width);
                for row in rows {
                    if row == total_rows {
                        ui.vertical_centered(|ui| {
                            ui.add_space(8.);
                            if loading {
                                ui.add(Spinner::new().size(17.).color(ACCENT));
                            } else {
                                let label = if self.criteria.media == MediaFilter::None {
                                    "Select Still or Animated to show wallpapers"
                                } else if self.downloaded && self.saved.items.is_empty() {
                                    "No downloaded wallpapers"
                                } else {
                                    "No wallpapers match your search"
                                };
                                ui.label(RichText::new(label).color(MUTED));
                            }
                        });
                        continue;
                    }
                    let first = row * columns;
                    let group = &items[first..(first + columns).min(items.len())];
                    ui.horizontal_top(|ui| {
                        for item in group.iter().copied() {
                            ui.push_id(item.key(), |ui| {
                                let (response, card_action) = card(
                                    ui,
                                    card_size,
                                    item,
                                    &mut self.thumbnails,
                                    view,
                                    removing,
                                    !moving,
                                );
                                if response.hovered() && !response.context_menu_opened() {
                                    hovered = Some((item, response));
                                }
                                if !matches!(card_action, Action::None) {
                                    action = card_action;
                                }
                            });
                        }
                    });
                }
            },
        );
        show_preview_tooltip(
            ui.ctx(),
            &mut self.hovered,
            &mut self.thumbnails,
            hovered.filter(|_| !ui.memory(|m| m.is_popup_open(Id::new(FILTERS_POPUP)))),
            moving,
        );
        if self.batch == Some(Batch::Next) {
            let row_height = card_size.y + GAP;
            let remaining =
                scrolled.content_size.y - scrolled.state.offset.y - scrolled.inner_rect.height();
            if batch_ready && remaining <= REVEAL_ROWS * row_height {
                self.batch = None;
                self.thumbnails.published_slots = self.thumbnails.preload_slots;
                ui.ctx().request_repaint();
            }
        } else if self.batch.is_none() && has_more {
            // Prepare one batch while the current cards are still being browsed.
            self.page_order = items
                .iter()
                .enumerate()
                .map(|(index, card)| (card.order_key(), index))
                .collect();
            for search in &mut self.searches {
                search.queue_pages(MORE_PAGES_PER_SOURCE);
            }
            self.batch = Some(Batch::Next);
            ui.ctx().request_repaint();
        }
        action
    }
}

fn loading_status(ui: &mut Ui, height: f32, message: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let label = ui
        .painter()
        .layout_no_wrap(message.into(), FontId::proportional(13.), MUTED);
    let row = Rect::from_center_size(rect.center(), vec2(25. + label.size().x, 20.));
    let mut status = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
    );
    status.add(Spinner::new().size(17.).color(ACCENT));
    status.label(label);
}

fn keep_page_order(
    items: &mut Vec<Card<'_>>,
    order: &HashMap<CardKey, usize>,
    hide_incoming: bool,
) {
    let rank = |item: &Card<'_>| {
        order
            .get(&item.order_key())
            .or_else(|| order.get(&item.key()))
            .copied()
    };
    if hide_incoming {
        items.retain(|item| rank(item).is_some() || matches!(item, Card::Saved(_, None)));
    }
    items.sort_by_cached_key(|item| rank(item).unwrap_or(usize::MAX));
}

fn cards<'a>(
    searches: &'a [RemoteSearch],
    saved: &'a Catalog,
    criteria: &Criteria,
    downloaded: bool,
    order: &HashMap<CardKey, usize>,
) -> Vec<Card<'a>> {
    let local = saved
        .items
        .iter()
        .filter(|item| criteria.matches_saved(item));
    if downloaded {
        return local.map(|item| Card::Saved(item, None)).collect();
    }
    let unfiltered = criteria.text.is_empty()
        && criteria.media == MediaFilter::All
        && criteria.resolution == Resolution::Any;
    let mut paths = HashSet::new();
    let mut cards: Vec<_> = gallery_items(searches, order, unfiltered)
        .into_iter()
        .filter_map(|listing| {
            if let Some(item) = saved.find(listing) {
                paths
                    .insert(&item.media.path)
                    .then_some(Card::Saved(item, Some(listing)))
            } else {
                Some(Card::Online(listing))
            }
        })
        .collect();
    let insert_at = if unfiltered {
        cards.len()
    } else {
        cards
            .iter()
            .take_while(|card| match card {
                Card::Online(listing) | Card::Saved(_, Some(listing)) => listing.trending,
                Card::Saved(_, None) => false,
            })
            .count()
    };
    cards.splice(
        insert_at..insert_at,
        local
            .filter(|item| !paths.contains(&item.media.path))
            .map(|item| Card::Saved(item, None)),
    );
    cards
}

fn filters(ui: &Ui, bounds: Rect, icon: &Response, criteria: &mut Criteria, downloaded: &mut bool) {
    let popup_id = Id::new(FILTERS_POPUP);
    if icon.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    if !ui.memory(|m| m.is_popup_open(popup_id)) {
        return;
    }
    let frame = Frame::popup(ui.style())
        .inner_margin(FILTER_PADDING)
        .corner_radius(10);
    let area = Area::new(popup_id)
        .kind(UiKind::Popup)
        .order(Order::Foreground)
        .fixed_pos(icon.rect.right_bottom() + vec2(0., 10.))
        .pivot(Align2::RIGHT_TOP)
        .default_width(FILTER_WIDTH + frame.total_margin().sum().x)
        .constrain_to(bounds.shrink(12.))
        .fade_in(false);
    ui.ctx().set_sublayer(ui.layer_id(), area.layer());
    let opacity = ui.painter().opacity();
    let popup = area.show(ui.ctx(), |ui| {
        ui.multiply_opacity(opacity);
        frame.show(ui, |ui| {
            ui.set_width(FILTER_WIDTH);
            ui.spacing_mut().interact_size.y = 22.;
            ui.spacing_mut().item_spacing = vec2(16., 4.);
            ui.columns(2, |columns| {
                let mut still = criteria.media.includes(false);
                let mut animated = criteria.media.includes(true);
                let types = &mut columns[0];
                types.label(RichText::new("Type").font(heading_font(13.)).color(MUTED));
                types.checkbox(&mut still, "Still");
                types.checkbox(&mut animated, "Animated");
                types.checkbox(downloaded, "Downloaded");
                criteria.media = match (still, animated) {
                    (true, true) => MediaFilter::All,
                    (true, false) => MediaFilter::Still,
                    (false, true) => MediaFilter::Animated,
                    (false, false) => MediaFilter::None,
                };
                let resolutions = &mut columns[1];
                resolutions.label(
                    RichText::new("Resolution")
                        .font(heading_font(13.))
                        .color(MUTED),
                );
                for resolution in Resolution::OPTIONS {
                    resolutions.radio_value(
                        &mut criteria.resolution,
                        resolution,
                        resolution.label(),
                    );
                }
            });
        });
    });
    if ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        || (icon.clicked_elsewhere() && popup.response.clicked_elsewhere())
    {
        ui.memory_mut(|m| m.close_popup());
    }
}

fn gallery_items<'a>(
    searches: &'a [RemoteSearch],
    order: &HashMap<CardKey, usize>,
    alternate: bool,
) -> Vec<&'a Listing> {
    let mut items = Vec::new();
    let (trending, regular): (Vec<Vec<_>>, Vec<Vec<_>>) = searches
        .iter()
        .map(|search| {
            search
                .results
                .iter()
                .filter(|item| {
                    if order.contains_key(&CardKey::Online(item.download.clone())) {
                        items.push(*item);
                        return false;
                    }
                    true
                })
                .partition(|item| item.trending)
        })
        .unzip();
    if alternate {
        let mut sources = trending;
        for (popular, rest) in sources.iter_mut().zip(regular) {
            popular.extend(rest);
        }
        alternate_sources(&mut items, &sources);
    } else {
        alternate_sources(&mut items, &trending);
        alternate_sources(&mut items, &regular);
    }
    items
}

fn alternate_sources<'a>(items: &mut Vec<&'a Listing>, sources: &[Vec<&'a Listing>]) {
    let rows = sources.iter().map(Vec::len).max().unwrap_or_default();
    for index in 0..rows {
        items.extend(
            sources
                .iter()
                .filter_map(|source| source.get(index).copied()),
        );
    }
}

fn card(
    ui: &mut Ui,
    size: Vec2,
    item: Card<'_>,
    thumbnails: &mut Thumbnails,
    view: View<'_>,
    removing: bool,
    animate: bool,
) -> (Response, Action) {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let response = response.on_hover_cursor(CursorIcon::PointingHand);
    let (title, _, _) = item.label();
    let saved = matches!(item, Card::Saved(..));
    let downloading = view.downloading.contains(&item.key());
    response.widget_info(|| {
        WidgetInfo::labeled(WidgetType::Button, !removing, format!("Apply {title}"))
    });
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
    if ui.is_rect_visible(rect) {
        let image = thumbnails
            .animated(ui.ctx(), item, animate)
            .or_else(|| thumbnails.still(ui.ctx(), item));
        let fade = thumbnails.opacity(ui.ctx(), item);
        let mut draw = ui.painter_at(rect);
        draw.multiply_opacity(fade);
        paint_preview(&draw, rect, image.as_ref(), true);
        paint_card_scrim(&draw, rect);
        let line = if response.has_focus() {
            ACCENT
        } else if response.hovered() {
            MUTED.gamma_multiply(0.75)
        } else {
            LINE
        };
        draw.rect_stroke(rect, 9, Stroke::new(1_f32, line), StrokeKind::Inside);

        let text_width = (rect.width() - 20.).max(1.);
        let title = card_line(&draw, title, heading_font(13.), text_width);
        let pos = pos2(rect.left() + 10., rect.bottom() - 40.);
        draw.galley(
            pos + vec2(0., 1.),
            title.clone(),
            Color32::from_black_alpha(160),
        );
        draw.galley(pos, title, Color32::WHITE);
        let metadata = card_line(
            &draw,
            item.metadata(),
            FontId::proportional(10.),
            text_width,
        );
        draw.galley(pos + vec2(0., 19.), metadata, TEXT);

        let center = rect.right_top() + vec2(-14., 14.);
        if saved {
            draw.circle_filled(center, 9.5, Color32::from_black_alpha(155));
            draw.circle_filled(center, 8., Color32::from_rgb(37, 135, 82));
            draw.add(Shape::line(
                vec![
                    center + vec2(-3.5, 0.),
                    center + vec2(-1., 2.5),
                    center + vec2(3.8, -2.5),
                ],
                Stroke::new(1.6_f32, Color32::WHITE),
            ));
        } else if downloading {
            draw.circle_filled(center, 9., Color32::from_black_alpha(155));
            Spinner::new()
                .size(13.)
                .color(ACCENT.gamma_multiply(fade))
                .paint_at(ui, Rect::from_center_size(center, Vec2::splat(13.)));
        }
    }

    let mut action = if response.clicked() && !removing {
        Action::Apply(item.selection(), view.selected_target())
    } else {
        Action::None
    };
    if ui.is_rect_visible(rect) {
        response.context_menu(|ui| {
            let row_size = vec2(if view.multi { 150. } else { 108. }, 24.);
            ui.set_width(row_size.x);
            let spacing = ui.spacing_mut();
            spacing.interact_size.y = row_size.y;
            spacing.button_padding = vec2(9., 3.);
            spacing.item_spacing.y = 2.;
            ui.visuals_mut().button_frame = true;
            ui.style_mut().wrap_mode = Some(TextWrapMode::Extend);
            if view.multi {
                for (index, display) in view.displays.iter().enumerate() {
                    if ui
                        .add_enabled(
                            !removing,
                            Button::new(format!("Apply to Display {}", index + 1))
                                .min_size(row_size),
                        )
                        .clicked()
                    {
                        action = Action::Apply(item.selection(), Some(display.id.clone()));
                        ui.close_menu();
                    }
                }
            } else if ui
                .add_enabled(!removing, Button::new("Apply").min_size(row_size))
                .clicked()
            {
                action = Action::Apply(item.selection(), None);
                ui.close_menu();
            }
            if let Card::Saved(saved, _) = item
                && ui
                    .add_enabled(
                        !removing && !downloading && !view.importing_local,
                        Button::new(
                            RichText::new("Delete").color(Color32::from_rgb(250, 166, 169)),
                        )
                        .min_size(row_size),
                    )
                    .clicked()
            {
                action = Action::Delete(saved.media.path.clone());
                ui.close_menu();
            }
        });
    }
    (response, action)
}

fn card_line(
    painter: &Painter,
    text: impl Into<String>,
    font: FontId,
    width: f32,
) -> std::sync::Arc<Galley> {
    let mut text = epaint::text::LayoutJob::single_section(
        text.into(),
        TextFormat::simple(font, Color32::PLACEHOLDER),
    );
    text.wrap.max_width = width;
    text.wrap.max_rows = 1;
    text.wrap.break_anywhere = true;
    painter.layout_job(text)
}

fn paint_preview(draw: &Painter, rect: Rect, image: Option<&TextureHandle>, cover: bool) {
    if let Some(image) = image {
        let uv = if cover {
            let aspect = image.aspect_ratio();
            let target = rect.aspect_ratio();
            let uv_size = vec2((target / aspect).min(1.), (aspect / target).min(1.));
            Rect::from_center_size(pos2(0.5, 0.5), uv_size)
        } else {
            Rect::from_min_max(Pos2::ZERO, pos2(1., 1.))
        };
        draw.add(epaint::RectShape::filled(rect, 8, Color32::WHITE).with_texture(image.id(), uv));
    } else {
        draw.rect_filled(rect, 8, FIELD);
        Icon::Picture.paint(draw, rect.center(), MUTED.gamma_multiply(0.5));
    }
}

fn paint_card_scrim(painter: &Painter, rect: Rect) {
    const STEPS: u32 = 14;
    let height = (rect.height() * 0.72).min(118.);
    let radius = 8_f32.min(height);
    let mut fade = Mesh::default();
    for step in 0..=STEPS {
        let t = step as f32 / STEPS as f32;
        let y = rect.bottom() - height * (1. - t);
        let curve = (y - (rect.bottom() - radius)).max(0.);
        let inset = radius - (radius * radius - curve * curve).max(0.).sqrt();
        let tint = Color32::from_black_alpha((178. * t.powf(1.55)) as u8);
        fade.colored_vertex(pos2(rect.left() + inset, y), tint);
        fade.colored_vertex(pos2(rect.right() - inset, y), tint);
        if step > 0 {
            let a = (step - 1) * 2;
            fade.add_triangle(a, a + 1, a + 2);
            fade.add_triangle(a + 2, a + 1, a + 3);
        }
    }
    painter.add(Shape::mesh(fade));
}

fn show_preview_tooltip(
    ctx: &Context,
    state: &mut Option<(CardKey, Option<f64>)>,
    thumbnails: &mut Thumbnails,
    hovered: Option<(Card<'_>, Response)>,
    scrolling: bool,
) {
    let Some((item, response)) = hovered else {
        *state = None;
        return;
    };
    let (time, pressed) = ctx.input(|i| (i.time, i.pointer.any_down() || i.pointer.any_released()));
    let key = item.key();
    if state.as_ref().is_none_or(|(previous, _)| previous != &key) {
        *state = Some((key, Some(time)));
    }
    let (_, since) = state.as_mut().expect("hovered card");
    if pressed {
        *since = None;
    } else if scrolling && since.is_some() {
        *since = Some(time);
    }
    let Some(since) = *since else {
        return;
    };
    let delay = PREVIEW_DELAY - (time - since);
    if delay > 0. {
        ctx.request_repaint_after(Duration::from_secs_f64(delay));
        return;
    }
    let Some(image) = thumbnails
        .animated(ctx, item, true)
        .or_else(|| thumbnails.still(ctx, item))
    else {
        return;
    };
    let bounds = ctx.screen_rect();
    let limit = (bounds.size() * vec2(0.60, 0.62)).min(vec2(540., 380.));
    let aspect = image.aspect_ratio();
    let width = limit.x.min(limit.y * aspect);
    let size = vec2(width, width / aspect);
    let rect = Rect::from_center_size(bounds.center(), size);
    let draw = ctx.layer_painter(LayerId::new(
        Order::Tooltip,
        response.id.with("gallery preview"),
    ));
    draw.rect_filled(rect.expand(1.), 9, LINE);
    draw.rect_filled(rect, 8, INK);
    paint_preview(&draw, rect, Some(&image), false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_apply_keeps_gallery_open_for_one_second_after_the_checkmark() {
        for multi in [false, true] {
            let ctx = Context::default();
            super::super::set_fonts(&ctx);
            let mut popup = Gallery::default();
            popup.criteria.media = MediaFilter::Still;
            popup.generation = 1;
            let listing = Listing::test_wallpaper("Wallpaper to apply", 1);
            popup.searches[2].results = vec![listing.clone()];
            let (_scan, result) = channel();
            popup.scan = Some((0, result));
            let displays = [Display {
                id: "display".into(),
                name: "Display".into(),
                primary: true,
                px: crate::layout::Rect::new(0., 0., 1920., 1080.),
                mm: [510., 290.],
                estimated: false,
                mirrors: Vec::new(),
                mirror_ids: Vec::new(),
            }];
            let downloading = HashSet::new();
            let view = View {
                displays: &displays,
                selected: 0,
                multi,
                downloading: &downloading,
                importing_local: false,
            };
            let draw = |popup: &mut Gallery, time, events, opening| {
                let mut action = Action::None;
                let output = ctx.run(
                    RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(760., 450.))),
                        time: Some(time),
                        events,
                        ..Default::default()
                    },
                    |ctx| action = popup.show(ctx, opening, 1., view),
                );
                (action, output)
            };
            draw(&mut popup, 0., vec![], true);
            let (_, output) = draw(&mut popup, 0.1, vec![], false);
            let pos = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Text(text) if text.galley.text() == listing.title => {
                        Some(text.visual_bounding_rect().center())
                    }
                    _ => None,
                })
                .unwrap();
            let click = |pressed| Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            draw(
                &mut popup,
                0.2,
                vec![Event::PointerMoved(pos), click(true)],
                false,
            );
            let (action, _) = draw(&mut popup, 0.3, vec![click(false)], false);
            let Action::Apply(selection, target) = action else {
                panic!("Wallpaper card did not apply")
            };
            assert_eq!(target.as_deref(), multi.then_some("display"));
            popup.finish_span_apply(&ctx, &CardKey::Online("previous selection".into()));
            assert!(matches!(
                draw(&mut popup, 2., vec![], false).0,
                Action::None
            ));

            let path = PathBuf::from("downloaded.png");
            popup
                .thumbnails
                .loaded
                .push_back((path.to_string_lossy().into_owned(), None));
            popup.record_saved(SavedWallpaper {
                media: crate::layout::Media {
                    path,
                    width: listing.width,
                    height: listing.height,
                },
                title: listing.title,
                origin: Some(listing.download),
                animated: false,
                bytes: 1,
            });
            popup.finish_span_apply(&ctx, &selection.key());
            let (action, output) = draw(&mut popup, 2.99, vec![], false);
            assert!(matches!(action, Action::None));
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
                Shape::Circle(circle) if circle.fill == Color32::from_rgb(37, 135, 82)
            )));
            assert_eq!(
                matches!(draw(&mut popup, 3., vec![], false).0, Action::Close),
                !multi
            );
        }
    }

    #[test]
    fn trending_results_alternate_before_the_rest_of_the_gallery() {
        let mut gallery = Gallery::default();
        let listing = |title, index, trending| {
            let mut item = Listing::test_wallpaper(title, index);
            item.trending = trending;
            item
        };
        gallery.searches[0].results = vec![
            listing("MotionBGs 1", 1, true),
            listing("MotionBGs 2", 2, true),
            listing("MotionBGs recent", 3, false),
        ];
        gallery.searches[1].results = vec![
            listing("DesktopHut 1", 4, true),
            listing("DesktopHut 2", 5, true),
        ];
        gallery.searches[2].results = vec![listing("Wallhaven Hot 1", 6, false)];
        let names = |alternate| {
            gallery_items(&gallery.searches, &gallery.page_order, alternate)
                .into_iter()
                .map(|item| item.title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(false),
            [
                "MotionBGs 1",
                "DesktopHut 1",
                "MotionBGs 2",
                "DesktopHut 2",
                "MotionBGs recent",
                "Wallhaven Hot 1",
            ]
        );
        assert_eq!(
            names(true),
            [
                "MotionBGs 1",
                "DesktopHut 1",
                "Wallhaven Hot 1",
                "MotionBGs 2",
                "DesktopHut 2",
                "MotionBGs recent",
            ]
        );
    }

    #[test]
    fn load_more_keeps_existing_cards_and_appends_the_next_page() {
        let mut popup = Gallery::default();
        let listing = Listing::test_wallpaper;
        popup.searches[0].results = vec![listing("first", 1), listing("third", 3)];
        popup.searches[1].results = vec![listing("second", 2)];
        popup.searches[2].results = vec![listing("wallhaven", 4)];
        fn ranked(popup: &Gallery) -> Vec<Card<'_>> {
            cards(
                &popup.searches,
                &popup.saved,
                &popup.criteria,
                false,
                &popup.page_order,
            )
        }
        let keys = |cards: &[Card<'_>]| {
            cards
                .iter()
                .map(|card| card.order_key())
                .collect::<Vec<_>>()
        };
        let previous = keys(&ranked(&popup));
        popup.page_order = previous
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, key)| (key, index))
            .collect();

        popup.searches[0].results.push(listing("next motion", 5));
        let mut items = ranked(&popup);
        keep_page_order(&mut items, &popup.page_order, true);
        assert_eq!(keys(&items), previous);
        popup.searches[1].results.push(listing("next desktop", 6));
        popup.searches[2].results.push(listing("next wallhaven", 7));
        popup.searches[0].results.push(listing("later motion", 8));
        let mut items = ranked(&popup);
        keep_page_order(&mut items, &popup.page_order, false);
        assert_eq!(&keys(&items)[..previous.len()], previous);
        assert_eq!(
            items[previous.len()..]
                .iter()
                .map(|card| card.label().0)
                .collect::<Vec<_>>(),
            [
                "next motion",
                "next desktop",
                "next wallhaven",
                "later motion"
            ]
        );
    }

    #[test]
    fn next_batch_is_prefetched_and_waits_to_reveal_until_near_the_end() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let mut gallery = Gallery::default();
        gallery.searches[2].results = (0..24)
            .map(|index| Listing::test_wallpaper("Hot wallpaper", index))
            .collect();
        gallery.searches[2].next = Some(SearchCursor::Wallhaven(2));
        let downloading = HashSet::new();
        let view = View {
            displays: &[],
            selected: 0,
            multi: false,
            downloading: &downloading,
            importing_local: false,
        };
        let draw = |gallery: &mut Gallery, scroll_offset| {
            ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(760., 450.))),
                    ..Default::default()
                },
                |ctx| {
                    CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                        if let Some(offset) = scroll_offset {
                            let id = ui.make_persistent_id(Id::new("gallery results"));
                            let mut state = scroll_area::State::load(ctx, id).unwrap();
                            state.offset.y = offset;
                            state.store(ctx, id);
                        }
                        gallery.grid(ui, 350., view);
                    });
                },
            )
        };
        draw(&mut gallery, None);
        assert!(gallery.batch == Some(Batch::Next));
        assert_eq!(gallery.searches[2].pages_left, 2);
        assert_eq!(gallery.page_order.len(), 24);

        for (page, pages_left) in [(2, 1), (3, 0)] {
            let (source, result) = channel();
            let search = &mut gallery.searches[2];
            search.request = Some((gallery.generation, result));
            source
                .send(Ok(Page {
                    listings: vec![Listing::test_wallpaper("More wallpapers", 24 + page)],
                    next: Some(SearchCursor::Wallhaven(page + 1)),
                }))
                .ok()
                .unwrap();
            search.poll(gallery.generation);
            assert_eq!(search.pages_left, pages_left);
            assert!(matches!(search.next, Some(SearchCursor::Wallhaven(next)) if next == page + 1));
            let offset = if pages_left > 0 { 550. } else { 0. };
            let output = draw(&mut gallery, Some(offset));
            assert!(gallery.batch == Some(Batch::Next));
            assert!(!output.shapes.iter().any(|shape| matches!(&shape.shape,
                Shape::Text(text) if text.galley.text() == "More wallpapers"
            )));
            assert_eq!(gallery.page_order.len(), 24);
        }
        assert_eq!(gallery.searches[2].results.len(), 26);

        draw(&mut gallery, Some(1000.));
        assert!(gallery.batch.is_none());
        let output = draw(&mut gallery, Some(1000.));
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            Shape::Text(text) if text.galley.text() == "More wallpapers"
        )));
        assert!(gallery.batch == Some(Batch::Next));
        assert_eq!(gallery.page_order.len(), 26);
        assert_eq!(gallery.searches[2].pages_left, 2);
    }

    #[test]
    fn first_page_waits_for_sources_and_posters_then_fades_the_cards_in() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let mut gallery = Gallery::default();
        gallery.generation = 1;
        gallery.batch = Some(Batch::Initial);
        let thumbnail = "https://th.wallhaven.cc/lg/00/000001.jpg";
        let mut first = Listing::test_wallpaper("First wallpaper", 1);
        first.thumbnail = Some(thumbnail.into());
        gallery.searches[0].results.push(first);
        let (source, result) = channel();
        gallery.searches[1].request = Some((1, result));
        let (poster, result) = channel();
        gallery.thumbnails.pending.push((thumbnail.into(), result));
        gallery.thumbnails.browsing = Some(Cancellation::default());
        let downloading = HashSet::new();
        let view = View {
            displays: &[],
            selected: 0,
            multi: false,
            downloading: &downloading,
            importing_local: false,
        };
        let draw = |gallery: &mut Gallery, time| {
            ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(760., 450.))),
                    time: Some(time),
                    ..Default::default()
                },
                |ctx| {
                    gallery.show(ctx, false, 1., view);
                },
            )
        };
        let has_label = |output: &FullOutput, label| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape,
                    Shape::Text(text) if text.galley.text() == label
                )
            })
        };
        draw(&mut gallery, 0.);
        let waiting = draw(&mut gallery, 0.1);
        assert!(has_label(&waiting, "Loading popular wallpapers…"));
        assert!(!has_label(&waiting, "First wallpaper"));
        source
            .send(Ok(Page {
                listings: vec![Listing::test_wallpaper("Second wallpaper", 2)],
                next: None,
            }))
            .ok()
            .unwrap();
        let waiting = draw(&mut gallery, 0.2);
        assert!(has_label(&waiting, "Loading popular wallpapers…"));
        assert!(!has_label(&waiting, "Second wallpaper"));
        poster.send(Ok(image::RgbaImage::new(24, 16))).unwrap();

        let faded = draw(&mut gallery, 0.3);
        assert!(!has_label(&faded, "Loading popular wallpapers…"));
        assert!(has_label(&faded, "First wallpaper"));
        assert!(has_label(&faded, "Second wallpaper"));
        assert!(has_label(&faded, "3840 × 2160 · Still · wallhaven"));
        let card_fill = |output: &FullOutput, alpha| {
            output.shapes.iter().any(|shape| {
                matches!(&shape.shape,
                    Shape::Rect(rect)
                        if rect.corner_radius == CornerRadius::same(8) && rect.fill.a() == alpha
                )
            })
        };
        assert!(card_fill(&faded, Color32::WHITE.gamma_multiply(0.2).a()));
        assert!(card_fill(&draw(&mut gallery, 0.6), 255));
    }

    #[test]
    fn hover_preview_grows_and_right_click_shows_the_compact_menu() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(760., 450.));
        let saved = SavedWallpaper {
            media: crate::layout::Media {
                path: "test.png".into(),
                ..Default::default()
            },
            title: "Saved picture".into(),
            origin: None,
            animated: false,
            bytes: 0,
        };
        let image = ctx.load_texture(
            "test card",
            ColorImage::new([32, 18], Color32::GRAY),
            TextureOptions::LINEAR,
        );
        let mut thumbnails = Thumbnails::default();
        thumbnails
            .loaded
            .push_back(("test.png".into(), Some(image.clone())));
        let mut hover = None;
        let item = Card::Saved(&saved, None);
        let view = View {
            displays: &[],
            selected: 0,
            multi: false,
            downloading: &HashSet::new(),
            importing_local: false,
        };
        let mut run = |time, events| {
            ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                        let (response, _) = card(
                            ui,
                            vec2(240., 135.),
                            item,
                            &mut thumbnails,
                            view,
                            false,
                            true,
                        );
                        let hovered = (response.hovered() && !response.context_menu_opened())
                            .then_some((item, response));
                        show_preview_tooltip(ctx, &mut hover, &mut thumbnails, hovered, false);
                    });
                },
            )
        };
        let has_hover_image = |output: FullOutput| {
            ctx.tessellate(output.shapes, output.pixels_per_point)
                .iter()
                .any(|primitive| match &primitive.primitive {
                    epaint::Primitive::Mesh(mesh) => {
                        mesh.texture_id == image.id()
                            && (360. ..470.).contains(&mesh.calc_bounds().width())
                    }
                    epaint::Primitive::Callback(_) => false,
                })
        };
        let pointer = pos2(90., 50.);
        run(0., vec![]);
        run(0.1, vec![Event::PointerMoved(pointer)]);
        assert!(!has_hover_image(run(0.5, vec![])));
        assert!(has_hover_image(run(0.8, vec![])));
        for (time, pressed) in [(0.9, true), (1.0, false)] {
            let output = run(
                time,
                vec![Event::PointerButton {
                    pos: pointer,
                    button: PointerButton::Secondary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }],
            );
            assert!(!has_hover_image(output));
        }
        let output = run(1.1, vec![]);
        let text_position = |label| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Text(text) if text.galley.text() == label => Some(text.pos),
                    _ => None,
                })
                .unwrap()
        };
        let apply = text_position("Apply");
        let delete = text_position("Delete");
        assert!(apply.x >= pointer.x + 9.);
        assert_eq!(apply.x, delete.x);
        assert!((delete.y - apply.y - 26.).abs() < 1.);
    }

    #[test]
    fn filter_popup_changes_type_resolution_and_downloaded_results() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let mut gallery = Gallery::default();
        assert_eq!(gallery.criteria.media, MediaFilter::All);
        assert!(!gallery.downloaded);
        let (_scan, result) = channel();
        gallery.scan = Some((gallery.revision, result));
        for (path, animated, width, height) in [
            ("Saved photo.png", false, 1920, 1080),
            ("Saved video.mp4", true, 3840, 2160),
        ] {
            gallery.saved.upsert(SavedWallpaper {
                media: crate::layout::Media {
                    path: path.into(),
                    width,
                    height,
                },
                title: path.into(),
                origin: None,
                animated,
                bytes: 0,
            });
        }
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(760., 450.));
        let bounds = screen.shrink(16.);
        {
            let mut run = |events| {
                ctx.run(
                    RawInput {
                        screen_rect: Some(screen),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        CentralPanel::default().frame(Frame::NONE).show(ctx, |ui| {
                            ui.scope_builder(UiBuilder::new().max_rect(bounds), |ui| {
                                ui.set_width(bounds.width());
                                gallery.controls(ui, false, bounds);
                            });
                        });
                    },
                )
            };
            let output = run(vec![]);
            let filter = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Path(path) if path.closed && path.points.len() == 6 => {
                        Some(Rect::from_points(&path.points).center())
                    }
                    _ => None,
                })
                .unwrap();
            let mut click = |pos| {
                for pressed in [true, false] {
                    run(vec![
                        Event::PointerMoved(pos),
                        Event::PointerButton {
                            pos,
                            button: PointerButton::Primary,
                            pressed,
                            modifiers: Modifiers::NONE,
                        },
                    ]);
                }
                run(vec![])
            };
            let mut output = click(filter);
            let text_bounds = |output: &FullOutput, label| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        Shape::Text(text) if text.galley.text() == label => {
                            Some(text.visual_bounding_rect())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing filter control {label}"))
            };
            let panel = ctx
                .memory(|memory| memory.area_rect(Id::new(FILTERS_POPUP)))
                .unwrap();
            assert!(bounds.contains_rect(panel));
            assert!(text_bounds(&output, "Type").left() >= panel.left() + 16.);
            assert!(text_bounds(&output, "Type").top() >= panel.top() + 13.);
            let still = text_bounds(&output, "Still").center();
            assert!(text_bounds(&output, "Any resolution").center().x > still.x + 100.);
            output = click(still);
            output = click(text_bounds(&output, "Downloaded").center());
            click(text_bounds(&output, "4K+").center());
        }
        assert_eq!(gallery.criteria.media, MediaFilter::Animated);
        assert_eq!(gallery.criteria.resolution, Resolution::Uhd);
        assert!(gallery.downloaded);
        assert!(gallery.searches.iter().all(|search| search.pages_left == 0));
        let items = cards(
            &gallery.searches,
            &gallery.saved,
            &gallery.criteria,
            gallery.downloaded,
            &gallery.page_order,
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label().0, "Saved video.mp4");
    }

    #[test]
    fn keyboard_submits_search_and_escape_dismisses_the_popup() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let mut popup = Gallery::default();
        popup.generation = 1;
        let titles = ["Mountain", "Ocean", "Forest"];
        popup.searches[2].results = titles
            .iter()
            .enumerate()
            .map(|(index, title)| Listing::test_wallpaper(title, index as u32))
            .collect();
        let (_local_scan, scan) = channel();
        popup.scan = Some((0, scan));
        let key = |key| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(760., 450.));
        let run = |popup: &mut Gallery, events, opening| {
            let mut action = Action::None;
            let output = ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    action = popup.show(
                        ctx,
                        opening,
                        1.,
                        View {
                            displays: &[],
                            selected: 0,
                            multi: false,
                            downloading: &HashSet::new(),
                            importing_local: false,
                        },
                    )
                },
            );
            (action, output)
        };
        run(&mut popup, Vec::new(), true);
        run(&mut popup, Vec::new(), false);
        let (_, output) = run(&mut popup, Vec::new(), false);
        for label in std::iter::once("Gallery").chain(titles) {
            assert!(
                output.shapes.iter().any(|shape| {
                    let epaint::Shape::Text(text) = &shape.shape else {
                        return false;
                    };
                    text.galley.text() == label
                        && screen
                            .shrink(16.)
                            .intersect(shape.clip_rect)
                            .contains_rect(text.visual_bounding_rect())
                }),
                "{label} is clipped in the minimum window size"
            );
        }
        popup.stop_browsing();
        run(&mut popup, vec![Event::Text("mountain".into())], false);
        run(&mut popup, vec![key(Key::Enter)], false);
        assert!(matches!(
            run(&mut popup, vec![key(Key::Escape)], false).0,
            Action::Close
        ));
        assert_eq!(popup.criteria.text, "mountain");
        assert!(popup.searches.iter().all(|search| search.pages_left == 1));
        assert!(popup.thumbnails.browsing.is_none());
        assert!(popup.searches.iter().all(|search| search.request.is_none()));
    }

    #[test]
    fn typing_auto_searches_after_one_second_and_resets_on_more_input() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let (_local_scan, scan) = channel();
        let mut popup = Gallery {
            generation: 1,
            scan: Some((0, scan)),
            ..Default::default()
        };
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(760., 450.));
        let run = |popup: &mut Gallery, time, events, opening| {
            let _ = ctx.run(
                RawInput {
                    screen_rect: Some(screen),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    popup.show(
                        ctx,
                        opening,
                        1.,
                        View {
                            displays: &[],
                            selected: 0,
                            multi: false,
                            downloading: &HashSet::new(),
                            importing_local: false,
                        },
                    );
                },
            );
        };
        run(&mut popup, 0.0, Vec::new(), true);
        popup.stop_browsing();
        run(&mut popup, 1.0, vec![Event::Text("cat".into())], false);
        assert_eq!(popup.criteria.text, "");
        run(&mut popup, 1.8, Vec::new(), false);
        assert_eq!(popup.criteria.text, "");
        // Typing more pushes the 1s debounce timer forward.
        run(&mut popup, 1.8, vec![Event::Text("s".into())], false);
        run(&mut popup, 2.5, Vec::new(), false);
        assert_eq!(popup.criteria.text, "");
        run(&mut popup, 2.8, Vec::new(), false);
        assert_eq!(popup.criteria.text, "cats");
        assert!(popup.search_at.is_none());
    }
}
