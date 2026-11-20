mod canvas;
mod chrome;
use crate::{
    displays, host,
    layout::{Display, Layout, Media, Rect as World, Settings},
    storage,
};
use anyhow::Result;
use canvas::{Camera, Drag, Tool};
use eframe::egui::{self, *};
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

const INK: Color32 = Color32::from_rgb(22, 22, 25);
const PANEL: Color32 = Color32::from_rgb(30, 30, 34);
const FIELD: Color32 = Color32::from_rgb(44, 44, 50);
const LINE: Color32 = Color32::from_rgb(57, 57, 64);
const ACCENT: Color32 = Color32::from_rgb(193, 187, 252);
const MUTED: Color32 = Color32::from_rgb(173, 172, 183);

struct Imported {
    media: Media,
    image: image::RgbaImage,
}
struct Editor {
    brand: TextureHandle,
    displays: Vec<Display>,
    settings: Settings,
    selected: usize,
    tool: Tool,
    drag: Option<(Drag, Pos2)>,
    camera: Camera,
    saved: Settings,
    sent: Settings,
    last_send: Instant,
    player_pipe: Option<PathBuf>,
    texture: Option<TextureHandle>,
    pending: Option<Receiver<Result<Imported>>>,
    new_source: bool,
    frame: World,
    login: bool,
    message: String,
    last_check: Instant,
    playback_error: Option<String>,
}
impl Editor {
    fn new(
        cc: &eframe::CreationContext<'_>,
        displays: Vec<Display>,
        settings: Settings,
        icon: &IconData,
    ) -> Self {
        chrome::install(cc);
        if let Some(dir) = std::env::var_os("WINDIR") {
            let path = PathBuf::from(dir).join("Fonts/segoeui.ttf");
            if let Ok(bytes) = std::fs::read(path) {
                let mut fonts = FontDefinitions::default();
                fonts
                    .font_data
                    .insert("Segoe UI".into(), FontData::from_owned(bytes).into());
                fonts
                    .families
                    .entry(FontFamily::Proportional)
                    .or_default()
                    .insert(0, "Segoe UI".into());
                cc.egui_ctx.set_fonts(fonts);
            }
        }
        let mut visuals = Visuals::dark();
        visuals.panel_fill = PANEL;
        visuals.window_fill = PANEL;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
        visuals.selection.bg_fill = Color32::from_rgb(73, 67, 98);
        visuals.selection.stroke = Stroke::new(1.0_f32, ACCENT);
        for (widget, fill) in [
            (&mut visuals.widgets.inactive, FIELD),
            (&mut visuals.widgets.hovered, Color32::from_rgb(59, 58, 67)),
            (&mut visuals.widgets.active, Color32::from_rgb(74, 70, 85)),
        ] {
            widget.bg_fill = fill;
            widget.weak_bg_fill = fill;
            widget.bg_stroke = Stroke::NONE;
            widget.corner_radius = CornerRadius::same(6);
            widget.fg_stroke.color = Color32::from_gray(235);
            widget.expansion = 0.;
        }
        visuals.widgets.noninteractive.fg_stroke.color = Color32::from_gray(228);
        cc.egui_ctx.set_visuals(visuals);
        cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = false);
        cc.egui_ctx.style_mut(|s| {
            s.spacing.item_spacing = vec2(8., 8.);
            s.spacing.interact_size.y = 28.;
            s.spacing.button_padding = vec2(10., 5.);
            s.text_styles
                .insert(TextStyle::Small, FontId::proportional(12.));
            s.text_styles
                .insert(TextStyle::Body, FontId::proportional(14.));
            s.text_styles
                .insert(TextStyle::Button, FontId::proportional(13.));
        });
        let login = storage::startup_enabled() || settings.media.path.as_os_str().is_empty();
        let brand = cc.egui_ctx.load_texture(
            "Seam",
            ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            ),
            TextureOptions::LINEAR,
        );
        let mut app = Self {
            brand,
            displays,
            selected: 0,
            tool: Tool::Wallpaper,
            drag: None,
            camera: Camera::default(),
            saved: settings.clone(),
            sent: settings.clone(),
            last_send: Instant::now(),
            player_pipe: std::env::args()
                .nth(2)
                .and_then(|pid| pid.parse().ok())
                .map(host::player_pipe),
            settings,
            texture: None,
            pending: None,
            new_source: false,
            frame: World::default(),
            login,
            message: String::new(),
            last_check: Instant::now(),
            playback_error: None,
        };
        app.reframe();
        if !app.settings.media.path.as_os_str().is_empty() {
            app.import(app.settings.media.path.clone(), false, &cc.egui_ctx);
        }
        app
    }
    fn layout(&self) -> Layout {
        Layout::new(&self.displays, &self.settings.calibration)
    }
    fn reframe(&mut self) {
        self.camera = Camera::default();
        let l = self.layout();
        self.frame = if self.settings.media.path.as_os_str().is_empty() {
            l.bounds
        } else {
            l.bounds.union(l.media(&self.settings))
        };
    }
    fn import(&mut self, path: PathBuf, new_source: bool, ctx: &Context) {
        self.new_source = new_source;
        let (tx, rx) = channel();
        self.pending = Some(rx);
        self.message.clear();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| {
                let media = storage::probe(&path)?;
                let image = storage::preview(&media)?;
                Ok(Imported { media, image })
            })();
            let _ = tx.send(result);
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
            self.import(path, true, ctx);
        }
    }
    fn collect(&mut self, ctx: &Context) {
        let Some(result) = self.pending.as_ref().and_then(|r| r.try_recv().ok()) else {
            return;
        };
        self.pending = None;
        match result {
            Ok(i) => {
                self.settings.media = i.media;
                if self.new_source {
                    self.settings.scale = 1.;
                    self.settings.pan = [0.; 2];
                }
                let size = [i.image.width() as usize, i.image.height() as usize];
                self.texture = Some(ctx.load_texture(
                    "wallpaper",
                    ColorImage::from_rgba_unmultiplied(size, i.image.as_raw()),
                    TextureOptions::LINEAR,
                ));
                self.reframe();
            }
            Err(e) => self.message = format!("{e:#}"),
        }
    }
    fn field(
        ui: &mut Ui,
        label: &str,
        value: &mut f32,
        range: std::ops::RangeInclusive<f32>,
        suffix: &str,
    ) -> bool {
        ui.horizontal(|ui| {
            ui.label(RichText::new(label).color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.add_sized(
                    [88., 28.],
                    DragValue::new(value)
                        .speed(0.5)
                        .range(range)
                        .max_decimals(1)
                        .suffix(suffix),
                )
                .changed()
            })
            .inner
        })
        .inner
    }
    fn calibration(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Displays").size(19.).strong());
        ui.add_space(3.);
        for (i, d) in self.displays.iter().enumerate() {
            let label = format!("{}    {}", i + 1, d.name);
            if ui
                .add_sized(
                    [ui.available_width(), 31.],
                    Button::new(label).selected(self.selected == i),
                )
                .clicked()
            {
                self.selected = i;
                self.tool = Tool::Displays;
            }
        }
        ui.add_space(8.);
        let i = self.selected;
        let d = &self.displays[i];
        ui.label(
            RichText::new(format!(
                "{:.0} × {:.0} px{}",
                d.px.w,
                d.px.h,
                if d.primary { "  ·  Primary" } else { "" }
            ))
            .small()
            .color(MUTED),
        );
        let l = self.layout();
        let mut size = [l.panels[i].w, l.panels[i].h];
        ui.add_space(4.);
        if d.estimated
            && self
                .settings
                .calibration
                .get(&d.id)
                .and_then(|c| c.size)
                .is_none()
        {
            ui.label(
                RichText::new("Estimated size · measure lit area")
                    .small()
                    .color(ACCENT),
            );
        }
        let width = Self::field(ui, "Panel width", &mut size[0], 10.0..=10000.0, " mm");
        let height = Self::field(ui, "Panel height", &mut size[1], 10.0..=10000.0, " mm");
        if width || height {
            self.settings
                .calibration
                .entry(d.id.clone())
                .or_default()
                .size = Some(size);
        }
        if i > 0 {
            let p = l.panels[i];
            let j = l.parents[i];
            let reference = l.panels[j];
            let a = reference;
            let b = p;
            let horizontal =
                (b.x - a.right()).max(a.x - b.right()) >= (b.y - a.bottom()).max(a.y - b.bottom());
            ui.add_space(4.);
            ui.label(
                RichText::new(format!("Relative to display {}", j + 1))
                    .small()
                    .color(MUTED),
            );
            let (label_x, label_y, x, y, sx, sy) = if horizontal {
                let right = b.x >= a.x;
                (
                    "Horizontal gap",
                    "Top offset",
                    if right {
                        p.x - reference.right()
                    } else {
                        reference.x - p.right()
                    },
                    p.y - reference.y,
                    if right { 1. } else { -1. },
                    1.,
                )
            } else {
                let below = b.y >= a.y;
                (
                    "Left offset",
                    "Vertical gap",
                    p.x - reference.x,
                    if below {
                        p.y - reference.bottom()
                    } else {
                        reference.y - p.bottom()
                    },
                    1.,
                    if below { 1. } else { -1. },
                )
            };
            let (mut nx, mut ny) = (x, y);
            let changed_x = Self::field(ui, label_x, &mut nx, -10000.0..=10000.0, " mm");
            let changed_y = Self::field(ui, label_y, &mut ny, -10000.0..=10000.0, " mm");
            if changed_x || changed_y {
                l.move_panel(
                    &self.displays,
                    &mut self.settings,
                    i,
                    [(nx - x) * sx, (ny - y) * sy],
                );
            }
        }
        ui.add_space(5.);
        if ui
            .button("Reset display")
            .on_hover_text("Use the detected size and placement for this display")
            .clicked()
        {
            self.settings.calibration.remove(&d.id);
            self.reframe();
        }
    }
    fn inspector(&mut self, ui: &mut Ui) {
        if self.tool == Tool::Displays {
            self.calibration(ui);
            return;
        }
        ui.label(RichText::new("Wallpaper").size(19.).strong());
        ui.add_space(4.);
        let media = &self.settings.media;
        if let Some(name) = media.path.file_name() {
            ui.add(Label::new(RichText::new(name.to_string_lossy()).size(14.)).wrap())
                .on_hover_text(media.path.display().to_string());
            let ext = media
                .path
                .extension()
                .unwrap_or_default()
                .to_string_lossy()
                .to_uppercase();
            ui.label(
                RichText::new(format!("{ext}   ·   {} × {}", media.width, media.height))
                    .small()
                    .color(MUTED),
            );
        }
        let label = if self.pending.is_some() {
            "Loading…"
        } else if self.texture.is_some() {
            "Change file…"
        } else {
            "Choose file…"
        };
        if ui.button(label).clicked() {
            self.pick(ui.ctx());
        }
        if self.texture.is_some() {
            ui.add_space(12.);
            ui.separator();
            ui.add_space(10.);
            ui.label(RichText::new("Placement").strong().size(15.));
            let mut percent = self.settings.scale * 100.;
            if Self::field(ui, "Scale", &mut percent, 1.0..=10000.0, " %") {
                self.settings.scale = percent / 100.;
            }
            ui.horizontal(|ui| {
                if ui.button("Native size").clicked() {
                    self.settings.scale = 1.;
                    self.settings.pan = [0.; 2];
                    self.reframe();
                }
                if ui.button("Cover").clicked() {
                    self.layout().cover(&mut self.settings);
                    self.reframe();
                }
            });
            ui.add_space(15.);
            ui.separator();
            ui.add_space(10.);
            ui.label(RichText::new("Displays").strong().size(15.));
            for (i, d) in self.displays.iter().enumerate() {
                let label = format!("{}    {}  ×  {}", i + 1, d.px.w as u32, d.px.h as u32);
                if ui
                    .add(Button::new(RichText::new(label).color(MUTED)).frame(false))
                    .on_hover_text(&d.name)
                    .clicked()
                {
                    self.selected = i;
                    self.tool = Tool::Displays;
                }
            }
        }
    }

    fn sync(&mut self, ctx: &Context) {
        if self.saved == self.settings && self.sent == self.settings {
            return;
        }
        let dragging = ctx.input(|i| i.pointer.primary_down());
        let interval = Duration::from_millis(33);
        if self.sent != self.settings && (!dragging || self.last_send.elapsed() >= interval) {
            let delivered = match &self.player_pipe {
                Some(pipe)
                    if self.settings.media.path == self.saved.media.path
                        && !self.settings.media.path.as_os_str().is_empty() =>
                {
                    host::update_mapping(pipe, &self.layout(), &self.settings).is_ok()
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
        } else if !dragging && self.settings != self.saved {
            self.persist();
        }
    }

    fn persist(&mut self) {
        let result = storage::save("layout.json", &self.settings);
        match result {
            Ok(()) => {
                if self.saved.media.path.as_os_str().is_empty()
                    && !self.settings.media.path.as_os_str().is_empty()
                {
                    if let Err(e) = storage::startup(self.login) {
                        self.message = e.to_string();
                    }
                }
                self.saved = self.settings.clone();
            }
            Err(e) => self.message = format!("Could not save layout · {e}"),
        }
    }
}
impl eframe::App for Editor {
    fn update(&mut self, ctx: &Context, _: &mut eframe::Frame) {
        self.collect(ctx);
        if self.last_check.elapsed() >= Duration::from_secs(3) {
            if let Ok(d) = displays::detect() {
                if self.displays != d {
                    self.displays = d;
                    self.selected = 0;
                    self.reframe();
                }
            }
            self.playback_error = storage::home()
                .ok()
                .and_then(|dir| std::fs::read_to_string(dir.join("host-error.txt")).ok());
            self.last_check = Instant::now();
        }
        ctx.request_repaint_after(Duration::from_secs(3));
        if let Some(path) = ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()))
        {
            self.import(path, true, ctx);
        }
        chrome::show(ctx, self.brand.id());
        SidePanel::right("inspector")
            .resizable(false)
            .exact_width(250.)
            .frame(Frame::new().fill(PANEL).inner_margin(18))
            .show(ctx, |ui| {
                ui.with_layout(egui::Layout::bottom_up(Align::Min), |ui| {
                    if ui.checkbox(&mut self.login, "Start at sign-in").changed() {
                        if let Err(e) = storage::startup(self.login) {
                            self.login = !self.login;
                            self.message = e.to_string();
                        }
                    }
                    let status = self.playback_error.as_deref().unwrap_or(&self.message);
                    if !status.is_empty() {
                        ui.colored_label(ACCENT, status);
                    }
                    ui.separator();
                    ui.with_layout(egui::Layout::top_down(Align::Min), |ui| {
                        ScrollArea::vertical().show(ui, |ui| self.inspector(ui));
                    });
                });
            });
        CentralPanel::default()
            .frame(Frame::new().fill(INK))
            .show(ctx, |ui| {
                self.toolbar(ui);
                self.canvas(ui);
            });
        self.sync(ctx);
    }
    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        if self.saved != self.settings {
            self.persist();
        }
    }
}
pub fn open() -> Result<()> {
    let displays = displays::detect()?;
    let settings = storage::load().unwrap_or_else(|_| {
        Settings::new(Media {
            path: PathBuf::new(),
            width: 1920,
            height: 1080,
        })
    });
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
            .with_inner_size([1080., 680.])
            .with_min_inner_size([760., 450.])
            .with_drag_and_drop(true)
            .with_icon(icon.clone()),
        ..Default::default()
    };
    eframe::run_native(
        "Multipapr",
        options,
        Box::new(move |cc| Ok(Box::new(Editor::new(cc, displays, settings, &icon)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
