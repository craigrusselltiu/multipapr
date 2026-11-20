use super::{ACCENT, FIELD, MUTED};
use eframe::egui::*;

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Image,
    Monitor,
    Minus,
    Plus,
    Cog,
    Close,
}

impl Icon {
    pub(super) fn paint(self, draw: &Painter, center: Pos2, tint: Color32) {
        let pen = Stroke::new(1.55_f32, tint);
        let point = |x, y| center + vec2(x, y);
        let line = |a, b| draw.line_segment([a, b], pen);
        match self {
            Self::Image => {
                draw.rect_stroke(
                    Rect::from_center_size(center, vec2(16., 14.)),
                    3,
                    pen,
                    StrokeKind::Middle,
                );
                draw.circle_filled(point(-3., -2.5), 1.35, tint);
                draw.add(Shape::line(
                    vec![
                        point(-7., 4.),
                        point(-2., 0.),
                        point(1., 2.),
                        point(4., -1.),
                        point(7., 2.),
                    ],
                    pen,
                ));
            }
            Self::Monitor => {
                draw.rect_stroke(
                    Rect::from_center_size(point(0., -1.5), vec2(17., 12.)),
                    2,
                    pen,
                    StrokeKind::Middle,
                );
                line(point(0., 5.), point(0., 8.));
                line(point(-4., 8.), point(4., 8.));
            }
            Self::Minus | Self::Plus => {
                line(point(-5., 0.), point(5., 0.));
                if matches!(self, Self::Plus) {
                    line(point(0., -5.), point(0., 5.));
                }
            }
            Self::Close => {
                line(point(-4.5, -4.5), point(4.5, 4.5));
                line(point(-4.5, 4.5), point(4.5, -4.5));
            }
            Self::Cog => {
                let mut outline = Vec::with_capacity(32);
                for tooth in 0..8 {
                    for (phase, length) in [(0., 6.1), (0.18, 8.), (0.52, 8.), (0.72, 6.1)] {
                        let a = (tooth as f32 + phase) * std::f32::consts::FRAC_PI_4;
                        outline.push(center + Vec2::angled(a) * length);
                    }
                }
                draw.add(Shape::closed_line(outline, pen));
                draw.circle_stroke(center, 2.55, pen);
            }
        }
    }

    pub(super) fn button(self, ui: &mut Ui, label: &str, selected: bool) -> Response {
        let (rect, response) = ui.allocate_exact_size(vec2(32., 34.), Sense::click());
        response.widget_info(|| {
            WidgetInfo::selected(WidgetType::Button, ui.is_enabled(), selected, label)
        });
        if selected || response.hovered() || response.has_focus() {
            let fill = if selected {
                Color32::from_rgb(56, 53, 71)
            } else {
                FIELD
            };
            ui.painter().rect_filled(rect, 7, fill);
        }
        self.paint(
            ui.painter(),
            rect.center(),
            if selected { ACCENT } else { MUTED },
        );
        response
            .on_hover_text(label)
            .on_hover_cursor(CursorIcon::PointingHand)
    }
}
