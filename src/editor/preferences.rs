use super::{Editor, INK, LINE, MUTED, PANEL, heading_font, icons::Icon, storage};
use eframe::egui::*;
use std::time::{Duration, Instant};

pub(super) struct SettingsPopup {
    backdrop: Option<TextureHandle>,
    requested: Instant,
    error: Option<String>,
}

impl SettingsPopup {
    pub(super) fn capture(ctx: &Context) -> Self {
        ctx.send_viewport_cmd(ViewportCommand::Screenshot(UserData::default()));
        ctx.request_repaint();
        Self {
            backdrop: None,
            requested: Instant::now(),
            error: None,
        }
    }

    pub(super) fn ready(&mut self, ctx: &Context) -> bool {
        if self.backdrop.is_none() {
            let frame = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
            if let Some(frame) = frame {
                // Sample the editor framebuffer once at quarter logical resolution.
                // The reduced texture keeps the blur inexpensive on high-DPI displays.
                let stride = (4. * ctx.pixels_per_point()).round().max(1.) as usize;
                let [w, h] = frame.size;
                let small = image::RgbImage::from_fn(
                    w.div_ceil(stride) as u32,
                    h.div_ceil(stride) as u32,
                    |x, y| {
                        let x = (x as usize * stride + stride / 2).min(w - 1);
                        let y = (y as usize * stride + stride / 2).min(h - 1);
                        let c = frame[(x, y)];
                        image::Rgb([c.r(), c.g(), c.b()])
                    },
                );
                let blur = image::imageops::blur(&small, 1.65);
                let image = ColorImage::from_rgb(
                    [blur.width() as usize, blur.height() as usize],
                    blur.as_raw(),
                );
                self.backdrop =
                    Some(ctx.load_texture("Settings background", image, TextureOptions::LINEAR));
            } else if self.requested.elapsed() < Duration::from_millis(350) {
                ctx.request_repaint_after(Duration::from_millis(16));
                return false;
            }
        }
        true
    }
}

impl Editor {
    pub(super) fn show_preferences(&mut self, ctx: &Context) {
        let popup = self.popup.as_mut().unwrap();
        CentralPanel::default()
            .frame(Frame::new().fill(INK))
            .show(ctx, |ui| {
                if let Some(tex) = &popup.backdrop {
                    ui.painter().image(
                        tex.id(),
                        ui.max_rect(),
                        Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                        Color32::WHITE,
                    );
                }
            });
        let response = Modal::new(Id::new("settings dialog"))
            .backdrop_color(Color32::from_black_alpha(72))
            .frame(
                Frame::new()
                    .fill(PANEL)
                    .inner_margin(22)
                    .corner_radius(14)
                    .stroke(Stroke::new(1.0_f32, LINE)),
            )
            .show(ctx, |ui| {
                ui.set_width(280.);
                let mut close = false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Settings").font(heading_font(20.)));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        close = Icon::Close.button(ui, "Close settings", false).clicked()
                    });
                });
                ui.separator();
                if ui.checkbox(&mut self.login, "Start at sign-in").changed() {
                    popup.error = storage::startup(self.login).err().map(|e| {
                        self.login = !self.login;
                        e.to_string()
                    });
                }
                if let Some(error) = &popup.error {
                    ui.colored_label(MUTED, error);
                }
                close
            });
        if response.inner || response.should_close() {
            self.popup = None;
            ctx.request_repaint();
        }
    }
}
