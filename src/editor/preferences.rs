use super::{
    ACCENT, Editor, FIELD, INK, LINE, MUTED, PANEL, gallery::Gallery, heading_font, icons::Icon,
    popup, storage,
};
use crate::{
    host, process,
    update::{self, CheckOutcome, Release},
};
use anyhow::{Context as _, Result};
use eframe::{
    egui::*,
    egui_glow::CallbackFn,
    glow::{self, HasContext},
};
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant},
};
use tempfile::NamedTempFile;
use windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;

const MODAL_WIDTH: f32 = 560.;
const DIALOG_ID: &str = "settings dialog";
const CARD_BORDER: Color32 = Color32::from_rgb(47, 47, 54);
const SUBTEXT: Color32 = Color32::from_rgb(154, 153, 165);
const DELETE_FILL: Color32 = Color32::from_rgb(174, 54, 72);

#[derive(Default)]
pub(super) struct SettingsPopup {
    error: Option<String>,
    confirm_clear: bool,
}

struct DownloadProgress {
    downloaded: AtomicU64,
    total: AtomicU64,
}

enum UpdatePhase {
    Idle,
    Checking(Receiver<Result<CheckOutcome>>),
    Latest,
    Available(Release),
    Updating {
        release: Release,
        progress: Arc<DownloadProgress>,
        cancel: Arc<AtomicBool>,
        rx: Receiver<Result<NamedTempFile>>,
    },
    CheckFailed(String),
}

pub(super) struct UpdateState {
    phase: UpdatePhase,
    error: Option<String>,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            phase: UpdatePhase::Idle,
            error: None,
        }
    }
}

impl UpdateState {
    pub(super) fn start_check(&mut self, ctx: &Context) {
        if matches!(self.phase, UpdatePhase::Updating { .. }) {
            return;
        }
        self.error = None;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(update::check_latest());
            ctx.request_repaint();
        });
        self.phase = UpdatePhase::Checking(rx);
    }

    fn start_update(&mut self, release: Release, exe: &Path, ctx: &Context) {
        let Some(target_dir) = exe.parent().map(Path::to_path_buf) else {
            self.error = Some("Cannot determine Multipapr folder".into());
            self.phase = UpdatePhase::Available(release);
            return;
        };
        self.error = None;
        let progress = Arc::new(DownloadProgress {
            downloaded: AtomicU64::new(0),
            total: AtomicU64::new(release.size.unwrap_or(0)),
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let thread_release = release.clone();
        let thread_progress = Arc::clone(&progress);
        let thread_cancel = Arc::clone(&cancel);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut last_repaint = Instant::now() - Duration::from_secs(1);
            let result = update::download_update(
                &thread_release,
                &target_dir,
                &thread_cancel,
                |done, total| {
                    thread_progress.downloaded.store(done, Ordering::Relaxed);
                    if let Some(total) = total {
                        thread_progress.total.store(total, Ordering::Relaxed);
                    }
                    if last_repaint.elapsed() >= Duration::from_millis(80) {
                        last_repaint = Instant::now();
                        ctx.request_repaint();
                    }
                },
            );
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.phase = UpdatePhase::Updating {
            release,
            progress,
            cancel,
            rx,
        };
    }

    pub(super) fn cancel(&mut self) {
        if let UpdatePhase::Updating { cancel, .. } = &self.phase {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    fn error_message(&self) -> Option<&str> {
        self.error.as_deref().or(match &self.phase {
            UpdatePhase::CheckFailed(msg) => Some(msg.as_str()),
            _ => None,
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum UpdateAction {
    None,
    Recheck,
    Install(Release),
    ClearGallery,
}

pub(super) fn blur_backdrop(ctx: &Context, opacity: f32) {
    ctx.layer_painter(LayerId::background()).add(PaintCallback {
        rect: ctx.screen_rect(),
        callback: Arc::new(CallbackFn::new(move |info, painter| {
            blur_editor(info, painter, opacity);
        })),
    });
}

fn blur_editor(info: PaintCallbackInfo, painter: &eframe::egui_glow::Painter, opacity: f32) {
    let gl = painter.gl();
    let [width, height] = info.screen_size_px.map(|n| n as i32);
    if width == 0 || height == 0 {
        return;
    }
    let screen = (painter.intermediate_fbo(), width, height);
    let depth = (16. * info.pixels_per_point).log2().round().clamp(1., 6.) as usize;
    let reduction = ((1 << depth) as f32).powf(opacity);
    let mut levels = Vec::with_capacity(depth + 1);
    levels.push(screen);
    let mut buffers = Vec::with_capacity(depth);
    unsafe {
        let previous_buffer = gl.get_parameter_renderbuffer(glow::RENDERBUFFER_BINDING);
        gl.disable(glow::SCISSOR_TEST);
        let blit = |(src, sw, sh), (dst, dw, dh)| {
            gl.bind_framebuffer(glow::READ_FRAMEBUFFER, src);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, dst);
            gl.blit_framebuffer(
                0,
                0,
                sw,
                sh,
                0,
                0,
                dw,
                dh,
                glow::COLOR_BUFFER_BIT,
                glow::LINEAR,
            );
        };
        for step in 1..=depth {
            let scale = ((1 << step) as f32).min(reduction);
            let previous = *levels.last().unwrap();
            let w = (width as f32 / scale).round().max(1.) as i32;
            let h = (height as f32 / scale).round().max(1.) as i32;
            if (w, h) == (previous.1, previous.2) {
                break;
            }
            let Ok(fbo) = gl.create_framebuffer() else {
                break;
            };
            let Ok(buffer) = gl.create_renderbuffer() else {
                gl.delete_framebuffer(fbo);
                break;
            };
            buffers.push((fbo, buffer));
            gl.bind_renderbuffer(glow::RENDERBUFFER, Some(buffer));
            gl.renderbuffer_storage(glow::RENDERBUFFER, glow::RGBA8, w, h);
            gl.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(fbo));
            gl.framebuffer_renderbuffer(
                glow::DRAW_FRAMEBUFFER,
                glow::COLOR_ATTACHMENT0,
                glow::RENDERBUFFER,
                Some(buffer),
            );
            if gl.check_framebuffer_status(glow::DRAW_FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                break;
            }
            let level = (Some(fbo), w, h);
            blit(previous, level);
            levels.push(level);
            if scale >= reduction {
                break;
            }
        }
        for pair in levels.windows(2).rev() {
            blit(pair[1], pair[0]);
        }
        gl.bind_framebuffer(glow::FRAMEBUFFER, screen.0);
        gl.bind_renderbuffer(glow::RENDERBUFFER, previous_buffer);
        gl.enable(glow::SCISSOR_TEST);
        for (fbo, buffer) in buffers {
            gl.delete_framebuffer(fbo);
            gl.delete_renderbuffer(buffer);
        }
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    Color32::from_rgb(
        lerp(a.r() as f32..=b.r() as f32, t).round() as u8,
        lerp(a.g() as f32..=b.g() as f32, t).round() as u8,
        lerp(a.b() as f32..=b.b() as f32, t).round() as u8,
    )
}

pub(super) fn toggle_switch(ui: &mut Ui, on: &mut bool, label: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(vec2(38., 22.), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response
        .widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, label));
    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, *on);
        let radius = 0.5 * rect.height();
        let track_off = if response.hovered() {
            Color32::from_rgb(66, 66, 75)
        } else {
            Color32::from_rgb(52, 52, 60)
        };
        let track_on = if response.hovered() {
            Color32::from_rgb(208, 203, 254)
        } else {
            ACCENT
        };
        ui.painter()
            .rect_filled(rect, radius, lerp_color(track_off, track_on, how_on));
        if response.has_focus() {
            ui.painter().rect_stroke(
                rect.expand(1.5),
                radius + 1.5,
                Stroke::new(1.5_f32, ACCENT),
                StrokeKind::Outside,
            );
        }
        let knob_fill = lerp_color(Color32::from_rgb(176, 175, 186), INK, how_on);
        let center_x = lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        ui.painter()
            .circle_filled(pos2(center_x, rect.center().y), radius - 3., knob_fill);
    }
    response.on_hover_cursor(CursorIcon::PointingHand)
}

pub(super) fn hairline(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0_f32, CARD_BORDER),
    );
}

fn section_heading(ui: &mut Ui, title: &str) {
    ui.label(
        RichText::new(title)
            .font(heading_font(15.))
            .color(Color32::from_gray(242)),
    );
    ui.add_space(8.);
}

fn settings_card<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    let inner_width = (ui.available_width() - 2.).max(160.);
    Frame::new()
        .fill(PANEL)
        .corner_radius(10)
        .stroke(Stroke::new(1.0_f32, CARD_BORDER))
        .inner_margin(0)
        .show(ui, |ui| {
            ui.set_min_width(inner_width);
            ui.set_max_width(inner_width);
            ui.spacing_mut().item_spacing.y = 0.;
            add_contents(ui)
        })
        .inner
}

fn setting_row<R>(
    ui: &mut Ui,
    title: &str,
    description: &str,
    control_size: Vec2,
    control: impl FnOnce(&mut Ui) -> R,
) -> R {
    let row_width = ui.available_width();
    Frame::new()
        .inner_margin(Margin::symmetric(18, 14))
        .show(ui, |ui| {
            let inner_width = (row_width - 36.).max(160.);
            ui.set_min_width(inner_width);
            ui.set_max_width(inner_width);
            let gap = 20.;
            let text_width = (inner_width - control_size.x - gap).max(120.);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.;
                let text = ui.allocate_ui_with_layout(
                    vec2(text_width, 0.),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.set_min_width(text_width);
                        ui.set_max_width(text_width);
                        ui.spacing_mut().item_spacing.y = 3.;
                        ui.label(
                            RichText::new(title)
                                .font(heading_font(13.5))
                                .color(Color32::from_gray(242)),
                        );
                        ui.add(
                            Label::new(
                                RichText::new(description)
                                    .font(FontId::proportional(12.5))
                                    .color(SUBTEXT),
                            )
                            .wrap(),
                        );
                    },
                );
                let row_height = text.response.rect.height().max(control_size.y);
                ui.allocate_ui_with_layout(
                    vec2(gap + control_size.x, row_height),
                    Layout::right_to_left(Align::Center),
                    control,
                )
                .inner
            })
            .inner
        })
        .inner
}

fn playback_setting(
    ui: &mut Ui,
    title: &str,
    description: &str,
    value: &mut storage::PlaybackAction,
    default: &str,
) {
    use storage::PlaybackAction::*;
    let options = [
        (Continue, default),
        (Pause, "Pause all"),
        (Stop, "Stop and free memory"),
    ];
    let selected = options
        .iter()
        .find(|(action, _)| action == value)
        .unwrap()
        .1;
    let width = 200.;
    setting_row(ui, title, description, vec2(width, 36.), |ui| {
        ComboBox::from_id_salt(title)
            .width(width)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                for (action, label) in options {
                    ui.selectable_value(value, action, label);
                }
            });
    });
}

fn render_update_card(ui: &mut Ui, update: &UpdateState) -> UpdateAction {
    let title = format!("Multipapr {}", update::current_version_label());
    let description = match &update.phase {
        UpdatePhase::Idle | UpdatePhase::Checking(_) => "Checking for updates…".into(),
        UpdatePhase::Latest => "You’re on the latest release.".into(),
        UpdatePhase::Available(release) => {
            if let Some(error) = &update.error {
                error.clone()
            } else {
                format!("{} is available to install.", release.version.display())
            }
        }
        UpdatePhase::Updating {
            release, progress, ..
        } => {
            let done = progress.downloaded.load(Ordering::Relaxed);
            let total = progress.total.load(Ordering::Relaxed);
            if total > 0 {
                let pct = done
                    .saturating_mul(100)
                    .checked_div(total)
                    .unwrap_or(0)
                    .min(100);
                format!("Downloading {}… {pct}%", release.version.display())
            } else {
                format!("Downloading {}…", release.version.display())
            }
        }
        UpdatePhase::CheckFailed(_) => update
            .error_message()
            .unwrap_or("Could not check for updates.")
            .into(),
    };

    let mut action = UpdateAction::None;
    settings_card(ui, |ui| {
        let button_size = vec2(98., 32.);
        setting_row(ui, &title, &description, button_size, |ui| {
            match &update.phase {
                UpdatePhase::Idle | UpdatePhase::Checking(_) => {
                    ui.add_enabled(
                        false,
                        Button::new(RichText::new("Checking…").color(SUBTEXT))
                            .fill(INK)
                            .stroke(Stroke::new(1.0_f32, CARD_BORDER))
                            .corner_radius(7)
                            .min_size(button_size),
                    );
                }
                UpdatePhase::Latest => {
                    ui.add_enabled(
                        false,
                        Button::new(RichText::new("Latest").color(SUBTEXT))
                            .fill(INK)
                            .stroke(Stroke::new(1.0_f32, CARD_BORDER))
                            .corner_radius(7)
                            .min_size(button_size),
                    );
                }
                UpdatePhase::Available(release) => {
                    let button = Button::new(RichText::new("Update").color(Color32::WHITE))
                        .selected(true)
                        .corner_radius(7)
                        .min_size(button_size);
                    if ui
                        .add(button)
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .on_hover_text(format!("Update to {}", release.version.display()))
                        .clicked()
                    {
                        action = UpdateAction::Install(release.clone());
                    }
                }
                UpdatePhase::Updating { .. } => {
                    ui.add_enabled(
                        false,
                        Button::new(RichText::new("Updating…").color(SUBTEXT))
                            .fill(INK)
                            .stroke(Stroke::new(1.0_f32, CARD_BORDER))
                            .corner_radius(7)
                            .min_size(button_size),
                    );
                }
                UpdatePhase::CheckFailed(_) => {
                    let button = Button::new("Retry")
                        .fill(FIELD)
                        .stroke(Stroke::new(1.0_f32, LINE))
                        .corner_radius(7)
                        .min_size(button_size);
                    if ui
                        .add(button)
                        .on_hover_cursor(CursorIcon::PointingHand)
                        .clicked()
                    {
                        action = UpdateAction::Recheck;
                    }
                }
            }
        });
        if let UpdatePhase::Updating { progress, .. } = &update.phase {
            let done = progress.downloaded.load(Ordering::Relaxed);
            let total = progress.total.load(Ordering::Relaxed);
            if total > 0 {
                let fraction = (done as f32 / total as f32).clamp(0., 1.);
                Frame::new()
                    .inner_margin(Margin {
                        left: 18,
                        right: 18,
                        top: 0,
                        bottom: 14,
                    })
                    .show(ui, |ui| {
                        let (bar, _) =
                            ui.allocate_exact_size(vec2(ui.available_width(), 3.), Sense::hover());
                        ui.painter().rect_filled(bar, 1.5, FIELD);
                        let filled = Rect::from_min_size(
                            bar.min,
                            vec2(bar.width() * fraction, bar.height()),
                        );
                        ui.painter().rect_filled(filled, 1.5, ACCENT);
                    });
            }
        }
    });
    action
}

fn apply_update_and_relaunch(exe: &Path, staged: NamedTempFile) -> Result<()> {
    update::replace_executable(exe, staged)?;
    if !host::relaunch()? {
        let child = process::spawn(storage::command(exe).arg("--editor"))
            .context("Cannot relaunch Multipapr")?;
        unsafe {
            AllowSetForegroundWindow(child.id());
        }
    }
    Ok(())
}

fn settings_dialog<R>(
    ctx: &Context,
    opacity: f32,
    contents: impl FnOnce(&mut Ui) -> R,
) -> ModalResponse<(bool, scroll_area::ScrollAreaOutput<R>)> {
    let bounds = ctx.screen_rect().shrink(16.);
    let width = MODAL_WIDTH.min(bounds.width() - 2.);
    popup::dialog(
        Id::new(DIALOG_ID),
        vec2(width + 2., bounds.height()),
        Margin::ZERO,
        opacity,
    )
    .show(ctx, |ui| {
        ui.multiply_opacity(opacity);
        ui.set_width(width);
        ui.set_max_height(bounds.height() - 2.);
        ui.spacing_mut().item_spacing.y = 0.;
        let close = popup::header(ui, "Settings", |ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                Icon::Close.button(ui, "Close settings", false).clicked()
            })
            .inner
        });
        hairline(ui);
        let scroll = Frame::new()
            .inner_margin(Margin::symmetric(12, 12))
            .show(ui, |ui| {
                ui.visuals_mut().clip_rect_margin = 0.;
                ui.spacing_mut().scroll = style::ScrollStyle::solid();
                ScrollArea::vertical()
                    .id_salt("settings body")
                    .scroll_bar_visibility(scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        Frame::new()
                            .inner_margin(Margin::symmetric(10, 6))
                            .show(ui, contents)
                            .inner
                    })
            })
            .inner;
        (close, scroll)
    })
}

impl SettingsPopup {
    fn contents(
        &mut self,
        ui: &mut Ui,
        login: &mut bool,
        preferences: &mut storage::Preferences,
        update: &UpdateState,
        gallery: &Gallery,
        importing: bool,
    ) -> UpdateAction {
        section_heading(ui, "General");
        settings_card(ui, |ui| {
            let toggle_size = vec2(38., 22.);
            if setting_row(
                ui,
                "Start at sign-in",
                "Launch Multipapr in the background when you sign in to Windows.",
                toggle_size,
                |ui| toggle_switch(ui, login, "Start at sign-in"),
            )
            .changed()
            {
                self.error = storage::startup(*login).err().map(|e| {
                    *login = !*login;
                    e.to_string()
                });
            }
            hairline(ui);
            if setting_row(
                ui,
                "Close to system tray",
                "Keep the wallpaper running in the tray when you close this window.",
                toggle_size,
                |ui| toggle_switch(ui, &mut preferences.close_to_tray, "Close to system tray"),
            )
            .changed()
            {
                self.error = storage::save_preferences(preferences).err().map(|e| {
                    preferences.close_to_tray = !preferences.close_to_tray;
                    e.to_string()
                });
            }
        });
        ui.add_space(18.);
        section_heading(ui, "Playback");
        let previous = preferences.clone();
        settings_card(ui, |ui| {
            playback_setting(
                ui,
                "Fullscreen game detected",
                "When a foreground app fills a display.",
                &mut preferences.fullscreen,
                "Play visible displays",
            );
            hairline(ui);
            playback_setting(
                ui,
                "Battery saver",
                "When Windows Energy Saver is on.",
                &mut preferences.battery_saver,
                "Keep playing",
            );
        });
        if *preferences != previous
            && let Err(error) = storage::save_preferences(preferences)
        {
            *preferences = previous;
            self.error = Some(error.to_string());
        }
        if let Some(error) = &self.error {
            ui.add_space(8.);
            ui.add(Label::new(RichText::new(error).small().color(MUTED)).wrap());
        }
        ui.add_space(18.);
        section_heading(ui, "Gallery");
        let clear = settings_card(ui, |ui| {
            const DELETE_WIDTH: f32 = 76.;
            const CANCEL_WIDTH: f32 = 68.;
            const BUTTON_GAP: f32 = 10.;
            const BUTTON_HEIGHT: f32 = 30.;
            let confirming = self.confirm_clear;
            let size = gallery_size(gallery.bytes());
            let description = if confirming {
                format!("Delete {size} of downloaded and imported copies?")
            } else if gallery.removing() {
                "Deleting wallpapers…".into()
            } else if gallery.scanning() && gallery.bytes() == 0 {
                "Calculating size…".into()
            } else {
                format!("{size} stored")
            };
            let can_clear = gallery.bytes() > 0 && !importing && !gallery.removing();
            let width = DELETE_WIDTH
                + if confirming {
                    CANCEL_WIDTH + BUTTON_GAP
                } else {
                    0.
                };
            setting_row(
                ui,
                "Clear gallery",
                &description,
                vec2(width, BUTTON_HEIGHT),
                |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = BUTTON_GAP;
                        ui.spacing_mut().interact_size.y = BUTTON_HEIGHT;
                        let delete = ui
                            .scope(|ui| {
                                let widgets = &mut ui.visuals_mut().widgets;
                                for (state, color) in [
                                    (&mut widgets.inactive, DELETE_FILL),
                                    (&mut widgets.hovered, Color32::from_rgb(194, 69, 84)),
                                    (&mut widgets.active, Color32::from_rgb(149, 43, 61)),
                                ] {
                                    state.weak_bg_fill = color;
                                    state.fg_stroke.color = Color32::WHITE;
                                }
                                ui.add_enabled(
                                    can_clear,
                                    Button::new("Delete")
                                        .min_size(vec2(DELETE_WIDTH, BUTTON_HEIGHT)),
                                )
                                .clicked()
                            })
                            .inner;
                        let cancel = confirming
                            && ui
                                .add_sized(vec2(CANCEL_WIDTH, BUTTON_HEIGHT), Button::new("Cancel"))
                                .clicked();
                        if delete || cancel {
                            self.confirm_clear = delete && !confirming;
                        }
                        delete && confirming
                    })
                    .inner
                },
            )
        });
        if let Some(error) = gallery.error() {
            ui.label(RichText::new(error).small().color(ACCENT));
        }
        ui.add_space(18.);
        section_heading(ui, "About");
        let update = render_update_card(ui, update);
        if clear {
            UpdateAction::ClearGallery
        } else {
            update
        }
    }
}

fn gallery_size(bytes: u64) -> String {
    let mut size = bytes as f64;
    let mut unit = "B";
    for next in ["KB", "MB", "GB", "TB"] {
        if size < 1024. {
            break;
        }
        size /= 1024.;
        unit = next;
    }
    if unit == "B" {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {unit}")
    }
}

impl Editor {
    pub(super) fn poll_update(&mut self, ctx: &Context) {
        let check_result = match &self.update.phase {
            UpdatePhase::Checking(rx) => rx.try_recv().ok(),
            _ => None,
        };
        if let Some(result) = check_result {
            match result {
                Ok(CheckOutcome::Latest) => {
                    self.update.phase = UpdatePhase::Latest;
                    self.update.error = None;
                }
                Ok(CheckOutcome::Available(release)) => {
                    self.update.phase = UpdatePhase::Available(release);
                    self.update.error = None;
                }
                Err(error) => {
                    self.update.phase = UpdatePhase::CheckFailed(format!("{error:#}"));
                    self.update.error = None;
                }
            }
            return;
        }

        let download_result = match &self.update.phase {
            UpdatePhase::Updating { rx, .. } => rx.try_recv().ok(),
            _ => None,
        };
        let Some(result) = download_result else {
            return;
        };
        let UpdatePhase::Updating { release, .. } =
            std::mem::replace(&mut self.update.phase, UpdatePhase::Idle)
        else {
            return;
        };
        match result {
            Ok(staged) => {
                if self.saved != self.settings {
                    self.persist();
                }
                match apply_update_and_relaunch(&self.exe, staged) {
                    Ok(()) => {
                        self.exit.relaunching.store(true, Ordering::Relaxed);
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    Err(error) => {
                        self.update.error = Some(format!("{error:#}"));
                        self.update.phase = UpdatePhase::Available(release);
                    }
                }
            }
            Err(error) => {
                self.update.error = Some(format!("{error:#}"));
                self.update.phase = UpdatePhase::Available(release);
            }
        }
    }

    pub(super) fn show_preferences(&mut self, ctx: &Context, opacity: f32) {
        let popup = self.popup.as_mut().unwrap();
        let response = settings_dialog(ctx, opacity, |ui| {
            popup.contents(
                ui,
                &mut self.login,
                &mut self.preferences,
                &self.update,
                &self.gallery,
                !self.pending.is_empty(),
            )
        });
        self.exit
            .close_to_tray
            .store(self.preferences.close_to_tray, Ordering::Relaxed);
        let close = response.inner.0 || response.should_close();
        match response.inner.1.inner {
            UpdateAction::None => {}
            UpdateAction::Recheck => self.update.start_check(ctx),
            UpdateAction::ClearGallery => self.remove_from_gallery(ctx, None),
            UpdateAction::Install(release) => {
                if self.saved != self.settings {
                    self.persist();
                }
                self.update.start_update(release, &self.exe, ctx);
            }
        }
        if close {
            self.closing_popup = popup::ClosingPopup::capture(ctx, Id::new(DIALOG_ID), opacity);
            self.popup = None;
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::Version;

    fn run_update_card(state: &UpdateState) -> FullOutput {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600., 300.))),
                ..Default::default()
            },
            |ctx| {
                CentralPanel::default().show(ctx, |ui| {
                    ui.set_width(MODAL_WIDTH);
                    let _ = render_update_card(ui, state);
                });
            },
        )
    }

    #[test]
    fn settings_fit_at_normal_size_and_scroll_only_while_space_is_limited() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        ctx.style_mut(|s| {
            s.spacing.item_spacing = vec2(8., 10.);
            s.spacing.interact_size.y = 36.;
            s.spacing.button_padding = vec2(14., 8.);
        });
        let mut popup = SettingsPopup::default();
        let mut prefs = storage::Preferences::default();
        let mut login = false;
        let update = UpdateState {
            phase: UpdatePhase::Latest,
            error: None,
        };
        let mut frame = |size: Vec2| {
            let mut result = None;
            let output = ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                    ..Default::default()
                },
                |ctx| {
                    let modal = settings_dialog(ctx, 1., |ui| {
                        popup.contents(
                            ui,
                            &mut login,
                            &mut prefs,
                            &update,
                            &Gallery::default(),
                            false,
                        )
                    });
                    result = Some((modal.response.rect, modal.inner.1));
                },
            );
            let (dialog, body) = result.unwrap();
            (dialog, body, output.shapes)
        };
        for (size, overflow) in [
            (vec2(1080., 680.), false),
            (vec2(760., 450.), true),
            (vec2(1080., 680.), false),
        ] {
            for _ in 0..3 {
                frame(size);
            }
            let (dialog, body, shapes) = frame(size);
            assert!(
                Rect::from_min_size(Pos2::ZERO, size)
                    .shrink(15.)
                    .contains_rect(dialog),
                "{dialog:?} in {size:?}"
            );
            for label in [
                "General",
                "Fullscreen game detected",
                "Battery saver",
                "About",
                "Latest",
            ] {
                let shape = shapes.iter().find_map(|shape| match &shape.shape {
                    epaint::Shape::Text(text) if text.galley.text() == label => {
                        Some((text.visual_bounding_rect(), shape.clip_rect))
                    }
                    _ => None,
                });
                if !overflow {
                    let (rect, clip) = shape.unwrap_or_else(|| panic!("missing {label}"));
                    assert!(
                        clip.intersect(dialog.shrink(10.)).contains_rect(rect),
                        "{label} at {rect:?}, clipped to {clip:?} in {dialog:?}"
                    );
                } else if let Some((_, clip)) = shape {
                    assert!(
                        clip.bottom() <= dialog.bottom() - 10.,
                        "body escapes {dialog:?}"
                    );
                }
            }
            assert_eq!(
                body.content_size.y > body.inner_rect.height() + 1.,
                overflow,
                "{:?} vs {:?}",
                body.content_size,
                body.inner_rect
            );
            if overflow {
                let mut scrolled = body.state;
                scrolled.offset.y = body.content_size.y;
                scrolled.store(&ctx, body.id);
                assert!(frame(size).1.state.offset.y > 0.);
            } else {
                assert_eq!(body.state.offset.y, 0.);
            }
        }
    }

    #[test]
    fn gallery_delete_is_red_and_spaced_from_cancel() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let mut popup = SettingsPopup {
            confirm_clear: true,
            ..Default::default()
        };
        let mut gallery = Gallery::default();
        gallery.record_saved(crate::gallery::SavedWallpaper {
            media: crate::layout::Media {
                path: "example.png".into(),
                ..Default::default()
            },
            title: "Saved wallpaper".into(),
            origin: None,
            animated: false,
            bytes: 1024,
        });
        let mut login = false;
        let mut prefs = storage::Preferences::default();
        let update = UpdateState {
            phase: UpdatePhase::Latest,
            error: None,
        };
        let mut draw = || {
            ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1080., 680.))),
                    ..Default::default()
                },
                |ctx| {
                    settings_dialog(ctx, 1., |ui| {
                        popup.contents(ui, &mut login, &mut prefs, &update, &gallery, false)
                    });
                },
            )
        };
        draw();
        let output = draw();
        let button_text = |label| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Text(text) if text.galley.text() == label => {
                        Some(text.visual_bounding_rect())
                    }
                    _ => None,
                })
                .unwrap()
        };
        let delete = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Rect(rect) if rect.fill == DELETE_FILL => Some(rect.rect),
                _ => None,
            })
            .unwrap();
        let cancel_text = button_text("Cancel");
        let cancel = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Rect(rect)
                    if (rect.rect.width() - 68.).abs() < 1.
                        && rect.rect.contains_rect(cancel_text) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .unwrap();
        assert!(delete.contains_rect(button_text("Delete")));
        assert!(delete.left() - cancel.right() >= 9.);
    }

    #[test]
    fn latest_state_renders_disabled_latest_button_and_version() {
        let state = UpdateState {
            phase: UpdatePhase::Latest,
            error: None,
        };
        let out = run_update_card(&state);
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(
            texts
                .iter()
                .any(|t| t == &format!("Multipapr {}", update::current_version_label()))
        );
        assert!(texts.iter().any(|t| t == "Latest"));
    }

    #[test]
    fn available_state_renders_update_button_and_target_version() {
        let state = UpdateState {
            phase: UpdatePhase::Available(Release {
                version: Version::parse("9.9.9").unwrap(),
                download_url:
                    "https://github.com/craigrusselltiu/multipapr/releases/download/v9.9.9/multipapr.exe"
                        .into(),
                size: None,
                sha256: None,
            }),
            error: None,
        };
        let out = run_update_card(&state);
        let texts: Vec<String> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                epaint::Shape::Text(t) => Some(t.galley.text().to_owned()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.contains("v9.9.9")));
        assert!(texts.iter().any(|t| t == "Update"));
    }

    #[test]
    fn toggle_switch_flips_on_click() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let mut enabled = true;
        let mut center = Pos2::ZERO;
        let run = |events, state: &mut bool, center: &mut Pos2| {
            let _ = ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(200., 100.))),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    CentralPanel::default().show(ctx, |ui| {
                        let resp = toggle_switch(ui, state, "Close to system tray");
                        *center = resp.rect.center();
                    });
                },
            );
        };
        run(vec![], &mut enabled, &mut center);
        run(
            vec![
                Event::PointerMoved(center),
                Event::PointerButton {
                    pos: center,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
            &mut enabled,
            &mut center,
        );
        run(
            vec![Event::PointerButton {
                pos: center,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
            &mut enabled,
            &mut center,
        );
        assert!(!enabled);
    }

    #[test]
    fn update_button_is_clickable_only_when_newer_release_is_available() {
        let click_button = |state: &UpdateState| -> UpdateAction {
            let ctx = Context::default();
            super::super::set_fonts(&ctx);
            let frame = |events| -> (Option<Pos2>, UpdateAction) {
                let mut action = UpdateAction::None;
                let out = ctx.run(
                    RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(600., 300.))),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        CentralPanel::default().show(ctx, |ui| {
                            ui.set_width(MODAL_WIDTH);
                            action = render_update_card(ui, state);
                        });
                    },
                );
                let center = out.shapes.iter().find_map(|shape| {
                    if let epaint::Shape::Text(t) = &shape.shape {
                        let text = t.galley.text();
                        if text == "Latest" || text == "Update" {
                            return Some(t.pos + t.galley.size() * 0.5);
                        }
                    }
                    None
                });
                (center, action)
            };
            let button_center = frame(vec![]).0.expect("button label rendered");
            let _ = frame(vec![
                Event::PointerMoved(button_center),
                Event::PointerButton {
                    pos: button_center,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ]);
            frame(vec![Event::PointerButton {
                pos: button_center,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }])
            .1
        };

        let latest = UpdateState {
            phase: UpdatePhase::Latest,
            error: None,
        };
        assert_eq!(click_button(&latest), UpdateAction::None);

        let release = Release {
            version: Version::parse("9.9.9").unwrap(),
            download_url:
                "https://github.com/craigrusselltiu/multipapr/releases/download/v9.9.9/multipapr.exe"
                    .into(),
            size: None,
            sha256: None,
        };
        let available = UpdateState {
            phase: UpdatePhase::Available(release.clone()),
            error: None,
        };
        assert_eq!(click_button(&available), UpdateAction::Install(release));
    }

    #[test]
    fn cards_span_full_width_and_rows_have_even_vertical_padding() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        ctx.style_mut(|s| s.spacing.interact_size.y = 36.);
        let mut general_card = Rect::NOTHING;
        let mut about_card = Rect::NOTHING;
        let mut toggle_rect = Rect::NOTHING;
        let mut on = true;
        let state = UpdateState {
            phase: UpdatePhase::Latest,
            error: None,
        };
        let mut run_frame = || {
            ctx.run(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800., 600.))),
                    ..Default::default()
                },
                |ctx| {
                    Modal::new(Id::new("test settings"))
                        .frame(Frame::new().inner_margin(0))
                        .show(ctx, |ui| {
                            ui.set_min_width(MODAL_WIDTH);
                            ui.set_max_width(MODAL_WIDTH);
                            Frame::new()
                                .inner_margin(Margin::symmetric(22, 18))
                                .show(ui, |ui| {
                                    let body_width = MODAL_WIDTH - 44.;
                                    ui.set_min_width(body_width);
                                    ui.set_max_width(body_width);
                                    ui.spacing_mut().item_spacing.y = 0.;
                                    general_card = ui
                                        .scope(|ui| {
                                            settings_card(ui, |ui| {
                                                toggle_rect = setting_row(
                                                    ui,
                                                    "Close to system tray",
                                                    "Short line.",
                                                    vec2(38., 22.),
                                                    |ui| toggle_switch(ui, &mut on, "Toggle"),
                                                )
                                                .rect;
                                            });
                                        })
                                        .response
                                        .rect;
                                    about_card = ui
                                        .scope(|ui| {
                                            let _ = render_update_card(ui, &state);
                                        })
                                        .response
                                        .rect;
                                });
                        });
                },
            )
        };
        let _ = run_frame();
        let out = run_frame();
        let expected_width = MODAL_WIDTH - 44.;
        assert!(
            (general_card.width() - expected_width).abs() < 1.0,
            "general card width {} != {expected_width}",
            general_card.width()
        );
        assert!(
            (about_card.width() - expected_width).abs() < 1.0,
            "about card width {} != {expected_width}",
            about_card.width()
        );
        let title_pos = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                epaint::Shape::Text(t) if t.galley.text() == "Close to system tray" => {
                    Some(t.pos.y)
                }
                _ => None,
            })
            .expect("title rendered");
        let desc_bottom = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                epaint::Shape::Text(t) if t.galley.text() == "Short line." => {
                    Some(t.pos.y + t.galley.size().y)
                }
                _ => None,
            })
            .expect("description rendered");
        let top_pad = title_pos - general_card.top();
        let bottom_pad = general_card.bottom() - desc_bottom;
        assert!(
            (top_pad - bottom_pad).abs() < 2.0,
            "top padding ({top_pad}) and bottom padding ({bottom_pad}) should match"
        );
        let text_center_y = (title_pos + desc_bottom) * 0.5;
        assert!(
            (toggle_rect.center().y - text_center_y).abs() < 2.0,
            "toggle ({}) should be vertically centered with row text ({text_center_y})",
            toggle_rect.center().y
        );
    }
}
