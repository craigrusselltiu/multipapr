use super::{ACCENT, FIELD, MUTED, popup::Hint};
use eframe::egui::*;

#[derive(Clone, Copy)]
pub(super) enum Icon {
    Minus,
    Plus,
    Play,
    Pause,
    Cog,
    Close,
    Undo,
    Redo,
    Gallery,
    Profiles,
    Folder,
    Picture,
    Search,
    Filter,
}

impl Icon {
    pub(super) fn paint(self, draw: &Painter, center: Pos2, tint: Color32) {
        let pen = Stroke::new(1.55_f32, tint);
        let point = |x, y| center + vec2(x, y);
        let line = |a, b| draw.line_segment([a, b], pen);
        match self {
            Self::Minus | Self::Plus => {
                line(point(-5., 0.), point(5., 0.));
                if matches!(self, Self::Plus) {
                    line(point(0., -5.), point(0., 5.));
                }
            }
            Self::Play => {
                draw.add(Shape::convex_polygon(
                    vec![point(-4., -6.), point(6., 0.), point(-4., 6.)],
                    tint,
                    Stroke::NONE,
                ));
            }
            Self::Pause => {
                for x in [-3., 3.] {
                    draw.rect_filled(
                        Rect::from_center_size(point(x, 0.), vec2(2.5, 12.)),
                        1,
                        tint,
                    );
                }
            }
            Self::Close => {
                line(point(-4.5, -4.5), point(4.5, 4.5));
                line(point(-4.5, 4.5), point(4.5, -4.5));
            }
            Self::Search => {
                draw.circle_stroke(point(-1.5, -1.5), 4.5, pen);
                line(point(1.8, 1.8), point(6.5, 6.5));
            }
            Self::Filter => {
                draw.add(Shape::closed_line(
                    vec![
                        point(-7., -5.),
                        point(7., -5.),
                        point(2., 0.),
                        point(2., 6.),
                        point(-2., 4.),
                        point(-2., 0.),
                    ],
                    pen,
                ));
            }
            Self::Gallery => {
                draw.add(Shape::line(
                    vec![point(-5., -7.5), point(9., -7.5), point(9., 4.5)],
                    pen,
                ));
                let front = point(-1., 1.5);
                draw.rect_stroke(
                    Rect::from_center_size(front, vec2(16., 12.)),
                    2,
                    pen,
                    StrokeKind::Inside,
                );
                draw.circle_filled(front + vec2(3.5, -2.5), 1.1, tint);
                draw.add(Shape::line(
                    vec![
                        front + vec2(-7., 4.5),
                        front + vec2(-2., -0.5),
                        front + vec2(2.5, 4.),
                        front + vec2(5., 1.5),
                        front + vec2(7., 3.5),
                    ],
                    pen,
                ));
            }
            Self::Picture => {
                draw.rect_stroke(
                    Rect::from_center_size(center, vec2(18., 14.)),
                    2,
                    pen,
                    StrokeKind::Inside,
                );
                draw.circle_filled(point(4., -3.), 1.2, tint);
                draw.add(Shape::line(
                    vec![
                        point(-8., 5.),
                        point(-2.5, -0.5),
                        point(3., 5.),
                        point(6., 2.),
                        point(8., 4.),
                    ],
                    pen,
                ));
            }
            Self::Undo | Self::Redo => {
                let flip = if matches!(self, Self::Redo) { -1. } else { 1. };
                let at = |x: f32, y: f32| point(x * flip, y);
                let mut hook = vec![at(-6., -3.)];
                for step in 0..=10 {
                    let a = (step as f32 / 10. - 0.5) * std::f32::consts::PI;
                    hook.push(at(2. + 4.5 * a.cos(), 1.5 + 4.5 * a.sin()));
                }
                hook.push(at(-1., 6.));
                draw.add(Shape::line(hook, pen));
                draw.add(Shape::line(
                    vec![at(-2.5, -6.5), at(-6., -3.), at(-2.5, 0.5)],
                    pen,
                ));
            }
            Self::Folder => {
                draw.add(Shape::closed_line(
                    vec![
                        point(-8., -6.),
                        point(-3., -6.),
                        point(-1., -4.),
                        point(8., -4.),
                        point(8., 6.),
                        point(-8., 6.),
                    ],
                    pen,
                ));
            }
            Self::Profiles => {
                draw.add(Shape::closed_line(
                    vec![
                        point(-5., -7.),
                        point(5., -7.),
                        point(5., 7.),
                        point(0., 3.),
                        point(-5., 7.),
                    ],
                    pen,
                ));
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
        response.on_hover_cursor(CursorIcon::PointingHand)
    }

    pub(super) fn framed_button(self, ui: &mut Ui, size: Vec2, label: &str) -> Response {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), label));
        let visuals = ui.style().interact(&response);
        ui.painter().rect(
            rect.expand(visuals.expansion),
            visuals.corner_radius,
            visuals.weak_bg_fill,
            visuals.bg_stroke,
            StrokeKind::Inside,
        );
        self.paint(ui.painter(), rect.center(), visuals.text_color());
        response.on_hover_cursor(CursorIcon::PointingHand)
    }
}

pub(super) fn info(ui: &mut Ui, help: &str) {
    let (rect, response) = ui.allocate_exact_size(vec2(17., 22.), Sense::hover());
    let c = rect.center();
    let tint = if response.hovered() { ACCENT } else { MUTED };
    let pen = Stroke::new(1.2_f32, tint);
    ui.painter().circle_stroke(c, 6.2, pen);
    ui.painter().circle_filled(c - vec2(0., 2.5), 0.9, tint);
    ui.painter().line_segment([c, c + vec2(0., 3.)], pen);
    response.hint(help);
}
