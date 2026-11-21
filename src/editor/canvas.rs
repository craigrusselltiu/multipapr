use super::{ACCENT, Editor, INK, PANEL, icons::Icon};
use crate::layout::Rect as World;
use eframe::egui::*;

#[derive(Copy, Clone, PartialEq)]
pub(super) enum Tool {
    Wallpaper,
    Displays,
}
#[derive(Copy, Clone)]
pub(super) enum Drag {
    Wallpaper,
    Display(usize),
    Pan,
    Resize,
}

pub(super) struct Camera {
    zoom: f32,
    offset: Vec2,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            zoom: 1.,
            offset: Vec2::ZERO,
        }
    }
}
impl Camera {
    fn zoom_by(&mut self, ratio: f32, anchor: Vec2) {
        let zoom = (self.zoom * ratio).clamp(0.1, 16.);
        self.offset = anchor - (anchor - self.offset) * (zoom / self.zoom);
        self.zoom = zoom;
    }
    fn transform(&self, area: Rect, frame: World) -> (Pos2, f32) {
        // Reserve 8.5% per side on the limiting axis when framing the composition.
        let fitted = (area.size() * 0.83).max(Vec2::splat(1.));
        let scale = (fitted.x / frame.w).min(fitted.y / frame.h) * self.zoom;
        let origin = area.center() + self.offset
            - vec2(frame.x + frame.w / 2., frame.y + frame.h / 2.) * scale;
        (origin, scale)
    }
}
fn screen_rect(origin: Pos2, scale: f32, world: World) -> Rect {
    Rect::from_min_size(
        origin + vec2(world.x, world.y) * scale,
        vec2(world.w, world.h) * scale,
    )
}
impl Editor {
    pub(super) fn sidebar_tools(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.;
            if Icon::Minus.button(ui, "Zoom out", false).clicked() {
                self.camera.zoom_by(0.8, Vec2::ZERO);
            }
            if Icon::Plus.button(ui, "Zoom in", false).clicked() {
                self.camera.zoom_by(1.25, Vec2::ZERO);
            }
            if ui.button("Reset scale").clicked() {
                self.reframe();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if Icon::Monitor
                    .button(ui, "Displays", self.tool == Tool::Displays)
                    .clicked()
                {
                    self.tool = Tool::Displays;
                }
                if Icon::Image
                    .button(ui, "Wallpaper", self.tool == Tool::Wallpaper)
                    .clicked()
                {
                    self.tool = Tool::Wallpaper;
                }
            });
        });
    }
    pub(super) fn canvas(&mut self, ui: &mut Ui) {
        let (area, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let layout = self.layout();
        let (origin, scale) = self.camera.transform(area, self.frame);
        let media = screen_rect(origin, scale, layout.media(&self.settings));
        let handle = Rect::from_center_size(media.right_bottom(), Vec2::splat(19.));
        let hit = |p: Pos2| {
            layout
                .panels
                .iter()
                .rposition(|r| screen_rect(origin, scale, *r).contains(p))
        };
        let target = |p: Pos2| match self.tool {
            Tool::Wallpaper if self.texture.is_some() && handle.contains(p) => Drag::Resize,
            Tool::Wallpaper if self.texture.is_some() && media.contains(p) => Drag::Wallpaper,
            Tool::Displays => hit(p).map(Drag::Display).unwrap_or(Drag::Pan),
            _ => Drag::Pan,
        };
        if response.hovered() {
            if let Some(pointer) = ui.input(|i| i.pointer.hover_pos()) {
                let ratio = ui.input(|i| (i.smooth_scroll_delta.y * 0.004).exp() * i.zoom_delta());
                if ratio != 1. {
                    self.camera.zoom_by(ratio, pointer - area.center());
                }
                response.clone().on_hover_cursor(match target(pointer) {
                    Drag::Resize => CursorIcon::ResizeNwSe,
                    Drag::Display(_) | Drag::Wallpaper => CursorIcon::Move,
                    Drag::Pan => CursorIcon::Grab,
                });
            }
        }
        if response.clicked() && self.tool == Tool::Displays {
            if let Some(index) = response.interact_pointer_pos().and_then(hit) {
                self.selected = index;
            }
        }
        if response.drag_started_by(PointerButton::Middle) {
            self.drag = ui
                .input(|i| i.pointer.press_origin())
                .map(|p| (Drag::Pan, p));
        } else if response.drag_started_by(PointerButton::Primary) {
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                let drag = target(p);
                if let Drag::Display(i) = drag {
                    self.selected = i;
                }
                self.drag = Some((drag, p));
            }
        }
        if let (Some((drag, last)), Some(pointer)) = (self.drag, response.interact_pointer_pos()) {
            let mut d = pointer - last;
            self.drag = Some((drag, pointer));
            let modifiers = ui.input(|i| i.modifiers);
            if modifiers.ctrl && !matches!(drag, Drag::Resize) {
                if modifiers.shift {
                    d.x = 0.;
                } else {
                    d.y = 0.;
                }
            }
            let delta = d / scale;
            match drag {
                Drag::Pan => {
                    self.camera.offset += d;
                    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                }
                Drag::Display(i) if i < layout.panels.len() && d != Vec2::ZERO => {
                    layout.move_panel(&self.displays, &mut self.settings, i, [delta.x, delta.y]);
                }
                Drag::Wallpaper => {
                    self.settings.pan[0] += delta.x;
                    self.settings.pan[1] += delta.y;
                }
                Drag::Resize => {
                    let diagonal = media.size();
                    let s = (self.settings.scale * (1. + d.dot(diagonal) / diagonal.length_sq()))
                        .clamp(0.01, 100.);
                    let growth = diagonal * ((s / self.settings.scale - 1.) / (2. * scale));
                    self.settings.pan[0] += growth.x;
                    self.settings.pan[1] += growth.y;
                    self.settings.scale = s;
                }
                _ => {}
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        if response.double_clicked() {
            self.reframe();
        }
        let layout = self.layout();
        let (origin, scale) = self.camera.transform(area, self.frame);
        let media = screen_rect(origin, scale, layout.media(&self.settings));
        let draw = ui.painter_at(area);
        draw.rect_filled(area, 0, INK);
        // Keep the grid legible throughout the zoom range and proportional to distance.
        let mut step = 25. * scale;
        while step < 16. {
            step *= 2.;
        }
        while step > 36. {
            step *= 0.5;
        }
        let start = area.min
            + vec2(
                (origin.x - area.left()).rem_euclid(step),
                (origin.y - area.top()).rem_euclid(step),
            );
        for x in 0..=(area.width() / step) as usize {
            for y in 0..=(area.height() / step) as usize {
                draw.circle_filled(
                    start + vec2(x as f32 * step, y as f32 * step),
                    0.8,
                    Color32::from_gray(49),
                );
            }
        }
        let uv = Rect::from_min_max(Pos2::ZERO, pos2(1., 1.));
        if let Some(tex) = &self.texture {
            draw.image(tex.id(), media, uv, Color32::from_gray(95));
        }
        for (i, panel) in layout.panels.iter().enumerate() {
            let rect = screen_rect(origin, scale, *panel);
            draw.rect_filled(rect, 0, PANEL);
            if let Some(tex) = &self.texture {
                draw.with_clip_rect(rect.intersect(area)).image(
                    tex.id(),
                    media,
                    uv,
                    Color32::WHITE,
                );
            }
            let selected = self.tool == Tool::Displays && self.selected == i;
            let tint = if selected {
                ACCENT
            } else if self.tool == Tool::Wallpaper {
                Color32::from_white_alpha(65)
            } else {
                Color32::from_white_alpha(210)
            };
            draw.rect_stroke(
                rect,
                1,
                Stroke::new(if selected { 2.0_f32 } else { 1. }, tint),
                StrokeKind::Inside,
            );
            let label = Rect::from_min_size(rect.min + vec2(7., 7.), vec2(23., 21.));
            if rect.contains_rect(label) {
                draw.rect_filled(
                    label,
                    5,
                    if selected {
                        ACCENT
                    } else {
                        Color32::from_black_alpha(185)
                    },
                );
                draw.text(
                    label.center(),
                    Align2::CENTER_CENTER,
                    (i + 1).to_string(),
                    FontId::proportional(12.),
                    if selected { INK } else { Color32::WHITE },
                );
            }
        }
        if self.texture.is_some() && self.tool == Tool::Wallpaper {
            draw.rect_stroke(
                media,
                0,
                Stroke::new(1.0_f32, Color32::from_white_alpha(120)),
                StrokeKind::Inside,
            );
            draw.rect(
                Rect::from_center_size(media.right_bottom(), Vec2::splat(9.)),
                2,
                ACCENT,
                Stroke::new(1.0_f32, INK),
                StrokeKind::Inside,
            );
        } else if self.texture.is_none() && self.pending.is_none() && self.tool == Tool::Wallpaper {
            if ui
                .put(
                    Rect::from_center_size(area.center(), vec2(148., 34.)),
                    Button::new("Choose wallpaper…"),
                )
                .clicked()
            {
                self.pick(ui.ctx());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn framed_composition_has_proportional_canvas_margins() {
        let composition = World::new(-280., -110., 900., 530.);
        for size in [vec2(474., 400.), vec2(1988., 1174.), vec2(2000., 750.)] {
            let canvas = Rect::from_min_size(pos2(0., 50.), size);
            let (origin, scale) = Camera::default().transform(canvas, composition);
            let preview = screen_rect(origin, scale, composition);
            assert!(preview.center().distance(canvas.center()) < 0.001);
            let fraction = preview.size() / canvas.size();
            assert!((fraction.x.max(fraction.y) - 0.83).abs() < 0.001);
        }
    }

    #[test]
    fn zoom_keeps_point_under_pointer_after_panning() {
        let mut camera = Camera {
            offset: vec2(43., -71.),
            ..Default::default()
        };
        let anchor = vec2(-100., 35.);
        let before = (anchor - camera.offset) / camera.zoom;
        for ratio in [1.25, 500., 0.0001, 3.] {
            camera.zoom_by(ratio, anchor);
            assert!(((anchor - camera.offset) / camera.zoom - before).length() < 0.001);
        }
    }

    #[test]
    fn display_drag_and_pan_use_separate_coordinate_spaces() {
        use super::super::ExitState;
        use crate::{
            layout::{Display, Media, Settings},
            storage::Preferences,
        };
        use std::{path::PathBuf, sync::Arc};
        let ctx = Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let display = Display {
            id: "a".into(),
            name: "a".into(),
            primary: true,
            px: World::new(0., 0., 800., 600.),
            mm: [415., 300.],
            estimated: false,
        };
        let settings = Settings::new(Media {
            path: Default::default(),
            width: 800,
            height: 600,
        });
        let mut editor = Editor::new(
            &cc,
            PathBuf::new(),
            vec![display],
            settings,
            Preferences::default(),
            Arc::new(ExitState::new(true)),
            &IconData {
                width: 1,
                height: 1,
                rgba: vec![255; 4],
            },
        );
        editor.tool = Tool::Displays;
        let area = Rect::from_min_size(Pos2::ZERO, vec2(1000., 750.));
        let frame_with_modifiers = |editor: &mut Editor, modifiers, events| {
            let _ = ctx.run(
                RawInput {
                    screen_rect: Some(area),
                    modifiers,
                    events,
                    ..Default::default()
                },
                |ctx| {
                    CentralPanel::default()
                        .frame(Frame::NONE)
                        .show(ctx, |ui| editor.canvas(ui));
                },
            );
        };
        let frame =
            |editor: &mut Editor, events| frame_with_modifiers(editor, Modifiers::NONE, events);
        let start = area.center();
        let end = start + vec2(70., -26.);
        let button = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(&mut editor, vec![]);
        frame(
            &mut editor,
            vec![Event::PointerMoved(start), button(start, true)],
        );
        frame(&mut editor, vec![Event::PointerMoved(start + vec2(3., 0.))]);
        frame(&mut editor, vec![Event::PointerMoved(end)]);
        let panel = editor.layout().panels[0];
        assert_eq!([panel.x, panel.y], [35., -13.]);
        let right = end + vec2(16., 18.);
        frame_with_modifiers(
            &mut editor,
            Modifiers::CTRL,
            vec![Event::PointerMoved(right)],
        );
        let panel = editor.layout().panels[0];
        assert_eq!([panel.x, panel.y], [43., -13.]);
        let down = right + vec2(16., 18.);
        frame_with_modifiers(
            &mut editor,
            Modifiers::CTRL | Modifiers::SHIFT,
            vec![Event::PointerMoved(down)],
        );
        let panel = editor.layout().panels[0];
        assert_eq!([panel.x, panel.y], [43., -4.]);
        frame(&mut editor, vec![button(down, false)]);
        let saved = editor.settings.clone();
        let middle = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Middle,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(
            &mut editor,
            vec![Event::PointerMoved(start), middle(start, true)],
        );
        frame(&mut editor, vec![Event::PointerMoved(end)]);
        frame(&mut editor, vec![middle(end, false)]);
        assert!(saved == editor.settings);
        assert_eq!(editor.camera.offset, end - start);
    }
}
