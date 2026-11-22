use super::{ACCENT, FIELD, INK, LINE, MUTED, TEXT, heading_font, icons::Icon, popup, texture};
use crate::gallery::{
    self, Catalog, Criteria, Listing, MediaFilter, Page, PreparedWallpaper, PreviewFrames, Removal,
    Resolution, SavedWallpaper, SearchCursor,
};
use crate::http::Cancellation;
use crate::layout::Display;
use anyhow::{Result, anyhow};
use eframe::egui::*;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::mpsc::{Receiver, TryRecvError, channel},
    time::{Duration, SystemTime},
};

const GAP: f32 = 12.;
const PREVIEW_DELAY: f64 = 2.;
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
    fn poll(&mut self, ctx: &Context) {
        let still_limit = self.visible_slots.max(64);
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
        self.animated
            .drain(..self.animated.len().saturating_sub(slots.max(12)));
        self.loaded
            .drain(..self.loaded.len().saturating_sub(slots.max(64)));
    }

    fn still(&mut self, ctx: &Context, card: Card<'_>) -> Option<TextureHandle> {
        let key = match card {
            Card::Online(listing) => listing.thumbnail.clone()?,
            Card::Saved(saved, _) => saved.media.path.to_string_lossy().into_owned(),
        };
        if let Some((_, image)) = self.loaded.iter().find(|(loaded, _)| loaded == &key) {
            return image.clone();
        }
        if self.pending.len() < 2
            && !self.pending.iter().any(|(pending, _)| pending == &key)
            && let Some(cancellation) = self.browsing.clone()
        {
            let job = match card {
                Card::Online(listing) => {
                    let source = listing.source;
                    let raw = key.clone();
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
            self.pending.push((key, job));
        }
        None
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
    queued: bool,
    next: Option<SearchCursor>,
}

impl RemoteSearch {
    fn new(initial: SearchCursor) -> Self {
        Self {
            initial,
            results: Vec::new(),
            request: None,
            queued: false,
            next: None,
        }
    }

    fn reset(&mut self, criteria: &Criteria) {
        self.results.clear();
        self.queued = self.initial.supports(criteria);
        self.next = self.queued.then(|| self.initial.clone());
    }

    fn poll(&mut self, generation: u64, criteria: &Criteria, featured: &[Listing]) {
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
        let Ok(page) = result else {
            self.next = None;
            return;
        };
        self.next = page.next;
        let mut seen: HashSet<_> = featured
            .iter()
            .filter(|item| criteria.matches_featured(item))
            .chain(self.results.iter())
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
        if self.queued
            && self.request.is_none()
            && let Some(cursor) = self.next.clone()
        {
            let criteria = criteria.clone();
            let cancellation = cancellation.clone();
            self.request = Some((
                generation,
                background(ctx, move || cursor.fetch(&criteria, &cancellation)),
            ));
            self.queued = false;
        }
    }

    fn loading(&self, generation: u64) -> bool {
        self.queued
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

pub(super) struct Gallery {
    featured: Vec<Listing>,
    searches: Vec<RemoteSearch>,
    draft: String,
    criteria: Criteria,
    generation: u64,
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
            featured: gallery::featured(),
            searches: vec![
                RemoteSearch::new(SearchCursor::MotionBgs(None)),
                RemoteSearch::new(SearchCursor::DesktopHut(None)),
                RemoteSearch::new(SearchCursor::Commons(None)),
            ],
            draft: String::new(),
            criteria: Criteria {
                media: MediaFilter::Animated,
                ..Criteria::default()
            },
            generation: 0,
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
        self.criteria = criteria;
        self.generation += 1;
        self.page_order.clear();
        for search in &mut self.searches {
            search.reset(&self.criteria);
            if self.downloaded {
                search.queued = false;
                search.next = None;
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
            if search
                .request
                .take()
                .is_some_and(|(started, _)| started == self.generation)
            {
                search.queued = search.next.is_some();
            }
        }
    }

    fn close(&mut self, ctx: &Context) -> Action {
        self.span_apply = None;
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
            search.poll(self.generation, &self.criteria, &self.featured);
        }
        self.thumbnails.poll(ctx);
        let screen = ctx.screen_rect().size();
        let gutter = ((screen - vec2(760., 450.)) * vec2(0.16, 0.12) + vec2(16., 16.))
            .clamp(vec2(16., 16.), vec2(120., 86.));
        let outer = screen - 2. * gutter;
        let size = (outer - vec2(36., 34.)).max(vec2(1., 1.));
        let dialog = popup::dialog(
            Id::new(DIALOG_ID),
            outer,
            Margin::symmetric(17, 16),
            opacity,
        )
        .show(ctx, |ui| {
            ui.multiply_opacity(opacity);
            ui.set_width(size.x);
            ui.set_min_height(size.y);
            ui.spacing_mut().item_spacing = vec2(8., 10.);
            let close = ui
                .horizontal(|ui| {
                    ui.label(RichText::new("Gallery").font(heading_font(17.)).color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        Icon::Close.button(ui, "Close gallery", false).clicked()
                    })
                    .inner
                })
                .inner;
            self.controls(ui, opening);
            if let Some(error) = self.error() {
                ui.add(Label::new(RichText::new(error).small().color(ACCENT)).wrap());
            }
            ui.add_space(2.);
            let height = size.y - (ui.next_widget_position().y - ui.min_rect().top());
            let chosen = self.grid(ui, height.max(1.), view);
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

    fn controls(&mut self, ui: &mut Ui, opening: bool) {
        let mut criteria = self.criteria.clone();
        let mut downloaded = self.downloaded;
        let submit = ui
            .horizontal(|ui| {
                ui.spacing_mut().interact_size.y = 34.;
                let search_width = ui.available_width() - 2. * (32. + ui.spacing().item_spacing.x);
                let field = self.search_field(ui, opening, search_width);
                filters(ui, &mut criteria, &mut downloaded);
                field
            })
            .inner;
        if submit || criteria != self.criteria || downloaded != self.downloaded {
            criteria.text = gallery::search_text(&self.draft);
            if criteria != self.criteria
                || downloaded != self.downloaded
                || (submit
                    && !self
                        .searches
                        .iter()
                        .any(|search| search.loading(self.generation)))
            {
                if downloaded && !self.downloaded && self.thumbnails.browsing.is_some() {
                    self.stop_browsing();
                    self.thumbnails.browsing = Some(Cancellation::default());
                }
                self.downloaded = downloaded;
                if downloaded {
                    self.refresh_saved(ui.ctx());
                }
                self.start_search(criteria);
            }
        } else if self.draft.trim().is_empty() && !self.criteria.text.is_empty() {
            criteria.text.clear();
            self.start_search(criteria);
        }
    }

    fn search_field(&mut self, ui: &mut Ui, opening: bool, width: f32) -> bool {
        let field = ui.add_sized(
            [width.max(80.), 34.],
            TextEdit::singleline(&mut self.draft)
                .hint_text("Search wallpapers")
                .char_limit(120)
                .vertical_align(Align::Center)
                .background_color(FIELD)
                .margin(Margin::symmetric(11, 5))
                .min_size(vec2(0., 34.)),
        );
        field.widget_info(|| WidgetInfo::labeled(WidgetType::TextEdit, true, "Search wallpapers"));
        if opening {
            field.request_focus();
        }
        let enter = field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        if enter {
            field.request_focus();
        }
        Icon::Search.button(ui, "Search", false).clicked() || enter
    }

    fn grid(&mut self, ui: &mut Ui, height: f32, view: View<'_>) -> Action {
        let mut items = cards(
            &self.searches,
            &self.featured,
            &self.saved,
            &self.criteria,
            self.downloaded,
        );
        let loading = if self.downloaded {
            self.scanning() && items.is_empty()
        } else {
            self.searches
                .iter()
                .any(|search| search.loading(self.generation))
        };
        if !self.page_order.is_empty() {
            keep_page_order(&mut items, &self.page_order, loading);
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
        let footer = loading || has_more || items.is_empty();
        let mut scroll = ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(height)
            .id_salt("gallery results");
        if std::mem::take(&mut self.reset_scroll) {
            scroll = scroll.vertical_scroll_offset(0.);
        }

        let mut action = Action::None;
        let mut load_more = false;
        let mut hovered = None;
        scroll.show_rows(
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
                            } else if has_more {
                                load_more = ui.button("Load more").clicked();
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
        if load_more {
            self.page_order = items
                .iter()
                .enumerate()
                .map(|(index, card)| (card.order_key(), index))
                .collect();
            for search in &mut self.searches {
                search.queued = search.next.is_some();
            }
        }
        action
    }
}

fn keep_page_order(items: &mut Vec<Card<'_>>, order: &HashMap<CardKey, usize>, loading: bool) {
    let rank = |item: &Card<'_>| {
        order
            .get(&item.order_key())
            .or_else(|| order.get(&item.key()))
            .copied()
    };
    if loading {
        // Reveal the next page together, even when its sources finish at different times.
        items.retain(|item| rank(item).is_some());
    }
    items.sort_by_cached_key(|item| rank(item).unwrap_or(usize::MAX));
}

fn cards<'a>(
    searches: &'a [RemoteSearch],
    featured: &'a [Listing],
    saved: &'a Catalog,
    criteria: &Criteria,
    downloaded: bool,
) -> Vec<Card<'a>> {
    let local = saved
        .items
        .iter()
        .filter(|item| criteria.matches_saved(item));
    if downloaded {
        return local.map(|item| Card::Saved(item, None)).collect();
    }
    let mut paths = HashSet::new();
    let mut cards: Vec<_> = gallery_items(searches, featured, criteria)
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
    let insert_at = cards
        .iter()
        .take_while(|card| match card {
            Card::Online(listing) | Card::Saved(_, Some(listing)) => listing.trending,
            Card::Saved(_, None) => false,
        })
        .count();
    cards.splice(
        insert_at..insert_at,
        local
            .filter(|item| !paths.contains(&item.media.path))
            .map(|item| Card::Saved(item, None)),
    );
    cards
}

fn filters(ui: &mut Ui, criteria: &mut Criteria, downloaded: &mut bool) {
    let popup_id = Id::new(FILTERS_POPUP);
    let open = ui.memory(|m| m.is_popup_open(popup_id));
    let filtered = criteria.media != MediaFilter::Animated
        || criteria.resolution != Resolution::Any
        || *downloaded;
    let icon = Icon::Filter
        .button(ui, "Filter wallpapers", open || filtered)
        .on_hover_text("Filter wallpapers");
    if icon.clicked() {
        ui.memory_mut(|m| m.toggle_popup(popup_id));
    }
    popup_below_widget(
        ui,
        popup_id,
        &icon,
        PopupCloseBehavior::CloseOnClickOutside,
        |ui| {
            ui.set_width(246.);
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
        },
    );
}

fn gallery_items<'a>(
    searches: &'a [RemoteSearch],
    featured: &'a [Listing],
    criteria: &Criteria,
) -> Vec<&'a Listing> {
    let (trending, mut regular): (Vec<Vec<_>>, Vec<Vec<_>>) = searches
        .iter()
        .map(|search| search.results.iter().partition(|item| item.trending))
        .unzip();
    regular.insert(
        0,
        featured
            .iter()
            .filter(|item| criteria.matches_featured(item))
            .collect(),
    );
    let mut items = Vec::new();
    for groups in [trending, regular] {
        let rows = groups.iter().map(Vec::len).max().unwrap_or_default();
        for index in 0..rows {
            items.extend(groups.iter().filter_map(|group| group.get(index).copied()));
        }
    }
    items
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
    let (title, width, height) = item.label();
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
        paint_preview(ui.painter(), rect, image.as_ref(), true);
        paint_card_scrim(ui.painter(), rect);
        let line = if response.has_focus() {
            ACCENT
        } else if response.hovered() {
            MUTED.gamma_multiply(0.75)
        } else {
            LINE
        };
        ui.painter()
            .rect_stroke(rect, 9, Stroke::new(1_f32, line), StrokeKind::Inside);

        let title = card_title(ui.painter(), title, rect.width() - 20.);
        let pos = pos2(rect.left() + 10., rect.bottom() - 40.);
        ui.painter().galley(
            pos + vec2(0., 1.),
            title.clone(),
            Color32::from_black_alpha(160),
        );
        ui.painter().galley(pos, title, Color32::WHITE);
        let dimensions = ui.painter().layout_no_wrap(
            format!("{width} × {height}"),
            FontId::proportional(11.),
            TEXT,
        );
        ui.painter().galley(pos + vec2(0., 19.), dimensions, TEXT);

        let center = rect.right_top() + vec2(-14., 14.);
        if saved {
            ui.painter()
                .circle_filled(center, 9.5, Color32::from_black_alpha(155));
            ui.painter()
                .circle_filled(center, 8., Color32::from_rgb(37, 135, 82));
            ui.painter().add(Shape::line(
                vec![
                    center + vec2(-3.5, 0.),
                    center + vec2(-1., 2.5),
                    center + vec2(3.8, -2.5),
                ],
                Stroke::new(1.6_f32, Color32::WHITE),
            ));
        } else if downloading {
            ui.painter()
                .circle_filled(center, 9., Color32::from_black_alpha(155));
            Spinner::new()
                .size(13.)
                .color(ACCENT)
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

fn card_title(painter: &Painter, title: &str, width: f32) -> std::sync::Arc<Galley> {
    let mut text = epaint::text::LayoutJob::single_section(
        title.into(),
        TextFormat::simple(heading_font(13.), Color32::PLACEHOLDER),
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
    let limit = bounds.size() * vec2(0.9, 0.88);
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
            popup.searches.clear();
            let mut listing = popup
                .featured
                .iter()
                .find(|item| !item.animated())
                .unwrap()
                .clone();
            listing.thumbnail = None;
            listing.title = "Wallpaper to apply".into();
            popup.featured = vec![listing.clone()];
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
        let mut popup = Gallery::default();
        popup.criteria.media = MediaFilter::All;
        let listing = |title: &str, trending| {
            let mut item = popup.featured[0].clone();
            item.title = title.to_owned();
            item.trending = trending;
            item
        };
        popup.searches[0].results = vec![
            listing("MotionBGs 1", true),
            listing("MotionBGs 2", true),
            listing("MotionBGs recent", false),
        ];
        popup.searches[1].results =
            vec![listing("DesktopHut 1", true), listing("DesktopHut 2", true)];
        let names = |popup: &Gallery| {
            gallery_items(&popup.searches, &popup.featured, &popup.criteria)
                .into_iter()
                .take(5)
                .map(|item| item.title.clone())
                .collect::<Vec<_>>()
        };
        let featured = popup.featured[0].title.as_str();
        assert_eq!(
            names(&popup),
            [
                "MotionBGs 1",
                "DesktopHut 1",
                "MotionBGs 2",
                "DesktopHut 2",
                featured
            ]
        );
        popup.searches[1].results.clear();
        assert_eq!(
            &names(&popup)[..3],
            ["MotionBGs 1", "MotionBGs 2", featured]
        );
    }

    #[test]
    fn load_more_keeps_existing_cards_and_appends_the_next_page() {
        let mut popup = Gallery::default();
        let listing = |name: &str| {
            let mut item = popup.featured[0].clone();
            item.title = name.to_owned();
            item.download = format!("https://example.com/{name}");
            item.trending = true;
            item
        };
        popup.searches[0].results = vec![listing("first")];
        popup.searches[1].results = vec![listing("second"), listing("third")];
        fn ranked(popup: &Gallery) -> Vec<Card<'_>> {
            cards(
                &popup.searches,
                &popup.featured,
                &popup.saved,
                &popup.criteria,
                false,
            )
        }
        let keys = |cards: &[Card<'_>]| {
            cards
                .iter()
                .map(|card| card.order_key())
                .collect::<Vec<_>>()
        };
        let previous = keys(&ranked(&popup));
        let order: HashMap<_, _> = previous
            .iter()
            .cloned()
            .enumerate()
            .map(|(index, key)| (key, index))
            .collect();

        popup.searches[0].results.push(listing("fourth"));
        let mut items = ranked(&popup);
        keep_page_order(&mut items, &order, true);
        assert_eq!(keys(&items), previous);
        popup.searches[1].results.push(listing("fifth"));
        let mut items = ranked(&popup);
        keep_page_order(&mut items, &order, false);
        assert_eq!(&keys(&items)[..previous.len()], previous);
        assert_eq!(
            items[previous.len()..]
                .iter()
                .map(|card| card.label().0)
                .collect::<Vec<_>>(),
            ["fourth", "fifth"]
        );
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
        let has_large_image = |output: FullOutput| {
            ctx.tessellate(output.shapes, output.pixels_per_point)
                .iter()
                .any(|primitive| match &primitive.primitive {
                    epaint::Primitive::Mesh(mesh) => {
                        mesh.texture_id == image.id() && mesh.calc_bounds().width() > 650.
                    }
                    epaint::Primitive::Callback(_) => false,
                })
        };
        let pointer = pos2(90., 50.);
        run(0., vec![]);
        run(0.1, vec![Event::PointerMoved(pointer)]);
        run(2.2, vec![]);
        assert!(has_large_image(run(2.3, vec![])));
        for (time, pressed) in [(2.4, true), (2.5, false)] {
            let output = run(
                time,
                vec![Event::PointerButton {
                    pos: pointer,
                    button: PointerButton::Secondary,
                    pressed,
                    modifiers: Modifiers::NONE,
                }],
            );
            assert!(!has_large_image(output));
        }
        let output = run(2.6, vec![]);
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
        assert_eq!(gallery.criteria.media, MediaFilter::Animated);
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
        {
            let mut run = |events| {
                ctx.run(
                    RawInput {
                        screen_rect: Some(screen),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        CentralPanel::default()
                            .frame(Frame::NONE)
                            .show(ctx, |ui| gallery.controls(ui, false));
                    },
                )
            };
            run(vec![]);
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
            let mut output = click(pos2(744., 17.));
            let text_center = |output: &FullOutput, label| {
                output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        Shape::Text(text) if text.galley.text() == label => {
                            Some(text.visual_bounding_rect().center())
                        }
                        _ => None,
                    })
                    .unwrap_or_else(|| panic!("missing filter control {label}"))
            };
            let still = text_center(&output, "Still");
            assert!(text_center(&output, "Any resolution").x > still.x + 100.);
            output = click(still);
            output = click(text_center(&output, "Downloaded"));
            click(text_center(&output, "4K+"));
        }
        assert_eq!(gallery.criteria.media, MediaFilter::All);
        assert_eq!(gallery.criteria.resolution, Resolution::Uhd);
        assert!(gallery.downloaded);
        assert!(gallery.searches.iter().all(|search| !search.queued));
        let items = cards(
            &gallery.searches,
            &gallery.featured,
            &gallery.saved,
            &gallery.criteria,
            gallery.downloaded,
        );
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].label().0, "Saved video.mp4");
    }

    #[test]
    fn keyboard_submits_search_and_escape_dismisses_the_popup() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let mut popup = Gallery::default();
        popup.criteria.media = MediaFilter::All;
        popup.featured.retain(|item| !item.animated());
        let (_local_scan, scan) = channel();
        popup.scan = Some((0, scan));
        for item in &mut popup.featured {
            item.thumbnail = None;
        }
        let _senders: Vec<_> = popup
            .searches
            .iter_mut()
            .map(|search| {
                let (tx, rx) = channel();
                search.request = Some((0, rx));
                tx
            })
            .collect();
        let key = |key| Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(760., 450.));
        let titles: Vec<_> = popup
            .featured
            .iter()
            .take(3)
            .map(|item| item.title.clone())
            .collect();
        let mut run = |events, opening| {
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
        run(Vec::new(), true);
        run(Vec::new(), false);
        let (_, output) = run(Vec::new(), false);
        for label in std::iter::once("Gallery").chain(titles.iter().map(String::as_str)) {
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
        run(vec![Event::Text("mountain".into())], false);
        run(vec![key(Key::Enter)], false);
        assert!(matches!(
            run(vec![key(Key::Escape)], false).0,
            Action::Close
        ));
        assert_eq!(popup.criteria.text, "mountain");
        assert!(popup.searches.iter().all(|search| search.queued));
        assert!(popup.thumbnails.browsing.is_none());
        assert!(popup.searches.iter().all(|search| search.request.is_none()));
    }
}
