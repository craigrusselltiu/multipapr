use super::{INK, LINE, heading_font};
use eframe::egui::*;

const FADE_SECONDS: f32 = 0.2;

/// Tooltips with roomier padding that stay clear of the window edge and of the
/// dialog or popup holding their widget.
pub(super) trait Hint {
    fn hint(self, text: impl Into<WidgetText>) -> Self;
}

impl Hint for Response {
    fn hint(self, text: impl Into<WidgetText>) -> Self {
        const EDGE: f32 = 8.;
        const GAP: f32 = 4.;
        let ctx = self.ctx.clone();
        let bounds = ctx
            .memory(|m| m.area_rect(self.layer_id.id))
            .unwrap_or_else(|| ctx.screen_rect())
            .shrink(EDGE);
        let widget = self.rect;
        self.on_hover_ui(|ui| {
            ui.set_max_width(ui.spacing().tooltip_width);
            Frame::new()
                .inner_margin(Margin::symmetric(4, 2))
                .show(ui, |ui| ui.label(text));
            let tip = ui.min_rect() + ui.spacing().menu_margin;
            let mut shift = Vec2::ZERO;
            if tip.right() > bounds.right() {
                shift.x = bounds.right() - tip.right();
            }
            if tip.left() + shift.x < bounds.left() {
                shift.x = bounds.left() - tip.left();
            }
            let above = widget.top() - GAP - tip.bottom();
            if tip.bottom() > bounds.bottom()
                && tip.top() >= widget.bottom()
                && tip.top() + above >= bounds.top()
            {
                shift.y = above;
            }
            ctx.set_transform_layer(ui.layer_id(), emath::TSTransform::from_translation(shift));
        })
    }
}

pub(super) fn opacity(ctx: &Context, open: bool) -> f32 {
    ctx.animate_bool_with_time_and_easing(
        Id::new("editor popup opacity"),
        open,
        FADE_SECONDS,
        emath::easing::quadratic_in_out,
    )
}

pub(super) fn dialog(id: Id, size: Vec2, margin: Margin, opacity: f32) -> Modal {
    Modal::new(id)
        .area(Modal::default_area(id).default_size(size).fade_in(false))
        .backdrop_color(Color32::from_black_alpha(96).gamma_multiply(opacity))
        .frame(
            Frame::new()
                .fill(INK.gamma_multiply(opacity))
                .inner_margin(margin)
                .corner_radius(14)
                .stroke(Stroke::new(1.0_f32, LINE.gamma_multiply(opacity))),
        )
}

pub(super) fn header<R>(ui: &mut Ui, title: &str, controls: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::new()
        .inner_margin(Margin {
            left: 22,
            right: 14,
            top: 10,
            bottom: 10,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(title)
                        .font(heading_font(15.5))
                        .color(Color32::from_gray(244)),
                );
                controls(ui)
            })
            .inner
        })
        .inner
}

pub(super) struct ClosingPopup {
    shapes: Vec<epaint::ClippedShape>,
    opacity: f32,
}

impl ClosingPopup {
    pub(super) fn capture(ctx: &Context, id: Id, opacity: f32) -> Option<Self> {
        if opacity <= 0. {
            return None;
        }
        let layer = Modal::default_area(id).layer();
        let shapes = ctx.graphics(|graphics| {
            graphics
                .get(layer)
                .map(|list| list.all_entries().cloned().collect())
        })?;
        Some(Self { shapes, opacity })
    }

    pub(super) fn show(&self, ctx: &Context, opacity: f32) {
        let id = Id::new("closing editor popup");
        Modal::new(id)
            .area(Modal::default_area(id).fade_in(false))
            .frame(Frame::NONE)
            .backdrop_color(Color32::TRANSPARENT)
            .show(ctx, |ui| {
                let mut painter = ctx.layer_painter(ui.layer_id());
                painter.multiply_opacity((opacity / self.opacity).min(1.));
                // Repaint the last frame without reviving any of the dismissed controls.
                for shape in &self.shapes {
                    painter
                        .with_clip_rect(shape.clip_rect)
                        .add(shape.shape.clone());
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_and_backdrop_fade_in_and_out_together() {
        let ctx = Context::default();
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(760., 450.));
        let id = Id::new("animated popup");
        let mut closing = None;
        let mut draw = |time, open| {
            let output = ctx.run(
                RawInput {
                    time: Some(time),
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    let alpha = opacity(ctx, open);
                    if open {
                        dialog(id, vec2(260., 140.), Margin::ZERO, alpha).show(ctx, |ui| {
                            ui.multiply_opacity(alpha);
                            ui.set_min_size(vec2(260., 140.));
                            ui.label("Popup contents");
                        });
                        closing = ClosingPopup::capture(ctx, id, alpha);
                    } else if alpha > 0.
                        && let Some(closing) = &closing
                    {
                        closing.show(ctx, alpha);
                    }
                },
            );
            let backdrop = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    Shape::Rect(rect) if rect.rect == screen => Some(rect.fill.a()),
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            let panel = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    Shape::Rect(rect) if rect.corner_radius == CornerRadius::same(14) => {
                        Some(rect.fill.a())
                    }
                    _ => None,
                })
                .max()
                .unwrap_or(0);
            (backdrop, panel)
        };
        assert_eq!(draw(0., false), (0, 0));
        draw(0.1, true);
        let (backdrop, panel) = draw(0.17, true);
        assert!((1..96).contains(&backdrop) && (1..255).contains(&panel));
        assert_eq!(draw(0.5, true), (96, 255));
        draw(0.6, false);
        let (backdrop, panel) = draw(0.67, false);
        assert!((1..96).contains(&backdrop) && (1..255).contains(&panel));
        assert_eq!(draw(1., false), (0, 0));
    }
}
