use crate::{
    displays,
    layout::{Display, Layout, Media, Rect as World, Settings},
    storage,
};
use anyhow::Result;
use eframe::egui::{self, *};
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

const INK: Color32 = Color32::from_rgb(20, 22, 25);
const PANEL: Color32 = Color32::from_rgb(28, 30, 34);
const LINE: Color32 = Color32::from_rgb(59, 63, 69);
const ACCENT: Color32 = Color32::from_rgb(170, 211, 184);
const MUTED: Color32 = Color32::from_rgb(163, 170, 177);

struct Imported {
    media: Media,
    image: image::RgbaImage,
}
struct Editor {
    displays: Vec<Display>,
    settings: Settings,
    selected: usize,
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
    fn new(cc: &eframe::CreationContext<'_>, displays: Vec<Display>, settings: Settings) -> Self {
        let mut visuals = Visuals::dark();
        visuals.panel_fill = PANEL;
        visuals.window_fill = PANEL;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
        visuals.widgets.inactive.bg_fill = Color32::from_rgb(46, 49, 54);
        visuals.selection.bg_fill = Color32::from_rgb(56, 83, 68);
        cc.egui_ctx.set_visuals(visuals);
        cc.egui_ctx.style_mut(|s| {
            s.spacing.item_spacing = vec2(8., 8.);
            s.spacing.button_padding = vec2(10., 6.);
        });
        let login = storage::startup_enabled() || settings.media.path.as_os_str().is_empty();
        let mut app = Self {
            displays,
            settings,
            selected: 0,
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
            ui.add_sized(
                [102., 20.],
                Label::new(RichText::new(label).color(MUTED)).halign(Align::LEFT),
            );
            ui.add(
                DragValue::new(value)
                    .speed(0.5)
                    .range(range)
                    .max_decimals(1)
                    .suffix(suffix),
            )
            .changed()
        })
        .inner
    }
    fn calibration(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Displays").strong().size(15.));
        for (i, d) in self.displays.iter().enumerate() {
            let label = format!("{}   {}", i + 1, d.name);
            if ui.selectable_label(self.selected == i, label).clicked() {
                self.selected = i;
            }
        }
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
        if d.estimated {
            ui.label(
                RichText::new("Estimated size · measure lit area")
                    .small()
                    .color(ACCENT),
            );
        }
        let width = Self::field(ui, "Panel width", &mut size[0], 10.0..=10000.0, " mm");
        let height = Self::field(ui, "Panel height", &mut size[1], 10.0..=10000.0, " mm");
        let entry = self.settings.calibration.entry(d.id.clone()).or_default();
        if width || height {
            entry.size = Some(size);
        }
        if i > 0 {
            let p = l.panels[i];
            let j = l.parents[i];
            let reference = l.panels[j];
            let a = self.displays[j].px;
            let b = d.px;
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
            if Self::field(ui, label_x, &mut nx, -10000.0..=10000.0, " mm") {
                entry.offset[0] += (nx - x) * sx;
            }
            if Self::field(ui, label_y, &mut ny, -10000.0..=10000.0, " mm") {
                entry.offset[1] += (ny - y) * sy;
            }
        }
        if ui.small_button("Reset display").clicked() {
            self.settings.calibration.remove(&d.id);
            self.reframe();
        }
    }
    fn inspector(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Source").strong().size(15.));
        let media = &self.settings.media;
        if let Some(name) = media.path.file_name() {
            ui.add(Label::new(name.to_string_lossy()).truncate())
                .on_hover_text(media.path.display().to_string());
            ui.label(
                RichText::new(format!("{} × {} px", media.width, media.height))
                    .small()
                    .color(MUTED),
            );
        }
        if ui
            .button(if self.pending.is_some() {
                "Loading…"
            } else {
                "Choose file…"
            })
            .clicked()
        {
            self.pick(ui.ctx());
        }
        if !self.settings.media.path.as_os_str().is_empty() {
            ui.add_space(3.);
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
        }
        ui.add_space(8.);
        ui.separator();
        ui.add_space(6.);
        self.calibration(ui);
        ui.add_space(8.);
        ui.separator();
        ui.add_space(6.);
        if ui.checkbox(&mut self.login, "Start at sign-in").changed() {
            if let Err(e) = storage::startup(self.login) {
                self.login = !self.login;
                self.message = e.to_string();
            }
        }
    }
    fn canvas(&mut self, ui: &mut Ui) {
        let (area, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let draw = ui.painter_at(area);
        draw.rect_filled(area, 0., INK);
        let step = 22.;
        for x in 0..(area.width() / step) as i32 + 1 {
            for y in 0..(area.height() / step) as i32 + 1 {
                draw.circle_filled(
                    area.min + vec2(x as f32 * step, y as f32 * step),
                    0.8,
                    Color32::from_gray(52),
                );
            }
        }
        let l = self.layout();
        let factor = ((area.width() - 96.).max(32.) / self.frame.w)
            .min((area.height() - 96.).max(32.) / self.frame.h);
        let origin = area.center()
            - vec2(
                self.frame.x + self.frame.w / 2.,
                self.frame.y + self.frame.h / 2.,
            ) * factor;
        let rect = |w: World| {
            Rect::from_min_size(origin + vec2(w.x, w.y) * factor, vec2(w.w, w.h) * factor)
        };
        let media = rect(l.media(&self.settings));
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1., 1.));
        if let Some(texture) = &self.texture {
            draw.image(texture.id(), media, uv, Color32::from_gray(97));
        }
        for (i, &p) in l.panels.iter().enumerate() {
            let screen = rect(p);
            draw.rect_filled(screen, 0., Color32::BLACK);
            if let Some(texture) = &self.texture {
                draw.with_clip_rect(screen.intersect(area)).image(
                    texture.id(),
                    media,
                    uv,
                    Color32::WHITE,
                );
            }
            let tint = if i == self.selected {
                ACCENT
            } else {
                Color32::LIGHT_GRAY
            };
            draw.rect_stroke(screen, 2., Stroke::new(1.5_f32, tint), StrokeKind::Inside);
            let badge = Rect::from_min_size(screen.min + vec2(6., 6.), vec2(21., 20.));
            draw.rect_filled(badge, 3., Color32::from_black_alpha(195));
            draw.text(
                badge.center(),
                Align2::CENTER_CENTER,
                (i + 1).to_string(),
                FontId::monospace(11.),
                tint,
            );
            if response.clicked()
                && response
                    .interact_pointer_pos()
                    .is_some_and(|pt| screen.contains(pt))
            {
                self.selected = i;
            }
        }
        if self.texture.is_some() {
            let handle = Rect::from_center_size(media.right_bottom(), vec2(16., 16.));
            let resize = ui
                .interact(handle.intersect(area), ui.id().with("size"), Sense::drag())
                .on_hover_cursor(CursorIcon::ResizeNwSe);
            draw.rect_stroke(
                media,
                0.,
                Stroke::new(1.0_f32, Color32::from_white_alpha(88)),
                StrokeKind::Inside,
            );
            draw.rect_filled(handle.shrink(4.), 2., ACCENT);
            if resize.dragged() {
                let delta = resize.drag_delta();
                let diagonal = media.size();
                let scale = (self.settings.scale
                    * (1. + delta.dot(diagonal) / diagonal.length_sq()))
                .clamp(0.01, 100.);
                let growth = diagonal * ((scale / self.settings.scale - 1.) / (2. * factor));
                self.settings.pan[0] += growth.x;
                self.settings.pan[1] += growth.y;
                self.settings.scale = scale;
            } else if response.dragged() {
                let d = response.drag_delta() / factor;
                self.settings.pan[0] += d.x;
                self.settings.pan[1] += d.y;
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            } else if response.hovered() {
                ui.ctx().set_cursor_icon(CursorIcon::Grab);
            }
            if response.double_clicked() {
                self.reframe();
            }
        } else if self.pending.is_none() {
            let button = Rect::from_center_size(area.center(), vec2(150., 32.));
            if ui.put(button, Button::new("Choose wallpaper…")).clicked() {
                self.pick(ui.ctx());
            }
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
        TopBottomPanel::top("bar")
            .exact_height(54.)
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let (icon, _) = ui.allocate_exact_size(vec2(28., 26.), Sense::hover());
                    for (x, y, w, h) in [(0., 8., 16., 11.), (17., 2., 9., 17.)] {
                        ui.painter().rect_stroke(
                            Rect::from_min_size(icon.min + vec2(x, y), vec2(w, h)),
                            1.,
                            Stroke::new(1.6_f32, ACCENT),
                            StrokeKind::Inside,
                        );
                    }
                    ui.label(RichText::new("Multipapr").strong().size(19.));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        let btn = Button::new(RichText::new("Apply wallpaper").strong().color(INK))
                            .fill(ACCENT);
                        if ui
                            .add_enabled(self.texture.is_some() && self.pending.is_none(), btn)
                            .clicked()
                        {
                            let result = storage::apply(&self.settings)
                                .and_then(|()| storage::startup(self.login));
                            self.message = match result {
                                Ok(()) => "Layout saved".into(),
                                Err(e) => e.to_string(),
                            };
                        }
                        let status = self.playback_error.as_deref().unwrap_or(&self.message);
                        if !status.is_empty() {
                            ui.add(Label::new(RichText::new(status).small()).truncate())
                                .on_hover_text(status);
                        }
                    });
                });
            });
        SidePanel::right("inspector")
            .resizable(false)
            .exact_width(262.)
            .frame(Frame::new().fill(PANEL).inner_margin(16))
            .show(ctx, |ui| {
                ScrollArea::vertical().show(ui, |ui| self.inspector(ui));
            });
        CentralPanel::default()
            .frame(Frame::new().fill(INK))
            .show(ctx, |ui| self.canvas(ui));
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
            .with_inner_size([1060., 690.])
            .with_min_inner_size([720., 480.])
            .with_drag_and_drop(true)
            .with_icon(icon),
        ..Default::default()
    };
    eframe::run_native(
        "Multipapr",
        options,
        Box::new(move |cc| Ok(Box::new(Editor::new(cc, displays, settings)))),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
