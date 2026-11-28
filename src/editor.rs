mod canvas;
mod chrome;
mod gallery;
mod icons;
mod popup;
mod preferences;
use crate::{
    displays, host,
    layout::{
        DEFAULT_PROFILE, Display, Framing, Layout, Media, Profile, Rect as World, Settings,
        Upscaling, Wallpaper, display_set, follow_displays, remember, unique_name,
    },
    shader,
    storage::{self, Preferences},
    update,
};
use anyhow::Result;
use canvas::{Camera, Drag, Tool};
use eframe::egui::{self, *};
use icons::Icon;
use popup::Hint;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, TryRecvError, channel},
    },
    time::{Duration, Instant, SystemTime},
};

const INK: Color32 = Color32::from_rgb(22, 22, 25);
const PANEL: Color32 = Color32::from_rgb(30, 30, 34);
const FIELD: Color32 = Color32::from_rgb(44, 44, 50);
const HOVER: Color32 = Color32::from_rgb(59, 58, 67);
const LINE: Color32 = Color32::from_rgb(57, 57, 64);
const ACCENT: Color32 = Color32::from_rgb(193, 187, 252);
const MUTED: Color32 = Color32::from_rgb(173, 172, 183);
const TEXT: Color32 = Color32::from_gray(235);
const HISTORY: usize = 100;
const INSPECTOR_PADDING: i8 = 18;
const SCROLLBAR_OUTSET: i8 = 6;

fn heading_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("Inter Semibold".into()))
}
fn set_fonts(ctx: &Context) {
    let mut fonts = FontDefinitions::default();
    for (name, bytes, family) in [
        (
            "Inter",
            include_bytes!("../assets/fonts/Inter-Regular.ttf").as_slice(),
            FontFamily::Proportional,
        ),
        (
            "Inter Semibold",
            include_bytes!("../assets/fonts/Inter-SemiBold.ttf").as_slice(),
            heading_font(16.).family,
        ),
    ] {
        fonts
            .font_data
            .insert(name.into(), FontData::from_static(bytes).into());
        let mut fallback = fonts.families[&FontFamily::Proportional].clone();
        fallback.insert(0, name.into());
        fonts.families.insert(family, fallback);
    }
    ctx.set_fonts(fonts);
}
fn note(ui: &mut Ui, text: impl Into<String>, color: Color32) {
    ui.add(Label::new(RichText::new(text).small().color(color)).wrap());
}
fn row<R>(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(Align::Center), control)
            .inner
    })
    .inner
}
fn resolution(ui: &mut Ui, w: u32, h: u32) {
    row(ui, "Resolution", |ui| ui.label(format!("{w} × {h} px")));
}
fn section_heading(ui: &mut Ui, title: &str, detail: Option<String>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).font(heading_font(19.)));
        if let Some(detail) = detail {
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.label(RichText::new(detail).color(MUTED));
            });
        }
    });
    ui.add_space(3.);
}
fn texture(ctx: &Context, image: &image::RgbaImage) -> TextureHandle {
    let size = [image.width() as usize, image.height() as usize];
    ctx.load_texture(
        "wallpaper",
        ColorImage::from_rgba_unmultiplied(size, image.as_raw()),
        TextureOptions::LINEAR,
    )
}
fn playing(settings: &Settings, displays: &[Display]) -> bool {
    !shader::plan(settings, displays).is_empty()
}
fn proportional_size(panel: World, ratio: f32) -> [f32; 2] {
    let ratio = ratio.max(10. / panel.w.min(panel.h));
    [panel.w * ratio, panel.h * ratio]
}

struct PendingImport {
    key: Option<gallery::CardKey>,
    targets: Vec<(Option<String>, World)>,
    result: Receiver<Result<crate::gallery::PreparedWallpaper>>,
}

#[derive(Clone, Copy)]
struct PlaybackTransition {
    requested: SystemTime,
    mode_switch: bool,
}

struct HistoryStep {
    settings: Settings,
    framings: BTreeMap<String, Framing>,
}
impl HistoryStep {
    fn new(settings: &Settings, profiles: &[Profile]) -> Self {
        Self {
            settings: settings.clone(),
            framings: profiles
                .iter()
                .find(|p| Some(&p.name) == settings.profile.as_ref())
                .map(|p| p.framings.clone())
                .unwrap_or_default(),
        }
    }
}
struct ExitState {
    close_to_tray: AtomicBool,
    relaunching: AtomicBool,
}
impl ExitState {
    fn new(close_to_tray: bool) -> Self {
        Self {
            close_to_tray: AtomicBool::new(close_to_tray),
            relaunching: AtomicBool::new(false),
        }
    }
}
struct Editor {
    brand: TextureHandle,
    exe: PathBuf,
    displays: Vec<Display>,
    settings: Settings,
    preferences: Preferences,
    exit: Arc<ExitState>,
    selected: usize,
    tool: Tool,
    drag: Option<(Drag, Pos2, Vec2)>,
    last_pick: Option<Pos2>,
    camera: Camera,
    frame: World,
    saved: Settings,
    sent: Settings,
    last_send: Instant,
    host: Option<u32>,
    textures: HashMap<PathBuf, TextureHandle>,
    loading: HashMap<PathBuf, Receiver<Result<image::RgbaImage>>>,
    failed: HashSet<PathBuf>,
    pending: Vec<PendingImport>,
    login: bool,
    gallery: gallery::Gallery,
    gallery_open: bool,
    popup: Option<preferences::SettingsPopup>,
    closing_popup: Option<popup::ClosingPopup>,
    update: preferences::UpdateState,
    message: String,
    last_check: Instant,
    last_playback_check: Instant,
    playback_transition: Option<PlaybackTransition>,
    playback_error: Option<String>,
    playback: Option<host::PlaybackReport>,
    undo: Vec<HistoryStep>,
    redo: Vec<HistoryStep>,
    committed: HistoryStep,
    edit_focus: Option<Id>,
    profiles: Vec<Profile>,
    profiles_error: Option<String>,
    profiles_dirty: bool,
    profiles_open: bool,
    profile_name: Option<String>,
}
enum ProfileAction {
    Load(usize),
    Create(String),
    Delete(usize),
}

struct EditorStartup {
    displays: Vec<Display>,
    settings: Settings,
    profiles: Result<Vec<Profile>>,
    startup_enabled: bool,
    preferences: Preferences,
}

impl Editor {
    fn new(
        cc: &eframe::CreationContext<'_>,
        exe: PathBuf,
        startup: EditorStartup,
        exit: Arc<ExitState>,
        icon: &IconData,
    ) -> Self {
        let EditorStartup {
            displays,
            mut settings,
            profiles,
            startup_enabled,
            preferences,
        } = startup;
        let (mut profiles, profiles_error) = match profiles {
            Ok(profiles) => (profiles, None),
            Err(e) => (Vec::new(), Some(format!("Could not read profiles · {e:#}"))),
        };
        let original = settings.clone();
        let layout = Layout::new(&displays, &settings.calibration);
        let set = display_set(&displays);
        let mut profiles_dirty = false;
        if profiles_error.is_none() {
            let before = profiles.clone();
            follow_displays(&mut profiles, &mut settings, &set);
            profiles_dirty = profiles != before;
        } else {
            settings.display_set = set;
        }
        settings.restore_multi_wallpapers(&displays, &layout, Path::is_file);
        if playing(&settings, &displays) {
            layout.freeze(&displays, &mut settings);
        }
        chrome::install(cc);
        set_fonts(&cc.egui_ctx);
        let mut visuals = Visuals::dark();
        visuals.panel_fill = PANEL;
        visuals.window_fill = PANEL;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
        visuals.selection.bg_fill = Color32::from_rgb(73, 67, 98);
        visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        for (widget, fill) in [
            (&mut visuals.widgets.inactive, FIELD),
            (&mut visuals.widgets.hovered, HOVER),
            (&mut visuals.widgets.active, Color32::from_rgb(74, 70, 85)),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.bg_stroke = Stroke::NONE;
            widget.corner_radius = CornerRadius::same(6);
            widget.fg_stroke.color = TEXT;
            widget.expansion = 0.;
        }
        visuals.widgets.noninteractive.fg_stroke.color = Color32::from_gray(228);
        cc.egui_ctx.set_visuals(visuals);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        cc.egui_ctx.style_mut(|s| {
            s.explanation_tooltips = false;
            s.spacing.item_spacing = vec2(8., 10.);
            s.spacing.interact_size.y = 36.;
            s.spacing.button_padding = vec2(14., 8.);
            s.text_styles
                .insert(TextStyle::Small, FontId::proportional(12.));
            s.text_styles
                .insert(TextStyle::Body, FontId::proportional(14.));
            s.text_styles
                .insert(TextStyle::Button, FontId::proportional(13.));
        });
        let login = startup_enabled
            || (settings.is_empty()
                && profiles_error.is_none()
                && profiles.iter().all(Profile::is_empty));
        let brand = cc.egui_ctx.load_texture(
            "Fold",
            ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            ),
            TextureOptions::LINEAR.with_mipmap_mode(Some(TextureFilter::Linear)),
        );
        let mut app = Self {
            brand,
            exe,
            displays,
            preferences,
            exit,
            selected: 0,
            tool: Tool::Wallpaper,
            drag: None,
            last_pick: None,
            camera: Camera::default(),
            frame: World::default(),
            saved: original,
            sent: settings.clone(),
            committed: HistoryStep::new(&settings, &profiles),
            last_send: Instant::now(),
            host: std::env::args().nth(2).and_then(|pid| pid.parse().ok()),
            settings,
            textures: HashMap::new(),
            loading: HashMap::new(),
            failed: HashSet::new(),
            pending: Vec::new(),
            login,
            gallery: gallery::Gallery::default(),
            gallery_open: false,
            popup: None,
            closing_popup: None,
            update: preferences::UpdateState::default(),
            message: String::new(),
            last_check: Instant::now(),
            last_playback_check: Instant::now(),
            playback_transition: None,
            playback_error: None,
            playback: None,
            undo: Vec::new(),
            redo: Vec::new(),
            edit_focus: None,
            profiles,
            profiles_error,
            profiles_dirty,
            profiles_open: false,
            profile_name: None,
        };
        app.reframe();
        app
    }
    fn layout(&self) -> Layout {
        Layout::new(&self.displays, &self.settings.calibration)
    }
    fn wallpaper(&self) -> &Wallpaper {
        if self.settings.multi {
            self.settings.shown(&self.displays[self.selected].id)
        } else {
            &self.settings.wallpaper
        }
    }
    fn wallpaper_mut(&mut self) -> &mut Wallpaper {
        if self.settings.multi {
            let id = self.displays[self.selected].id.clone();
            let span = &self.settings.wallpaper;
            self.settings
                .screens
                .entry(id)
                .or_insert_with(|| span.clone())
        } else {
            &mut self.settings.wallpaper
        }
    }
    fn target(&self, layout: &Layout) -> World {
        if self.settings.multi {
            layout.panels[self.selected]
        } else {
            layout.bounds
        }
    }
    fn reframe(&mut self) {
        self.last_pick = None;
        self.camera = Camera::default();
        let l = self.layout();
        let w = self.wallpaper();
        self.frame = if w.is_empty() || !self.shows_full_wallpaper() {
            l.bounds
        } else {
            l.bounds.union(l.media(&self.settings, w))
        };
    }
    fn set_multi(&mut self, on: bool, available: impl Fn(&Path) -> bool) {
        self.settings.multi = on;
        let layout = self.layout();
        self.settings
            .restore_multi_wallpapers(&self.displays, &layout, available);
        if self.message.starts_with("Missing ") {
            self.message.clear();
        }
        self.reframe();
    }
    fn import_target(&self) -> Option<String> {
        self.settings
            .multi
            .then(|| self.displays[self.selected].id.clone())
    }
    fn importing_selected(&self) -> bool {
        let selected = self.import_target();
        self.pending
            .iter()
            .any(|job| job.targets.iter().any(|(target, _)| target == &selected))
    }
    fn cancel_import(&mut self, target: &Option<String>) {
        for job in &mut self.pending {
            job.targets.retain(|(previous, _)| previous != target);
        }
    }
    fn is_animated_wallpaper(&self, path: &Path) -> bool {
        if path.as_os_str().is_empty() {
            return false;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext.eq_ignore_ascii_case("mp4")
                || ext.eq_ignore_ascii_case("m4v")
                || ext.eq_ignore_ascii_case("mov")
                || ext.eq_ignore_ascii_case("mkv")
                || ext.eq_ignore_ascii_case("webm")
                || ext.eq_ignore_ascii_case("ogv")
                || ext.eq_ignore_ascii_case("gif")
            {
                return true;
            }
            if ext.eq_ignore_ascii_case("png")
                || ext.eq_ignore_ascii_case("jpg")
                || ext.eq_ignore_ascii_case("jpeg")
                || ext.eq_ignore_ascii_case("webp")
                || ext.eq_ignore_ascii_case("bmp")
                || ext.eq_ignore_ascii_case("ico")
            {
                return false;
            }
        }
        self.gallery.is_saved_animated(path).unwrap_or(false)
    }
    fn clear_wallpaper(&mut self) {
        self.cancel_import(&self.import_target());
        if !self.wallpaper().is_empty() {
            self.wallpaper_mut().clear();
            self.message.clear();
            self.reframe();
        }
    }
    fn forget_gallery_wallpapers(&mut self, matches: impl Fn(&Path) -> bool) {
        let reframe = matches(&self.wallpaper().media.path);
        self.settings.clear_wallpapers(&matches);
        for profile in &mut self.profiles {
            self.profiles_dirty |= profile.clear_wallpapers(&matches);
        }
        for step in self
            .undo
            .iter_mut()
            .chain(&mut self.redo)
            .chain(std::iter::once(&mut self.committed))
        {
            if matches(&step.settings.wallpaper.media.path) {
                step.framings.clear();
            }
            step.settings.clear_wallpapers(&matches);
        }
        if reframe {
            self.message.clear();
            self.reframe();
        }
    }
    fn remove_from_gallery(&mut self, ctx: &Context, path: Option<PathBuf>) {
        if self.gallery.removing() {
            return;
        }
        let dir = match storage::wallpaper_dir() {
            Ok(dir) => dir,
            Err(error) => {
                self.gallery.set_error(error.to_string());
                return;
            }
        };
        let matches = |assigned: &Path| match &path {
            Some(path) => path == assigned,
            None => storage::in_wallpaper_dir(assigned, &dir),
        };
        self.forget_gallery_wallpapers(matches);
        self.persist();
        if self.settings != self.saved || self.profiles_dirty {
            self.gallery.set_error(self.message.clone());
            return;
        }
        let after = std::fs::metadata(dir.with_file_name("layout.json"))
            .and_then(|info| info.modified())
            .ok();
        self.gallery.remove(ctx, path, after);
    }
    fn import(&mut self, path: PathBuf, ctx: &Context) {
        self.import_from(ctx, self.import_target(), None, move || {
            crate::gallery::import(&path)
        });
    }
    fn import_from(
        &mut self,
        ctx: &Context,
        target: Option<String>,
        key: Option<gallery::CardKey>,
        prepare: impl FnOnce() -> Result<crate::gallery::PreparedWallpaper> + Send + 'static,
    ) {
        if self.gallery.removing() {
            self.message = "Wait for the Gallery to finish deleting wallpapers.".into();
            return;
        }
        let layout = self.layout();
        let frame = if let Some(id) = &target {
            let Some(index) = self.displays.iter().position(|display| &display.id == id) else {
                return;
            };
            layout.panels[index]
        } else {
            layout.bounds
        };
        self.cancel_import(&target);
        self.message.clear();
        self.gallery.clear_error();
        if key.is_some()
            && let Some(job) = self.pending.iter_mut().find(|job| job.key == key)
        {
            job.targets.push((target, frame));
            return;
        }
        let (tx, rx) = channel();
        self.pending.push(PendingImport {
            key,
            targets: vec![(target, frame)],
            result: rx,
        });
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(prepare());
            ctx.request_repaint();
        });
    }
    fn pick(&mut self, ctx: &Context) {
        if let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Wallpapers",
                &[
                    "mp4", "m4v", "mov", "mkv", "webm", "png", "jpg", "jpeg", "webp", "bmp",
                ],
            )
            .pick_file()
        {
            self.import(path, ctx);
        }
    }
    fn collect(&mut self, ctx: &Context) {
        let mut completed = Vec::new();
        self.pending.retain_mut(|job| {
            let result = match job.result.try_recv() {
                Ok(result) => result,
                Err(TryRecvError::Empty) => return true,
                Err(TryRecvError::Disconnected) => Err(anyhow::anyhow!("Wallpaper import stopped")),
            };
            completed.push((job.key.take(), std::mem::take(&mut job.targets), result));
            false
        });
        for (key, targets, result) in completed {
            match result {
                Ok(i) => {
                    let media = i.saved.media.clone();
                    self.gallery.record_saved(i.saved);
                    if targets.is_empty() {
                        continue;
                    }
                    if !self.settings.multi
                        && targets.iter().any(|(target, _)| target.is_none())
                        && let Some(key) = &key
                    {
                        self.gallery.finish_span_apply(ctx, key);
                    }
                    self.message.clear();
                    self.loading.remove(&media.path);
                    self.failed.remove(&media.path);
                    self.textures
                        .insert(media.path.clone(), texture(ctx, &i.image));
                    let layout = self.layout();
                    let pitch = layout.pitch(&self.settings);
                    let selected = self.import_target();
                    let reframe = targets.iter().any(|(target, _)| target == &selected);
                    for (target, original_frame) in targets {
                        let (w, frame) = match target {
                            Some(id) => {
                                let frame = self
                                    .displays
                                    .iter()
                                    .position(|d| d.id == id)
                                    .map_or(original_frame, |index| layout.panels[index]);
                                let span = &self.settings.wallpaper;
                                (
                                    self.settings
                                        .screens
                                        .entry(id)
                                        .or_insert_with(|| span.clone()),
                                    frame,
                                )
                            }
                            None => (&mut self.settings.wallpaper, layout.bounds),
                        };
                        w.media = media.clone();
                        w.paused = false;
                        w.cover(pitch, frame);
                    }
                    layout.freeze(&self.displays, &mut self.settings);
                    if reframe {
                        self.reframe();
                    }
                }
                Err(e) if e.is::<crate::gallery::UnavailableWallpaper>() => {
                    if let Some(key) = &key {
                        self.gallery.omit_unavailable(key);
                    }
                }
                Err(e) if !targets.is_empty() => {
                    let message = if is_missing_file(&e) {
                        let display = targets[0].0.as_ref().and_then(|id| {
                            self.displays.iter().position(|display| &display.id == id)
                        });
                        shader::missing_wallpaper(display, None)
                    } else {
                        format!("{e:#}")
                    };
                    if key.is_some() {
                        self.gallery.set_error(message.clone());
                        if is_missing_file(&e) {
                            self.gallery.refresh_saved(ctx);
                        }
                    }
                    self.message = message;
                }
                Err(_) => {}
            }
        }
    }
    fn missing_preview(&self, path: &Path) -> Option<String> {
        shader::plan(&self.settings, &self.displays)
            .into_iter()
            .find(|part| part.media == path)
            .map(|part| part.missing_wallpaper(self.settings.multi))
    }
    fn load_previews(&mut self, ctx: &Context) {
        let wanted: Vec<_> = self
            .displays
            .iter()
            .map(|d| self.settings.shown(&d.id).media.clone())
            .collect();
        self.loading
            .retain(|path, _| wanted.iter().any(|m| m.path == *path));
        self.textures
            .retain(|path, _| wanted.iter().any(|m| m.path == *path));
        self.failed
            .retain(|path| wanted.iter().any(|m| m.path == *path));
        let (textures, failed, message) = (&mut self.textures, &mut self.failed, &mut self.message);
        let mut missing = None;
        self.loading.retain(|path, rx| match rx.try_recv() {
            Ok(Ok(image)) => {
                textures.insert(path.clone(), texture(ctx, &image));
                false
            }
            Ok(Err(e)) => {
                failed.insert(path.clone());
                if is_missing_file(&e) {
                    missing = Some(path.clone());
                } else {
                    *message = format!("{e:#}");
                }
                false
            }
            Err(TryRecvError::Empty) => true,
            Err(TryRecvError::Disconnected) => false,
        });
        if let Some(path) = missing
            && let Some(message) = self.missing_preview(&path)
        {
            self.message = message;
        }
        for media in wanted {
            let path = &media.path;
            if path.as_os_str().is_empty()
                || self.textures.contains_key(path)
                || self.loading.contains_key(path)
                || self.failed.contains(path)
            {
                continue;
            }
            let (tx, rx) = channel();
            self.loading.insert(path.clone(), rx);
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let image = crate::gallery::saved_thumbnail(&media);
                let _ = tx.send(image);
                ctx.request_repaint();
            });
        }
    }
    fn update_displays(&mut self, displays: Vec<Display>) {
        let changed_set = display_set(&self.displays) != display_set(&displays);
        let selected = self.displays.get(self.selected).map(|d| d.id.clone());
        let added = displays
            .iter()
            .position(|d| !self.displays.iter().any(|o| o.id == d.id));
        self.selected = added
            .or_else(|| {
                displays
                    .iter()
                    .position(|d| Some(&d.id) == selected.as_ref())
            })
            .unwrap_or(0);
        self.drag = None;
        if added.is_some() {
            self.tool = Tool::Displays;
        }
        self.displays = displays;
        let layout = self.layout();
        let before = self.settings.clone();
        if changed_set && self.profiles_error.is_none() {
            let previous = self.profiles.clone();
            follow_displays(
                &mut self.profiles,
                &mut self.settings,
                &display_set(&self.displays),
            );
            self.profiles_dirty |= self.profiles != previous;
        } else if changed_set {
            self.settings.display_set = display_set(&self.displays);
        }
        if changed_set {
            self.undo.clear();
            self.redo.clear();
        }
        self.settings
            .restore_multi_wallpapers(&self.displays, &layout, Path::is_file);
        if playing(&self.settings, &self.displays) {
            layout.freeze(&self.displays, &mut self.settings);
        }
        if before == self.committed.settings || changed_set {
            self.committed = HistoryStep::new(&self.settings, &self.profiles);
        }
        self.reframe();
    }
    fn field(
        ui: &mut Ui,
        label: &str,
        help: Option<&str>,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        suffix: &str,
    ) -> bool {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(MUTED));
            if let Some(help) = help {
                icons::info(ui, help);
            }
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.add_sized(
                    [ui.available_width().min(102.), 30.],
                    DragValue::new(value)
                        .speed(0.5)
                        .range(range)
                        .clamp_existing_to_range(false)
                        .max_decimals(1)
                        .suffix(suffix),
                )
                .changed()
            })
            .inner
        })
        .inner
    }
    fn workspace(&mut self, ctx: &Context) {
        SidePanel::right("inspector")
            .resizable(true)
            .default_width(chrome::SIDEBAR_WIDTH)
            .width_range(260.0..=520.0)
            .frame(Frame::new().fill(PANEL).inner_margin(Margin {
                left: INSPECTOR_PADDING,
                right: INSPECTOR_PADDING - SCROLLBAR_OUTSET,
                top: 14,
                bottom: 14,
            }))
            .show(ctx, |ui| self.sidebar(ui));
        CentralPanel::default()
            .frame(Frame::new().fill(INK))
            .show(ctx, |ui| self.canvas(ui));
    }
    fn sidebar(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 6.;
        ui.spacing_mut().interact_size.y = 30.;
        ui.spacing_mut().button_padding = vec2(10., 5.);
        ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
        ui.spacing_mut().scroll.bar_inner_margin += f32::from(SCROLLBAR_OUTSET);
        let content_width = ui.available_width() - f32::from(SCROLLBAR_OUTSET);
        let mut multi = self.settings.multi;
        let switched = ui
            .horizontal(|ui| {
                ui.set_max_width(content_width);
                ui.set_min_height(34.);
                ui.label(RichText::new("Multi-mode").color(TEXT));
                icons::info(ui, "Give each display its own wallpaper.");
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    let changed =
                        preferences::toggle_switch(ui, &mut multi, "Multi-mode").changed();
                    if let Some(transition) = self.playback_transition {
                        let hint = if transition.mode_switch {
                            "Switching wallpaper layout…"
                        } else {
                            "Applying wallpaper…"
                        };
                        ui.add(Spinner::new().size(16.).color(ACCENT)).hint(hint);
                    }
                    changed
                })
                .inner
            })
            .inner;
        if switched {
            self.set_multi(multi, Path::is_file);
            ui.ctx().request_repaint();
        }
        ui.add_space(2.);
        ui.add_sized(
            [content_width, ui.spacing().item_spacing.y],
            Separator::default().horizontal(),
        );
        ui.add_space(5.);
        ScrollArea::vertical()
            .auto_shrink([false; 2])
            .scroll_bar_visibility(scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width().min(content_width));
                if self.settings.multi {
                    self.displays_section(ui);
                    ui.add_space(4.);
                    ui.separator();
                    self.wallpaper_section(ui);
                } else {
                    self.wallpaper_section(ui);
                    ui.add_space(4.);
                    ui.separator();
                    self.displays_section(ui);
                }
                ui.add_space(4.);
                ui.separator();
                CollapsingHeader::new(RichText::new("Advanced").font(heading_font(14.)))
                    .id_salt(("advanced", &self.displays[self.selected].id))
                    .default_open(false)
                    .show_unindented(ui, |ui| self.advanced(ui));
                if let Some(error) = &self.playback_error {
                    ui.colored_label(ACCENT, error);
                }
                if !self.message.is_empty() {
                    ui.colored_label(ACCENT, &self.message);
                }
            });
    }
    fn file_field(&mut self, ui: &mut Ui) {
        let can_clear = !self.wallpaper().is_empty() || self.importing_selected();
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 36.), Sense::click());
        let response = response.on_hover_cursor(CursorIcon::PointingHand);
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Choose file"));
        let folder = pos2(rect.right() - 20., rect.center().y);
        let clear_rect =
            Rect::from_center_size(pos2(folder.x - 26., rect.center().y), Vec2::splat(24.));
        let clear = can_clear.then(|| {
            let clear_hint = if self.settings.multi {
                "Clear this display’s wallpaper"
            } else {
                "Clear the spanning wallpaper"
            };
            let resp = ui
                .interact(clear_rect, response.id.with("clear"), Sense::click())
                .on_hover_cursor(CursorIcon::PointingHand);
            resp.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, "Clear wallpaper"));
            resp.hint(clear_hint)
        });
        let over_clear = clear
            .as_ref()
            .is_some_and(|resp| resp.hovered() || resp.has_focus());
        let over_field = response.hovered() && !over_clear;
        let draw = ui.painter();
        draw.rect_filled(rect, 6, if over_field { HOVER } else { FIELD });
        if response.has_focus() {
            draw.rect_stroke(rect, 6, Stroke::new(1.0_f32, ACCENT), StrokeKind::Inside);
        }
        let name = self.wallpaper().media.path.file_name();
        let (text, color) = match name {
            _ if self.importing_selected() => ("Loading…".to_owned(), MUTED),
            Some(name) => (name.to_string_lossy().into_owned(), TEXT),
            None => ("Choose a file…".to_owned(), MUTED),
        };
        let left = rect.left() + 12.;
        let text_right = if can_clear {
            clear_rect.left() - 6.
        } else {
            folder.x - 16.
        };
        let mut job = text::LayoutJob::simple_singleline(text, FontId::proportional(13.5), color);
        job.wrap = text::TextWrapping {
            max_width: (text_right - left).max(1.),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('…'),
        };
        let galley = draw.layout_job(job);
        draw.galley(
            pos2(left, rect.center().y - galley.size().y / 2.),
            galley,
            color,
        );
        if let Some(clear_resp) = &clear {
            if over_clear {
                draw.rect_filled(clear_rect, 5, HOVER);
            }
            if clear_resp.has_focus() {
                draw.rect_stroke(
                    clear_rect,
                    5,
                    Stroke::new(1.0_f32, ACCENT),
                    StrokeKind::Inside,
                );
            }
            Icon::Close.paint(draw, clear_rect.center(), if over_clear { TEXT } else { MUTED });
        }
        Icon::Folder.paint(draw, folder, if over_field { TEXT } else { MUTED });
        if clear.is_some_and(|resp| resp.clicked()) {
            self.clear_wallpaper();
        } else if response.clicked() {
            self.pick(ui.ctx());
        }
    }
    fn wallpaper_section(&mut self, ui: &mut Ui) {
        let detail = self
            .settings
            .multi
            .then(|| format!("Display {}", self.selected + 1));
        section_heading(ui, "Wallpaper", detail);
        self.file_field(ui);
        let has = !self.wallpaper().is_empty();
        let animated = has && self.is_animated_wallpaper(&self.wallpaper().media.path);
        ui.horizontal(|ui| {
            let height = ui.spacing().interact_size.y.max(30.);
            let gap = ui.spacing().item_spacing.x;
            let playback_width = if animated { height + gap } else { 0. };
            let button_size = vec2((ui.available_width() - gap - playback_width) / 2., height);
            ui.add_enabled_ui(has, |ui| {
                if animated {
                    let paused = self.wallpaper().paused;
                    let (icon, label, hint) = if paused {
                        (Icon::Play, "Play", "Resume animated wallpaper")
                    } else {
                        (
                            Icon::Pause,
                            "Pause",
                            "Pause animated wallpaper and display the first frame",
                        )
                    };
                    if icon
                        .framed_button(ui, Vec2::splat(height), label)
                        .hint(hint)
                        .clicked()
                    {
                        self.wallpaper_mut().paused = !paused;
                    }
                }
                if ui
                    .add_sized(button_size, Button::new("Fill"))
                    .hint("Use the wallpaper’s native size")
                    .clicked()
                {
                    let target = self.target(&self.layout());
                    self.wallpaper_mut().fill(target);
                    self.reframe();
                }
                let fit_hint = if self.settings.multi {
                    "Cover the display"
                } else {
                    "Cover all displays"
                };
                if ui
                    .add_sized(button_size, Button::new("Fit"))
                    .hint(fit_hint)
                    .clicked()
                {
                    let layout = self.layout();
                    let (pitch, target) = (layout.pitch(&self.settings), self.target(&layout));
                    self.wallpaper_mut().cover(pitch, target);
                    self.reframe();
                }
            });
        });
        if !self.wallpaper().is_empty() {
            let media = self.wallpaper().media.clone();
            resolution(ui, media.width, media.height);
            let mut percent = self.wallpaper().scale * 100.;
            if Self::field(ui, "Scale", None, &mut percent, 1.0..=10000.0, " %") {
                self.wallpaper_mut().scale = percent / 100.;
            }
            row(ui, "Upscaling", |ui| {
                ComboBox::from_id_salt("upscaling")
                    .width(ui.available_width().min(150.))
                    .selected_text(self.settings.upscaling.label())
                    .show_ui(ui, |ui| {
                        for option in Upscaling::ALL {
                            ui.selectable_value(
                                &mut self.settings.upscaling,
                                option,
                                option.label(),
                            );
                        }
                    });
            });
        }
    }
    fn displays_section(&mut self, ui: &mut Ui) {
        section_heading(ui, "Displays", None);
        for i in 0..self.displays.len() {
            let d = &self.displays[i];
            let selected = self.tool == Tool::Displays && self.selected == i;
            let (rect, response) =
                ui.allocate_exact_size(vec2(ui.available_width(), 46.), Sense::click());
            response
                .widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, &d.name));
            let draw = ui.painter();
            let fill = if selected {
                ui.visuals().selection.bg_fill
            } else if response.hovered() {
                HOVER
            } else {
                FIELD
            };
            draw.rect_filled(rect, 6, fill);
            if response.has_focus() {
                draw.rect_stroke(rect, 6, Stroke::new(1.0_f32, ACCENT), StrokeKind::Inside);
            }
            let badge = Rect::from_center_size(rect.left_center() + vec2(24., 0.), vec2(26., 24.));
            draw.rect_filled(badge, 4, if selected { ACCENT } else { PANEL });
            draw.text(
                badge.center(),
                Align2::CENTER_CENTER,
                i + 1,
                heading_font(13.),
                if selected { INK } else { MUTED },
            );
            let left = badge.right() + 10.;
            let label_draw =
                draw.with_clip_rect(draw.clip_rect().intersect(rect.shrink2(vec2(10., 0.))));
            label_draw.text(
                pos2(left, rect.center().y - 8.),
                Align2::LEFT_CENTER,
                &d.name,
                heading_font(13.),
                TEXT,
            );
            let detail = if self.settings.multi && self.settings.shown(&d.id).is_empty() {
                "No wallpaper".to_owned()
            } else {
                format!("{} × {} px", d.px.w as u32, d.px.h as u32)
            };
            label_draw.text(
                pos2(left, rect.center().y + 9.),
                Align2::LEFT_CENTER,
                detail,
                FontId::proportional(12.),
                MUTED,
            );
            if response.on_hover_cursor(CursorIcon::PointingHand).clicked() {
                self.last_pick = None;
                self.selected = i;
                self.tool = Tool::Displays;
            }
        }
        let i = self.selected;
        let d = &self.displays[i];
        if !d.mirrors.is_empty() {
            note(ui, format!("Duplicated on {}", d.mirrors.join(", ")), MUTED);
        }
        let overlaps: Vec<_> = self
            .layout()
            .overlapping(i)
            .iter()
            .map(|j| (j + 1).to_string())
            .collect();
        if !overlaps.is_empty() {
            note(
                ui,
                format!(
                    "Overlaps display{} {}",
                    if overlaps.len() > 1 { "s" } else { "" },
                    overlaps.join(", ")
                ),
                MUTED,
            );
        }
        ui.add_space(4.);
        if ui
            .button("Reset displays")
            .hint("Restores every display’s detected size and position.")
            .clicked()
        {
            self.settings.calibration.clear();
            self.layout().freeze(&self.displays, &mut self.settings);
            self.reframe();
        }
    }
    fn advanced(&mut self, ui: &mut Ui) {
        let i = self.selected;
        let d = &self.displays[i];
        let l = self.layout();
        let ratio = row(ui, "Maintain ratio", |ui| {
            preferences::toggle_switch(ui, &mut self.preferences.maintain_ratio, "Maintain ratio")
                .changed()
        });
        if ratio && let Err(e) = storage::save_preferences(&self.preferences) {
            self.preferences.maintain_ratio = !self.preferences.maintain_ratio;
            self.message = e.to_string();
        }
        let p = l.panels[i];
        let mut size = [p.w, p.h];
        let width = Self::field(
            ui,
            "Panel width",
            Some("Width of the lit screen, excluding the bezel."),
            &mut size[0],
            10.0..=10000.0,
            " mm",
        );
        let height = Self::field(
            ui,
            "Panel height",
            Some("Height of the lit screen, excluding the bezel."),
            &mut size[1],
            10.0..=10000.0,
            " mm",
        );
        if width || height {
            if self.preferences.maintain_ratio {
                let ratio = if width { size[0] / p.w } else { size[1] / p.h };
                size = proportional_size(p, ratio);
            }
            l.resize_panel(&self.displays, &mut self.settings, i, size);
        }
        let (mut x, mut y) = (p.x, p.y);
        if l.parents[i] != i {
            let j = l.parents[i];
            let reference = l.panels[j];
            let a = self.displays[j].px;
            let b = d.px;
            let horizontal =
                (b.x - a.right()).max(a.x - b.right()) >= (b.y - a.bottom()).max(a.y - b.bottom());
            x -= reference.x;
            y -= reference.y;
            if horizontal {
                x -= if b.x >= a.x { reference.w } else { -p.w };
            } else {
                y -= if b.y >= a.y { reference.h } else { -p.h };
            }
        }
        let previous = [x, y];
        let move_x = Self::field(
            ui,
            "Horizontal offset",
            Some("Shifts this display left (−) or right (+)."),
            &mut x,
            -10000.0..=10000.0,
            " mm",
        );
        let move_y = Self::field(
            ui,
            "Vertical offset",
            Some("Shifts this display up (−) or down (+)."),
            &mut y,
            -10000.0..=10000.0,
            " mm",
        );
        if move_x || move_y {
            l.move_panel(
                &self.displays,
                &mut self.settings,
                i,
                [x - previous[0], y - previous[1]],
            );
        }
    }
    fn active_profile(&self) -> Option<usize> {
        let name = self.settings.profile.as_ref()?;
        self.profiles.iter().position(|p| &p.name == name)
    }
    fn load_profile(&mut self, i: usize) -> bool {
        if self.active_profile() == Some(i) {
            return true;
        }
        let layout = self.layout();
        let mut next = self.settings.clone();
        self.profiles[i].apply(&mut next, &layout, &display_set(&self.displays));
        if let Some(missing) = shader::plan(&next, &self.displays)
            .into_iter()
            .find(|p| !p.media.is_file())
        {
            self.message = missing.missing_wallpaper(next.multi);
            return false;
        }
        // The active paths passed validation. Fit the spanning wallpaper to any unassigned displays.
        next.restore_multi_wallpapers(&self.displays, &layout, |_| true);
        self.remember_current();
        self.settings = next;
        self.start_profile();
        true
    }
    fn start_profile(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.edit_focus = None;
        self.committed = HistoryStep::new(&self.settings, &self.profiles);
        self.pending.iter_mut().for_each(|job| job.targets.clear());
        self.message.clear();
        self.reframe();
    }
    fn remember_current(&mut self) {
        if self.profiles_error.is_none() {
            let before = self.profiles.clone();
            remember(
                &mut self.profiles,
                &mut self.settings,
                &display_set(&self.displays),
            );
            self.profiles_dirty |= self.profiles != before;
        }
    }
    fn show_profiles(&mut self, ctx: &Context, anchor: Rect) {
        const WIDTH: f32 = 240.;
        let active = self.active_profile();
        let name_id = Id::new("profile name");
        let mut close = self.profile_name.is_none() && ctx.input(|i| i.key_pressed(Key::Escape));
        let mut action = None;
        let row =
            |ui: &mut Ui| ui.allocate_exact_size(vec2(ui.available_width(), 32.), Sense::click());
        let popup = Area::new(Id::new("profiles"))
            .order(Order::Foreground)
            .fixed_pos(pos2(anchor.right() - WIDTH, anchor.bottom() + 2.))
            .show(ctx, |ui| {
                Frame::new()
                    .fill(PANEL)
                    .stroke(Stroke::new(1.0_f32, LINE))
                    .corner_radius(10)
                    .inner_margin(6)
                    .shadow(ui.visuals().popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(WIDTH - 12.);
                        ui.spacing_mut().item_spacing.y = 2.;
                        if let Some(error) = &self.profiles_error {
                            Frame::new()
                                .inner_margin(8)
                                .show(ui, |ui| note(ui, error.clone(), ACCENT));
                            return;
                        }
                        ScrollArea::vertical()
                            .max_height(
                                (ctx.screen_rect().bottom() - anchor.bottom() - 130.).max(64.),
                            )
                            .show(ui, |ui| {
                                for (i, p) in self.profiles.iter().enumerate() {
                                    let (r, response) = row(ui);
                                    let bin = Rect::from_center_size(
                                        pos2(r.right() - 16., r.center().y),
                                        Vec2::splat(24.),
                                    );
                                    let delete = (p.name != DEFAULT_PROFILE).then(|| {
                                        ui.interact(bin, response.id.with("delete"), Sense::click())
                                    });
                                    let over_bin = delete.as_ref().is_some_and(|d| d.hovered());
                                    let selected = active == Some(i);
                                    if selected || response.hovered() || over_bin {
                                        let fill = if selected {
                                            Color32::from_rgb(56, 53, 71)
                                        } else {
                                            FIELD
                                        };
                                        ui.painter().rect_filled(r, 6, fill);
                                    }
                                    ui.painter_at(r.with_max_x(bin.left() - 2.)).text(
                                        r.left_center() + vec2(10., 0.),
                                        Align2::LEFT_CENTER,
                                        &p.name,
                                        FontId::proportional(13.5),
                                        if selected { ACCENT } else { TEXT },
                                    );
                                    if let Some(delete) = &delete {
                                        if over_bin {
                                            ui.painter().rect_filled(bin, 5, LINE);
                                        }
                                        let tint = if over_bin { Color32::WHITE } else { MUTED };
                                        Icon::Close.paint(ui.painter(), bin.center(), tint);
                                        if delete.clicked() {
                                            action = Some(ProfileAction::Delete(i));
                                            continue;
                                        }
                                    }
                                    if response.clicked() {
                                        action = Some(ProfileAction::Load(i));
                                    }
                                }
                            });
                        ui.add_space(3.);
                        ui.separator();
                        ui.add_space(1.);
                        if let Some(name) = &mut self.profile_name {
                            let field = ui.add(
                                TextEdit::singleline(name)
                                    .id(name_id)
                                    .hint_text("Profile name")
                                    .desired_width(f32::INFINITY)
                                    .margin(Margin::symmetric(10, 9)),
                            );
                            let enter =
                                field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                            let escape =
                                field.lost_focus() && ui.input(|i| i.key_pressed(Key::Escape));
                            let name = name.trim().to_owned();
                            ui.add_space(4.);
                            ui.horizontal(|ui| {
                                let create =
                                    ui.add_enabled(!name.is_empty(), Button::new("Create"));
                                if (create.clicked() || enter) && !name.is_empty() {
                                    action = Some(ProfileAction::Create(name));
                                } else if ui.button("Cancel").clicked() || escape {
                                    self.profile_name = None;
                                }
                            });
                        } else {
                            let (r, response) = row(ui);
                            if response.hovered() {
                                ui.painter().rect_filled(r, 6, FIELD);
                            }
                            Icon::Plus.paint(ui.painter(), r.left_center() + vec2(16., 0.), TEXT);
                            ui.painter().text(
                                r.left_center() + vec2(32., 0.),
                                Align2::LEFT_CENTER,
                                "New profile",
                                FontId::proportional(13.5),
                                TEXT,
                            );
                            if response.clicked() {
                                self.profile_name = Some(String::new());
                                ui.memory_mut(|m| m.request_focus(name_id));
                            }
                        }
                    });
            });
        if ctx.input(|i| i.pointer.any_pressed())
            && let Some(p) = ctx.input(|i| i.pointer.interact_pos())
            && !popup.response.rect.contains(p)
            && !anchor.contains(p)
        {
            close = true;
        }
        match action {
            Some(ProfileAction::Load(i)) => {
                self.load_profile(i);
                close = true;
            }
            Some(ProfileAction::Create(name)) => {
                self.remember_current();
                let mut profile = self.profiles[active.unwrap_or(0)].clone();
                profile.name = unique_name(&self.profiles, &name);
                self.settings.profile = Some(profile.name.clone());
                self.profiles.push(profile);
                self.profiles_dirty = true;
                self.start_profile();
                self.profile_name = None;
            }
            Some(ProfileAction::Delete(i)) if active != Some(i) || self.load_profile(0) => {
                self.profiles.remove(i);
                self.profiles_dirty = true;
            }
            Some(ProfileAction::Delete(_)) | None => {}
        }
        if close {
            self.profiles_open = false;
            self.profile_name = None;
        }
    }
    fn record(&mut self, ctx: &Context) {
        let focus = ctx
            .memory(|m| m.focused())
            .filter(|&id| TextEdit::load_state(ctx, id).is_some());
        if focus != self.edit_focus {
            self.edit_focus = None;
        }
        if self.settings == self.committed.settings || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let settings = HistoryStep::new(&self.settings, &self.profiles);
        if focus.is_none() || focus != self.edit_focus {
            self.undo
                .push(std::mem::replace(&mut self.committed, settings));
            if self.undo.len() > HISTORY {
                self.undo.remove(0);
            }
        } else {
            self.committed = settings;
        }
        self.edit_focus = focus;
        self.redo.clear();
    }
    fn step(&mut self, undo: bool) {
        let (from, to) = if undo {
            (&mut self.undo, &mut self.redo)
        } else {
            (&mut self.redo, &mut self.undo)
        };
        let Some(step) = from.pop() else {
            return;
        };
        let settings = step.settings;
        let id = &self.displays[self.selected].id;
        let reframe = settings.multi != self.settings.multi
            || settings.shown(id).media != self.settings.shown(id).media;
        to.push(HistoryStep::new(&self.settings, &self.profiles));
        self.settings = settings;
        if let Some(i) = self.active_profile() {
            self.profiles[i].media = self.settings.wallpaper.media.clone();
            self.profiles[i].paused = self.settings.wallpaper.paused;
            self.profiles[i].framings = step.framings;
            self.profiles_dirty = true;
        }
        if playing(&self.settings, &self.displays) {
            self.layout().freeze(&self.displays, &mut self.settings);
        }
        self.committed = HistoryStep::new(&self.settings, &self.profiles);
        self.edit_focus = None;
        self.pending.iter_mut().for_each(|job| job.targets.clear());
        if reframe {
            self.reframe();
        }
    }

    fn sync(&mut self, ctx: &Context) {
        if self.saved == self.settings && self.sent == self.settings && !self.profiles_dirty {
            return;
        }
        let dragging = ctx.input(|i| i.pointer.primary_down());
        let interval = Duration::from_millis(33);
        if self.sent != self.settings && (!dragging || self.last_send.elapsed() >= interval) {
            let plan = shader::plan(&self.settings, &self.displays);
            let delivered = match self.host {
                Some(host) if plan == shader::plan(&self.saved, &self.displays) => {
                    let layout = self.layout();
                    let mut delivered = true;
                    for part in &plan {
                        if part.paused {
                            continue;
                        }
                        let parameters =
                            shader::parameters(&layout, &self.settings, &self.displays, part);
                        delivered &= host::update_mapping(
                            &host::player_pipe(host, self.settings.multi, part, &self.displays),
                            host,
                            &parameters,
                        )
                        .is_ok();
                    }
                    delivered
                }
                _ => true,
            };
            if delivered || !dragging {
                self.sent = self.settings.clone();
            }
            self.last_send = Instant::now();
        }
        if dragging && self.sent != self.settings {
            ctx.request_repaint_after(interval);
        } else if !dragging && (self.settings != self.saved || self.profiles_dirty) {
            self.persist();
            if self.playback_transition.is_some() {
                ctx.request_repaint();
            }
        }
    }

    fn persist(&mut self) {
        self.remember_current();
        if self.profiles_dirty {
            if let Err(e) = storage::save_profiles(&self.profiles) {
                self.message = format!("Could not save profiles · {e:#}");
            } else {
                self.profiles_dirty = false;
                if self.message.starts_with("Could not save profiles") {
                    self.message.clear();
                }
            }
        }
        if self.settings == self.saved {
            return;
        }
        match storage::save("layout.json", &self.settings) {
            Ok(()) => {
                if self.message.starts_with("Could not save layout") {
                    self.message.clear();
                }
                if self.saved.is_empty()
                    && playing(&self.settings, &self.displays)
                    && let Err(e) = storage::startup(self.login)
                {
                    self.message = e.to_string();
                }
                let before = shader::plan(&self.saved, &self.displays);
                let after = shader::plan(&self.settings, &self.displays);
                let mode_switch = self.saved.multi != self.settings.multi;
                if mode_switch || before != after {
                    self.playback_transition =
                        if !(before.is_empty() && after.is_empty()) && host::running() {
                            storage::home()
                                .ok()
                                .and_then(|dir| std::fs::metadata(dir.join("layout.json")).ok())
                                .and_then(|info| info.modified().ok())
                                .map(|requested| PlaybackTransition {
                                    requested,
                                    mode_switch,
                                })
                        } else {
                            None
                        };
                }
                self.saved = self.settings.clone();
            }
            Err(e) => self.message = format!("Could not save layout · {e:#}"),
        }
    }
}
impl eframe::App for Editor {
    fn update(&mut self, ctx: &Context, _: &mut eframe::Frame) {
        self.collect(ctx);
        for path in self.gallery.poll_storage(ctx) {
            self.textures.remove(&path);
            self.loading.remove(&path);
            self.failed.remove(&path);
        }
        self.load_previews(ctx);
        self.poll_update(ctx);
        if self.last_check.elapsed() >= Duration::from_secs(3) {
            if let Ok(d) = displays::detect()
                && self.displays != d
            {
                self.update_displays(d);
            }
            self.last_check = Instant::now();
        }
        let poll_interval = if self.playback_transition.is_some() {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(3)
        };
        if self.last_playback_check.elapsed() >= poll_interval {
            let dir = storage::home().ok();
            self.playback_error = dir
                .as_ref()
                .and_then(|dir| std::fs::read_to_string(dir.join("host-error.txt")).ok());
            self.playback = dir
                .and_then(|dir| std::fs::read(dir.join("playback.json")).ok())
                .and_then(|data| serde_json::from_slice::<host::PlaybackReport>(&data).ok())
                .filter(|report| {
                    self.host
                        .map_or_else(host::running, |pid| pid == report.host)
                });
            if let Some(transition) = self.playback_transition
                && (self.playback_error.is_some()
                    || !host::running()
                    || self
                        .playback
                        .as_ref()
                        .is_some_and(|report| report.layout_finished(transition.requested)))
            {
                self.playback_transition = None;
            }
            self.last_playback_check = Instant::now();
        }
        ctx.request_repaint_after(poll_interval);
        if self.popup.is_none()
            && self.closing_popup.is_none()
            && let Some(path) =
                ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()))
        {
            self.import(path, ctx);
        }
        let actions = chrome::show(
            ctx,
            self.brand.id(),
            self.preferences.close_to_tray,
            !self.undo.is_empty(),
            !self.redo.is_empty(),
            self.profiles_open,
            self.gallery_open,
        );
        let modal_open = self.popup.is_some() || self.gallery_open || self.closing_popup.is_some();
        let gallery_opening = actions.gallery && !modal_open;
        if gallery_opening {
            self.gallery_open = true;
            self.profiles_open = false;
            self.profile_name = None;
            self.drag = None;
        }
        if actions.profiles && !modal_open && !gallery_opening {
            self.profiles_open = !self.profiles_open;
            self.profile_name = None;
        }
        let typing = ctx
            .memory(|m| m.focused())
            .is_some_and(|id| text_edit::TextEditState::load(ctx, id).is_some());
        let (mut undo, mut redo) = (actions.undo, actions.redo);
        if !modal_open && !gallery_opening && !typing {
            let shortcut = |modifiers, key| KeyboardShortcut::new(modifiers, key);
            ctx.input_mut(|i| {
                redo |= i
                    .consume_shortcut(&shortcut(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z))
                    || i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Y));
                undo |= i.consume_shortcut(&shortcut(Modifiers::COMMAND, Key::Z));
            });
        }
        if !modal_open && !gallery_opening && !ctx.input(|i| i.pointer.any_down()) {
            if undo {
                self.step(true);
            } else if redo {
                self.step(false);
            }
        }
        self.workspace(ctx);
        if self.profiles_open && self.popup.is_none() {
            self.show_profiles(ctx, actions.profiles_anchor);
        }
        self.sync(ctx);
        self.record(ctx);
        if actions.settings && !modal_open && !gallery_opening {
            self.profiles_open = false;
            self.popup = Some(preferences::SettingsPopup::default());
            self.update.start_check(ctx);
            self.gallery.refresh_saved(ctx);
        }
        let opacity = popup::opacity(ctx, self.popup.is_some() || self.gallery_open);
        if opacity > 0. {
            preferences::blur_backdrop(ctx, opacity);
        }
        if let Some(closing) = &self.closing_popup {
            if opacity > 0. {
                closing.show(ctx, opacity);
            } else {
                self.closing_popup = None;
            }
        } else if self.popup.is_some() {
            self.show_preferences(ctx, opacity);
        } else if self.gallery_open {
            let downloading: HashSet<_> = self
                .pending
                .iter()
                .filter_map(|job| job.key.as_ref())
                .collect();
            let view = gallery::View {
                displays: &self.displays,
                selected: self.selected,
                multi: self.settings.multi,
                downloading: &downloading,
                importing_local: self.pending.iter().any(|job| job.key.is_none()),
            };
            match self.gallery.show(ctx, gallery_opening, opacity, view) {
                gallery::Action::None => {}
                gallery::Action::Close => {
                    self.closing_popup =
                        popup::ClosingPopup::capture(ctx, Id::new(gallery::DIALOG_ID), opacity);
                    self.gallery_open = false;
                    ctx.memory_mut(|memory| memory.request_focus(actions.gallery_id));
                    ctx.request_repaint();
                }
                gallery::Action::Apply(item, target) => {
                    let key = Some(item.key());
                    self.import_from(ctx, target, key, move || item.prepare());
                }
                gallery::Action::Delete(path) => self.remove_from_gallery(ctx, Some(path)),
            }
        }
    }
    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        if self.saved != self.settings || self.profiles_dirty {
            self.persist();
        }
        self.update.cancel();
    }
}

fn is_missing_file(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::NotFound)
    })
}
pub fn open() -> Result<()> {
    let exe = std::env::current_exe()?;
    update::cleanup_old_executables();
    std::thread::spawn(|| {
        if storage::mpv().is_err() {
            storage::warm_up_mpv();
        }
    });
    let displays = displays::detect()?;
    let settings = storage::load_optional()?.unwrap_or_else(|| Settings::new(Media::default()));
    let profiles = storage::load_profiles();
    let preferences = storage::load_preferences();
    let exit = Arc::new(ExitState::new(preferences.close_to_tray));
    let icon = image::load_from_memory_with_format(
        include_bytes!("../multipapr.ico"),
        image::ImageFormat::Ico,
    )?
    .into_rgba8();
    let icon = IconData {
        width: icon.width(),
        height: icon.height(),
        rgba: icon.into_raw(),
    };
    let options = eframe::NativeOptions {
        viewport: ViewportBuilder::default()
            .with_decorations(false)
            .with_inner_size([1160., 760.])
            .with_min_inner_size([760., 450.])
            .with_drag_and_drop(true)
            .with_icon(icon.clone()),
        centered: true,
        ..Default::default()
    };
    let editor_exit = Arc::clone(&exit);
    eframe::run_native(
        "Multipapr",
        options,
        Box::new(move |cc| {
            Ok(Box::new(Editor::new(
                cc,
                exe,
                EditorStartup {
                    displays,
                    settings,
                    profiles,
                    startup_enabled: storage::startup_enabled(),
                    preferences,
                },
                editor_exit,
                &icon,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))?;
    if !exit.relaunching.load(Ordering::Relaxed) && !exit.close_to_tray.load(Ordering::Relaxed) {
        let _ = host::stop();
    }
    Ok(())
}
