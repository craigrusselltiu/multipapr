use super::{Editor, LINE, MUTED, PANEL, heading_font, icons::Icon, storage};
use eframe::{
    egui::*,
    egui_glow::CallbackFn,
    glow::{self, HasContext},
};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct SettingsPopup {
    error: Option<String>,
}

// Each linear reduction averages a 2×2 area. Expanding the same pyramid
// softens the result while keeping the dialog itself at native resolution.
fn blur_editor(info: PaintCallbackInfo, painter: &eframe::egui_glow::Painter) {
    let gl = painter.gl();
    let [width, height] = info.screen_size_px.map(|n| n as i32);
    if width == 0 || height == 0 {
        return;
    }
    let screen = (painter.intermediate_fbo(), width, height);
    let depth = (16. * info.pixels_per_point).log2().round().clamp(1., 6.) as usize;
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
        for _ in 0..depth {
            let previous = *levels.last().unwrap();
            let (w, h) = ((previous.1 / 2).max(1), (previous.2 / 2).max(1));
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

impl Editor {
    pub(super) fn show_preferences(&mut self, ctx: &Context) {
        let popup = self.popup.as_mut().unwrap();
        ctx.layer_painter(LayerId::background()).add(PaintCallback {
            rect: ctx.screen_rect(),
            callback: Arc::new(CallbackFn::new(blur_editor)),
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
