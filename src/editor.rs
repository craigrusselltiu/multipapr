mod canvas;
mod chrome;
mod icons;
mod preferences;
use crate::{
    displays, host,
    layout::{Display, Layout, Media, Rect as World, Settings, Upscaling},
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
fn resolution(ui: &mut Ui, w: u32, h: u32) {
    ui.horizontal(|ui| {
        ui.label(RichText::new("Resolution").color(MUTED));
        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
            ui.label(format!("{w} × {h} px"));
        });
    });
}

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
    popup: Option<preferences::SettingsPopup>,
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
        set_fonts(&cc.egui_ctx);
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
        let login = storage::startup_enabled() || settings.media.path.as_os_str().is_empty();
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
            popup: None,
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
        self.message = if storage::mpv().is_err() {
            "Running first-time setup…".into()
        } else {
            String::new()
        };
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
                self.message.clear();
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
                    [102., 34.],
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
        ui.label(RichText::new("Displays").font(heading_font(19.)));
        ui.add_space(3.);
        for (i, d) in self.displays.iter().enumerate() {
            let label = format!("{}    {}", i + 1, d.name);
            if ui
                .add_sized(
                    [ui.available_width(), 36.],
                    Button::new(label)
                        .selected(self.selected == i)
                        .corner_radius(7)
                        .stroke(Stroke::NONE),
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
        resolution(ui, d.px.w as u32, d.px.h as u32);
        ui.add_space(4.);
        ui.separator();
        CollapsingHeader::new(RichText::new("Advanced").font(heading_font(14.)))
            .id_salt(("advanced", &d.id))
            .default_open(false)
            .show_unindented(ui, |ui| self.advanced(ui));
    }
    fn advanced(&mut self, ui: &mut Ui) {
        let i = self.selected;
        let d = &self.displays[i];
        let l = self.layout();
        let mut size = [l.panels[i].w, l.panels[i].h];
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
            self.settings
                .calibration
                .entry(d.id.clone())
                .or_default()
                .size = Some(size);
        }
        let p = l.panels[i];
        let (mut x, mut y) = (p.x, p.y);
        if i > 0 {
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
            ui.add_space(4.);
            ui.label(
                RichText::new(format!("Relative to display {}", j + 1))
                    .small()
                    .color(MUTED),
            );
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
        ui.add_space(5.);
        let reset = ui
            .horizontal(|ui| {
                let reset = ui.button("Reset display").clicked();
                icons::info(ui, "Restores the detected size and position.");
                reset
            })
            .inner;
        if reset {
            self.settings.calibration.remove(&d.id);
            self.reframe();
        }
    }
    fn inspector(&mut self, ui: &mut Ui) {
        if self.tool == Tool::Displays {
            self.calibration(ui);
            return;
        }
        ui.label(RichText::new("Wallpaper").font(heading_font(19.)));
        ui.add_space(4.);
        let label = if self.pending.is_some() {
            "Loading…"
        } else if self.texture.is_some() {
            "Change file…"
        } else {
            "Choose file…"
        };
        ui.horizontal(|ui| {
            if ui.button(label).clicked() {
                self.pick(ui.ctx());
            }
            ui.add_enabled_ui(self.texture.is_some(), |ui| {
                if ui
                    .button("Fill")
                    .on_hover_text("Use the wallpaper’s native size")
                    .clicked()
                {
                    self.settings.scale = 1.;
                    self.settings.pan = [0.; 2];
                    self.reframe();
                }
                if ui
                    .button("Fit")
                    .on_hover_text("Cover all displays")
                    .clicked()
                {
                    self.layout().cover(&mut self.settings);
                    self.reframe();
                }
            });
        });
        ui.add_space(4.);
        let media = &self.settings.media;
        if let Some(name) = media.path.file_name() {
            ui.add(Label::new(RichText::new(name.to_string_lossy()).color(MUTED)).wrap());
            resolution(ui, media.width, media.height);
        }
        if self.texture.is_some() {
            let mut percent = self.settings.scale * 100.;
            if Self::field(ui, "Scale", None, &mut percent, 1.0..=10000.0, " %") {
                self.settings.scale = percent / 100.;
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("Upscaling").color(MUTED));
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    ComboBox::from_id_salt("upscaling")
                        .width(150.)
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
            });
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
        if self.popup.is_none()
            && let Some(path) =
                ctx.input(|i| i.raw.dropped_files.first().and_then(|f| f.path.clone()))
        {
            self.import(path, true, ctx);
        }
        let open_settings = chrome::show(ctx, self.brand.id());
        SidePanel::right("inspector")
            .resizable(false)
            .exact_width(286.)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(18, 14)),
            )
            .show(ctx, |ui| {
                self.sidebar_tools(ui);
                ui.add_space(2.);
                ui.separator();
                ui.add_space(5.);
                ScrollArea::vertical().show(ui, |ui| {
                    self.inspector(ui);
                    let status = self.playback_error.as_deref().unwrap_or(&self.message);
                    if !status.is_empty() {
                        ui.colored_label(ACCENT, status);
                    }
                });
            });
        CentralPanel::default()
            .frame(Frame::new().fill(INK))
            .show(ctx, |ui| self.canvas(ui));
        self.sync(ctx);
        if open_settings && self.popup.is_none() {
            self.popup = Some(preferences::SettingsPopup::default());
        }
        if self.popup.is_some() {
            self.show_preferences(ctx);
        }
    }
    fn on_exit(&mut self, _: Option<&eframe::glow::Context>) {
        if self.saved != self.settings {
            self.persist();
        }
    }
}
pub fn open() -> Result<()> {
    if storage::mpv().is_err() {
        // First run: set up mpv while the window opens and the user picks a wallpaper.
        std::thread::spawn(storage::warm_up_mpv);
    }
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
