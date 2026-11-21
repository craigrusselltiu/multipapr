use super::{ACCENT, Editor, FIELD, INK, LINE, PANEL, TEXT, icons::Icon};
use crate::layout::Rect as World;
use eframe::egui::*;

#[derive(Copy, Clone, PartialEq)]
pub(super) enum Tool {
    None,
    Wallpaper,
    Displays,
}
#[derive(Copy, Clone, PartialEq, Debug)]
pub(super) enum Edge {
    Corner,
    Right,
    Bottom,
}
#[derive(Copy, Clone, PartialEq, Debug)]
pub(super) enum Drag {
    Wallpaper,
    Display(usize),
    Pan,
    ResizeWallpaper(Edge),
    ResizeDisplay(usize, Edge),
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
        let fitted = (area.size() * 0.83).max(Vec2::splat(1.));
        let scale = (fitted.x / frame.w).min(fitted.y / frame.h) * self.zoom;
        let origin = area.center() + self.offset
            - vec2(frame.x + frame.w / 2., frame.y + frame.h / 2.) * scale;
        (origin, scale)
    }
}
fn hatch(draw: &Painter, r: Rect) {
    let draw = draw.with_clip_rect(r.intersect(draw.clip_rect()));
    let r = draw.clip_rect();
    draw.rect_filled(r, 0, Color32::from_black_alpha(90));
    let pen = Stroke::new(1.0_f32, Color32::from_white_alpha(90));
    let mut x = r.left() - r.height();
    while x < r.right() {
        draw.line_segment([pos2(x, r.bottom()), pos2(x + r.height(), r.top())], pen);
        x += 7.;
    }
}
const GRAB: f32 = 6.;
fn screen_rect(origin: Pos2, scale: f32, world: World) -> Rect {
    Rect::from_min_size(
        origin + vec2(world.x, world.y) * scale,
        vec2(world.w, world.h) * scale,
    )
}
fn handles(r: Rect) -> [(Edge, Pos2); 3] {
    [
        (Edge::Corner, r.right_bottom()),
        (Edge::Right, r.right_center()),
        (Edge::Bottom, r.center_bottom()),
    ]
}
fn handle_at(r: Rect, p: Pos2) -> Option<Edge> {
    handles(r)
        .into_iter()
        .find(|(_, c)| Rect::from_center_size(*c, Vec2::splat(19.)).contains(p))
        .map(|(edge, _)| edge)
}
fn draw_handles(draw: &Painter, r: Rect) {
    for (edge, c) in handles(r) {
        let size = match edge {
            Edge::Corner => vec2(9., 9.),
            Edge::Right => vec2(6., 14.),
            Edge::Bottom => vec2(14., 6.),
        };
        draw.rect(
            Rect::from_center_size(c, size),
            2,
            ACCENT,
            Stroke::new(1.0_f32, INK),
            StrokeKind::Inside,
        );
    }
}
fn cursor(drag: Drag) -> CursorIcon {
    match drag {
        Drag::ResizeWallpaper(edge) | Drag::ResizeDisplay(_, edge) => match edge {
            Edge::Corner => CursorIcon::ResizeNwSe,
            Edge::Right => CursorIcon::ResizeHorizontal,
            Edge::Bottom => CursorIcon::ResizeVertical,
        },
        Drag::Display(_) | Drag::Wallpaper => CursorIcon::Move,
        Drag::Pan => CursorIcon::Grab,
    }
}
fn text_button(ui: &mut Ui, label: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.into(), FontId::proportional(13.), TEXT);
    let (rect, response) = ui.allocate_exact_size(vec2(galley.size().x + 20., 34.), Sense::click());
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    if response.hovered() || response.has_focus() {
        ui.painter().rect_filled(rect, 7, FIELD);
    }
    ui.painter()
        .galley(rect.center() - galley.size() / 2., galley, TEXT);
    response.on_hover_cursor(CursorIcon::PointingHand)
}
impl Editor {
    pub(super) fn shows_full_wallpaper(&self) -> bool {
        !self.settings.multi || self.tool != Tool::None
    }
    fn stacking(&self, count: usize) -> Vec<usize> {
        let mut order: Vec<usize> = (0..count).collect();
        if self.tool == Tool::Displays && self.selected < count {
            order.retain(|&i| i != self.selected);
            order.push(self.selected);
        }
        order
    }
    fn toolbar(&mut self, ui: &mut Ui, area: Rect) {
        let mut bar = ui.new_child(
            UiBuilder::new()
                .max_rect(area.shrink(12.))
                .layout(Layout::right_to_left(Align::Min)),
        );
        Frame::new()
            .fill(PANEL)
            .stroke(Stroke::new(1.0_f32, LINE))
            .corner_radius(10)
            .inner_margin(3)
            .show(&mut bar, |ui| {
                ui.spacing_mut().item_spacing.x = 2.;
                if text_button(ui, "Reset scale").clicked() {
                    self.reframe();
                }
                if Icon::Plus
                    .button(ui, "Zoom in", false)
                    .on_hover_text("Zoom in")
                    .clicked()
                {
                    self.camera.zoom_by(1.25, Vec2::ZERO);
                }
                if Icon::Minus
                    .button(ui, "Zoom out", false)
                    .on_hover_text("Zoom out")
                    .clicked()
                {
                    self.camera.zoom_by(0.8, Vec2::ZERO);
                }
            });
    }
    pub(super) fn canvas(&mut self, ui: &mut Ui) {
        let (area, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let layout = self.layout();
        let (origin, scale) = self.camera.transform(area, self.frame);
        let wallpaper = self.wallpaper().clone();
        let has_wallpaper = !wallpaper.is_empty();
        let media = screen_rect(origin, scale, layout.media(&self.settings, &wallpaper));
        let order = self.stacking(layout.panels.len());
        let rect = |i: usize| screen_rect(origin, scale, layout.panels[i]);
        let under = |p: Pos2| -> Vec<usize> {
            order
                .iter()
                .rev()
                .copied()
                .filter(|&i| rect(i).contains(p))
                .collect()
        };
        let hit = |p: Pos2| under(p).first().copied();
        let tool = self.tool;
        let selected = self.selected.min(layout.panels.len() - 1);
        let outline = |p: Pos2| {
            order.iter().rev().copied().find(|&i| {
                let r = rect(i);
                r.expand(GRAB).contains(p) && !r.shrink(GRAB).contains(p)
            })
        };
        let full_wallpaper = self.shows_full_wallpaper();
        let on_wallpaper = |p: Pos2| has_wallpaper && full_wallpaper && media.contains(p);
        let target = |p: Pos2| match tool {
            Tool::Wallpaper => {
                if has_wallpaper && let Some(edge) = handle_at(media, p) {
                    return Drag::ResizeWallpaper(edge);
                }
                outline(p)
                    .map(Drag::Display)
                    .or((has_wallpaper && media.contains(p)).then_some(Drag::Wallpaper))
                    .or(hit(p).map(Drag::Display))
                    .unwrap_or(Drag::Pan)
            }
            Tool::Displays | Tool::None => {
                if tool == Tool::Displays
                    && let Some(edge) = handle_at(rect(selected), p)
                {
                    return Drag::ResizeDisplay(selected, edge);
                }
                hit(p)
                    .map(Drag::Display)
                    .or(on_wallpaper(p).then_some(Drag::Wallpaper))
                    .unwrap_or(Drag::Pan)
            }
        };
        if response.hovered()
            && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
        {
            let ratio = ui.input(|i| (i.smooth_scroll_delta.y * 0.004).exp() * i.zoom_delta());
            if ratio != 1. {
                self.last_pick = None;
                self.camera.zoom_by(ratio, pointer - area.center());
            }
            response.clone().on_hover_cursor(cursor(target(pointer)));
        }
        let stack = |p: Pos2| {
            let mut ids = under(p);
            ids.sort_unstable();
            let mut stack: Vec<Drag> = ids.into_iter().map(Drag::Display).collect();
            if on_wallpaper(p) {
                stack.push(Drag::Wallpaper);
            }
            if stack.is_empty()
                && let Some(i) = outline(p)
            {
                stack.push(Drag::Display(i));
            }
            stack
        };
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let stack = stack(p);
            let current = match tool {
                Tool::Displays => Some(Drag::Display(selected)),
                Tool::Wallpaper => Some(Drag::Wallpaper),
                Tool::None => None,
            };
            let repeated = self.last_pick.is_some_and(|last| last.distance(p) <= GRAB);
            self.last_pick = Some(p);
            let next = if repeated && let Some(at) = stack.iter().position(|s| Some(*s) == current)
            {
                Some(stack[(at + 1) % stack.len()])
            } else {
                hit(p).map(Drag::Display).or(stack.first().copied())
            };
            match next {
                Some(Drag::Display(i)) => {
                    self.selected = i;
                    self.tool = Tool::Displays;
                }
                Some(Drag::Wallpaper) => self.tool = Tool::Wallpaper,
                _ => self.tool = Tool::None,
            }
        }
        if response.double_clicked()
            && let Some(p) = response.interact_pointer_pos()
            && stack(p).is_empty()
        {
            self.reframe();
        }
        if response.drag_started_by(PointerButton::Middle) {
            self.last_pick = None;
            self.drag = ui
                .input(|i| i.pointer.press_origin())
                .map(|p| (Drag::Pan, p));
        } else if response.drag_started_by(PointerButton::Primary) {
            self.last_pick = None;
            if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
                let drag = target(p);
                match drag {
                    Drag::Display(i) | Drag::ResizeDisplay(i, _) => {
                        self.selected = i;
                        self.tool = Tool::Displays;
                    }
                    Drag::Wallpaper | Drag::ResizeWallpaper(_) => self.tool = Tool::Wallpaper,
                    Drag::Pan => {}
                }
                self.drag = Some((drag, p));
            }
        }
        if let (Some((drag, last)), Some(pointer)) = (self.drag, response.interact_pointer_pos()) {
            let mut d = pointer - last;
            self.drag = Some((drag, pointer));
            let modifiers = ui.input(|i| i.modifiers);
            if modifiers.ctrl && matches!(drag, Drag::Wallpaper | Drag::Display(_) | Drag::Pan) {
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
                    let w = self.wallpaper_mut();
                    w.center[0] += delta.x;
                    w.center[1] += delta.y;
                }
                Drag::ResizeWallpaper(edge) if d != Vec2::ZERO => {
                    let size = media.size() / scale;
                    let w = self.wallpaper_mut();
                    match edge {
                        Edge::Corner => {
                            let diagonal = media.size();
                            let s = (w.scale * (1. + d.dot(diagonal) / diagonal.length_sq()))
                                .clamp(0.01, 100.);
                            let growth = diagonal * ((s / w.scale - 1.) / (2. * scale));
                            w.center[0] += growth.x;
                            w.center[1] += growth.y;
                            w.scale = s;
                        }
                        Edge::Right => {
                            let width = (size.x + delta.x).max(1.);
                            w.stretch *= width / size.x;
                            w.center[0] += (width - size.x) / 2.;
                        }
                        Edge::Bottom => {
                            let height = (size.y + delta.y).max(1.);
                            let ratio = height / size.y;
                            w.scale *= ratio;
                            w.stretch /= ratio;
                            w.center[1] += (height - size.y) / 2.;
                        }
                    }
                }
                Drag::ResizeDisplay(i, edge) if i < layout.panels.len() && d != Vec2::ZERO => {
                    let p = layout.panels[i];
                    let size = match edge {
                        Edge::Corner => {
                            let diagonal = rect(i).size();
                            super::proportional_size(p, 1. + d.dot(diagonal) / diagonal.length_sq())
                        }
                        Edge::Right => [p.w + delta.x, p.h],
                        Edge::Bottom => [p.w, p.h + delta.y],
                    };
                    layout.resize_panel(&self.displays, &mut self.settings, i, size);
                }
                _ => {}
            }
        }
        if response.drag_stopped() {
            self.drag = None;
        }
        let layout = self.layout();
        let (origin, scale) = self.camera.transform(area, self.frame);
        let wallpaper = self.wallpaper().clone();
        let media = screen_rect(origin, scale, layout.media(&self.settings, &wallpaper));
        let draw = ui.painter_at(area);
        draw.rect_filled(area, 0, INK);
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
        if self.shows_full_wallpaper()
            && let Some(tex) = self.textures.get(&wallpaper.media.path)
        {
            draw.image(tex.id(), media, uv, Color32::from_gray(95));
        }
        let order = self.stacking(layout.panels.len());
        let rects: Vec<Rect> = layout
            .panels
            .iter()
            .map(|r| screen_rect(origin, scale, *r))
            .collect();
        for &i in &order {
            draw.rect_filled(rects[i], 0, PANEL);
            let shown = self.settings.shown(&self.displays[i].id);
            if let Some(tex) = self.textures.get(&shown.media.path) {
                let r = screen_rect(origin, scale, layout.media(&self.settings, shown));
                draw.with_clip_rect(rects[i].intersect(area)).image(
                    tex.id(),
                    r,
                    uv,
                    Color32::WHITE,
                );
            }
        }
        if self.tool == Tool::Displays {
            for (a, pa) in layout.panels.iter().enumerate() {
                for (b, pb) in layout.panels.iter().enumerate().skip(a + 1) {
                    if pa.overlaps(*pb) {
                        hatch(&draw, rects[a].intersect(rects[b]));
                    }
                }
            }
        }
        for &i in &order {
            let selected = self.tool == Tool::Displays && self.selected == i;
            let tint = if selected {
                ACCENT
            } else if self.tool == Tool::Wallpaper {
                Color32::from_white_alpha(65)
            } else {
                Color32::from_white_alpha(210)
            };
            draw.rect_stroke(
                rects[i],
                1,
                Stroke::new(if selected { 2.0_f32 } else { 1. }, tint),
                StrokeKind::Inside,
            );
        }
        let mut badges: Vec<Rect> = Vec::new();
        for &i in &order {
            let selected = self.tool == Tool::Displays && self.selected == i;
            let mut label = Rect::from_min_size(rects[i].min + vec2(7., 7.), vec2(23., 21.));
            while badges.iter().any(|b| b.intersects(label)) {
                label = label.translate(vec2(27., 0.));
            }
            if !rects[i].contains_rect(label) {
                continue;
            }
            badges.push(label);
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
        if !wallpaper.is_empty() && self.shows_full_wallpaper() {
            if self.tool == Tool::Wallpaper {
                draw.rect_stroke(media, 0, Stroke::new(2.0_f32, ACCENT), StrokeKind::Inside);
                draw_handles(&draw, media);
            } else {
                draw.rect_stroke(
                    media,
                    0,
                    Stroke::new(1.0_f32, Color32::from_white_alpha(65)),
                    StrokeKind::Inside,
                );
            }
        }
        if self.tool == Tool::Displays && self.selected < rects.len() {
            draw_handles(&draw, rects[self.selected]);
        }
        if wallpaper.is_empty()
            && !self.importing_selected()
            && self.tool != Tool::Displays
            && ui
                .put(
                    Rect::from_center_size(area.center(), vec2(176., 34.)),
                    Button::new("Choose wallpaper…"),
                )
                .clicked()
        {
            self.pick(ui.ctx());
        }
        self.toolbar(ui, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Calibration, Display, Media, Settings, Upscaling};

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

    const AREA: Rect = Rect {
        min: Pos2::ZERO,
        max: pos2(1000., 750.),
    };

    fn display(id: &str) -> Display {
        Display {
            id: id.into(),
            name: id.into(),
            primary: id == "a",
            px: World::new(0., 0., 800., 600.),
            mm: [415., 300.],
            estimated: false,
            mirrors: Vec::new(),
            mirror_ids: Vec::new(),
        }
    }

    fn editor(ctx: &Context, displays: Vec<Display>, settings: Settings) -> Editor {
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        Editor::new(
            &cc,
            std::path::PathBuf::new(),
            super::super::EditorStartup {
                displays,
                settings,
                profiles: Ok(Vec::new()),
                startup_enabled: false,
                preferences: crate::storage::Preferences::default(),
            },
            std::sync::Arc::new(super::super::ExitState::new(true)),
            &IconData {
                width: 1,
                height: 1,
                rgba: vec![255; 4],
            },
        )
    }

    fn blank() -> Settings {
        Settings::new(Media {
            path: Default::default(),
            width: 800,
            height: 600,
        })
    }

    fn run(
        ctx: &Context,
        editor: &mut Editor,
        modifiers: Modifiers,
        events: Vec<Event>,
    ) -> FullOutput {
        ctx.run(
            RawInput {
                screen_rect: Some(AREA),
                modifiers,
                events,
                ..Default::default()
            },
            |ctx| {
                CentralPanel::default()
                    .frame(Frame::NONE)
                    .show(ctx, |ui| editor.canvas(ui));
            },
        )
    }

    fn button(pos: Pos2, pressed: bool) -> Event {
        Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        }
    }

    fn click(ctx: &Context, editor: &mut Editor, pos: Pos2) {
        for events in [
            vec![],
            vec![Event::PointerMoved(pos), button(pos, true)],
            vec![button(pos, false)],
        ] {
            run(ctx, editor, Modifiers::NONE, events);
        }
        ctx.input_mut(|i| i.time += 1.);
    }

    fn drag(ctx: &Context, editor: &mut Editor, from: Pos2, to: Pos2) {
        run(ctx, editor, Modifiers::NONE, vec![]);
        run(
            ctx,
            editor,
            Modifiers::NONE,
            vec![Event::PointerMoved(from), button(from, true)],
        );
        run(ctx, editor, Modifiers::NONE, vec![Event::PointerMoved(to)]);
        run(ctx, editor, Modifiers::NONE, vec![button(to, false)]);
        ctx.input_mut(|i| i.time += 1.);
    }

    fn covered(ctx: &Context) -> Editor {
        let mut b = display("b");
        b.px.x = 800.;
        let mut settings = blank();
        settings.wallpaper.media.path = "wall.png".into();
        let mut editor = editor(ctx, vec![display("a"), b], settings);
        let l = editor.layout();
        let pitch = l.pitch(&editor.settings);
        editor.settings.wallpaper.cover(pitch, l.bounds);
        editor.textures.insert(
            "wall.png".into(),
            ctx.load_texture(
                "wall",
                ColorImage::new([1, 1], Color32::WHITE),
                TextureOptions::default(),
            ),
        );
        editor.reframe();
        editor
    }

    fn screen(editor: &Editor, world: World) -> Rect {
        let (origin, scale) = editor.camera.transform(AREA, editor.frame);
        screen_rect(origin, scale, world)
    }

    #[test]
    fn workspace_keeps_zoom_controls_in_the_corner_and_inspector_values_visible() {
        for (size, multi) in [
            (vec2(1080., 680.), false),
            (vec2(1080., 680.), true),
            (vec2(760., 450.), true),
        ] {
            let ctx = Context::default();
            let mut editor = covered(&ctx);
            editor.set_multi(multi);
            ctx.style_mut(|style| style.animation_time = 0.);
            let mut frame = || {
                ctx.run(
                    RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        super::super::chrome::show(
                            ctx,
                            editor.brand.id(),
                            true,
                            false,
                            false,
                            false,
                        );
                        editor.workspace(ctx);
                    },
                )
            };
            frame();
            frame();
            let output = frame();
            if size.y == 680. {
                let heading_y = |label| {
                    output
                        .shapes
                        .iter()
                        .find_map(|shape| match &shape.shape {
                            Shape::Text(t) if t.galley.job.text == label => Some(t.pos.y),
                            _ => None,
                        })
                        .unwrap()
                };
                assert!(heading_y("Advanced") > heading_y("Wallpaper").max(heading_y("Displays")));
            }
            let canvas = ctx.available_rect();
            let toolbar = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Rect(r) if r.fill == PANEL && r.stroke.color == LINE => Some(r.rect),
                    _ => None,
                })
                .unwrap();
            assert!(
                toolbar.width() < 175. && toolbar.height() < 48.,
                "{toolbar:?}"
            );
            assert!(
                toolbar
                    .right_top()
                    .distance(canvas.right_top() + vec2(-12., 12.))
                    < 1.
            );
            let scrollbar = output.shapes.iter().find_map(|s| match &s.shape {
                Shape::Rect(r)
                    if r.rect.width() == 6.
                        && r.rect.right() == size.x - 12.
                        && r.rect.height() > 30. =>
                {
                    Some(r.rect)
                }
                _ => None,
            });
            assert_eq!(scrollbar.is_some(), size.y == 450.);
            let content_right = scrollbar.map_or(size.x - 18., |r| r.left() - 10.);
            let mut found_resolution = false;
            for shape in &output.shapes {
                if let Shape::Text(t) = &shape.shape
                    && t.galley.job.text.ends_with(" px")
                {
                    found_resolution = true;
                    let right = t.visual_bounding_rect().right();
                    assert!(
                        right <= content_right.min(shape.clip_rect.right()) + 1.,
                        "resolution ends at {right}, content ends at {content_right}, clip {:?}",
                        shape.clip_rect
                    );
                }
            }
            assert!(found_resolution);
        }
    }

    #[test]
    fn clearing_selection_removes_accents_and_crops_multi_mode_previews() {
        for multi in [false, true] {
            let ctx = Context::default();
            let mut editor = covered(&ctx);
            editor.set_multi(multi);
            let layout = editor.layout();
            let panels: Vec<_> = layout.panels.iter().map(|p| screen(&editor, *p)).collect();
            let media = screen(&editor, layout.media(&editor.settings, editor.wallpaper()));
            let margin = media.center_top() + vec2(0., 2.);
            assert!(!panels.iter().any(|p| p.contains(margin)));

            click(&ctx, &mut editor, panels[1].center());
            assert!(editor.tool == Tool::Displays && editor.selected == 1);
            click(&ctx, &mut editor, pos2(3., 3.));
            assert!(editor.tool == Tool::None);
            let tex = editor.textures[std::path::Path::new("wall.png")].id();
            let clips = |frame: FullOutput| {
                frame
                    .shapes
                    .into_iter()
                    .filter_map(|s| match s.shape {
                        Shape::Mesh(m) if m.texture_id == tex => Some(s.clip_rect),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
            };
            let output = run(&ctx, &mut editor, Modifiers::NONE, vec![]);
            assert!(!output.shapes.iter().any(|s| matches!(&s.shape, Shape::Rect(r) if r.fill == ACCENT || r.stroke.color == ACCENT)));
            let preview = clips(output);
            assert!(panels.iter().all(|p| preview.contains(p)));
            assert_eq!(preview.contains(&AREA), !multi);
            click(&ctx, &mut editor, margin);
            assert!(if multi {
                editor.tool == Tool::None
            } else {
                editor.tool == Tool::Wallpaper
            });
            click(&ctx, &mut editor, panels[0].center());
            assert!(clips(run(&ctx, &mut editor, Modifiers::NONE, vec![])).contains(&AREA));
        }
    }

    #[test]
    fn clicking_overlapping_displays_steps_through_them() {
        let ctx = Context::default();
        let mut settings = blank();
        for id in ["a", "b"] {
            settings.calibration.insert(
                id.into(),
                Calibration {
                    position: Some([0., 0.]),
                    ..Default::default()
                },
            );
        }
        let mut editor = editor(&ctx, vec![display("a"), display("b")], settings);
        editor.tool = Tool::Displays;
        assert_eq!(editor.layout().overlapping(0), vec![1]);
        let mut picked = Vec::new();
        for _ in 0..3 {
            click(&ctx, &mut editor, pos2(425., 295.));
            picked.push(editor.selected);
        }
        assert_eq!(picked, [0, 1, 0]);
    }

    #[test]
    fn clicks_step_from_a_display_to_the_wallpaper_behind_it_and_drags_move_the_selection() {
        let ctx = Context::default();
        let mut editor = covered(&ctx);
        editor.tool = Tool::Wallpaper;
        let b = screen(&editor, editor.layout().panels[1]).center();
        let center = editor.settings.wallpaper.center;
        drag(&ctx, &mut editor, b, b + vec2(40., 0.));
        assert!(editor.tool == Tool::Wallpaper);
        assert!(editor.settings.wallpaper.center != center);
        click(&ctx, &mut editor, b);
        assert!(editor.tool == Tool::Displays && editor.selected == 1);
        click(&ctx, &mut editor, b);
        assert!(editor.tool == Tool::Wallpaper);
    }

    #[test]
    fn edge_handles_change_only_width_or_height() {
        let ctx = Context::default();
        let mut editor = covered(&ctx);
        editor.tool = Tool::Wallpaper;
        let l = editor.layout();
        let before = l.media(&editor.settings, &editor.settings.wallpaper);
        let media = screen(&editor, before);
        drag(
            &ctx,
            &mut editor,
            media.right_center(),
            media.right_center() + vec2(30., 0.),
        );
        let after = l.media(&editor.settings, &editor.settings.wallpaper);
        assert!(after.w > before.w);
        assert!((after.h - before.h).abs() < 0.01);
        assert!((after.x - before.x).abs() < 0.01 && (after.y - before.y).abs() < 0.01);
        editor.tool = Tool::Displays;
        editor.selected = 1;
        let panel = editor.layout().panels[1];
        let bottom = screen(&editor, panel).center_bottom();
        drag(&ctx, &mut editor, bottom, bottom + vec2(0., 25.));
        let resized = editor.layout().panels[1];
        assert!(resized.h > panel.h);
        assert_eq!(
            [resized.x, resized.y, resized.w],
            [panel.x, panel.y, panel.w]
        );
    }

    #[test]
    fn imports_for_different_displays_can_finish_together() {
        let ctx = Context::default();
        let mut editor = covered(&ctx);
        editor.set_multi(true);
        for (i, id) in ["a", "b"].into_iter().enumerate() {
            let (tx, rx) = std::sync::mpsc::channel();
            let frame = editor.layout().panels[i];
            editor.pending.insert(Some(id.into()), (frame, rx));
            tx.send(Ok(super::super::Imported {
                media: Media {
                    path: id.into(),
                    width: 25,
                    height: 50,
                },
                image: image::RgbaImage::new(1, 1),
            }))
            .ok()
            .unwrap();
        }
        editor.collect(&ctx);
        for (i, id) in ["a", "b"].into_iter().enumerate() {
            let w = editor.settings.shown(id);
            assert_eq!(w.media.path.to_str(), Some(id));
            assert_eq!(w.center, editor.layout().panels[i].center());
        }
        assert!(editor.pending.is_empty());
    }

    #[test]
    fn undoing_a_file_change_restores_the_other_monitor_framings() {
        let ctx = Context::default();
        let mut editor = covered(&ctx);
        let other = crate::layout::Framing {
            scale: 3.,
            stretch: 0.75,
            center: [51., 84.],
        };
        editor.profiles[0].framings.insert("a".into(), other);
        editor.start_profile();
        editor.settings.wallpaper.media.path = "replacement.jpg".into();
        editor.remember_current();
        editor.record(&ctx);
        assert!(!editor.profiles[0].framings.contains_key("a"));
        editor.step(true);
        editor.remember_current();
        assert_eq!(
            editor.settings.wallpaper.media.path.to_str(),
            Some("wall.png")
        );
        assert!(editor.profiles[0].framings["a"] == other);
        editor.step(false);
        editor.remember_current();
        assert!(!editor.profiles[0].framings.contains_key("a"));
    }

    #[test]
    fn switching_profiles_validates_only_the_files_on_connected_screens() {
        let ctx = Context::default();
        let mut editor = covered(&ctx);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("present.jpg");
        std::fs::write(&file, []).unwrap();
        let mut profile = editor.profiles[0].clone();
        profile.name = "separate".into();
        profile.media.path = dir.path().join("missing-span");
        profile.multi = true;
        for id in ["a", "b"] {
            let mut w = editor.settings.wallpaper.clone();
            w.media.path = file.clone();
            profile.screens.insert(id.into(), w);
        }
        editor.profiles.push(profile);
        assert!(editor.load_profile(1));
        editor.profiles[0].media.path = dir.path().join("missing");
        assert!(!editor.load_profile(0));
        assert_eq!(editor.active_profile(), Some(1));
        editor.profiles[0].multi = true;
        editor.profiles[0].screens = editor.settings.screens.clone();
        editor.profiles[0].screens.get_mut("b").unwrap().media.path = dir.path().join("gone.jpg");
        assert!(!editor.load_profile(0));
        assert!(editor.message.contains("gone.jpg"));
    }

    #[test]
    fn multi_mode_starts_each_display_on_the_span_and_edits_only_the_selected_one() {
        let ctx = Context::default();
        let mut editor = covered(&ctx);
        editor.set_multi(true);
        assert!(editor.settings.screens.len() == 2);
        for id in ["a", "b"] {
            assert!(editor.settings.shown(id) == &editor.settings.wallpaper);
        }
        editor.selected = 1;
        editor.wallpaper_mut().scale *= 2.;
        assert!(editor.settings.shown("a") == &editor.settings.wallpaper);
        assert!(editor.settings.shown("b") != &editor.settings.wallpaper);
    }

    #[test]
    fn undo_and_redo_step_through_committed_edits() {
        let ctx = Context::default();
        let mut editor = editor(&ctx, vec![display("a")], blank());
        let original = editor.settings.clone();
        editor.settings.wallpaper.scale = 2.;
        editor.record(&ctx);
        let first = editor.settings.clone();
        editor.settings.wallpaper.center = [40., 0.];
        editor.record(&ctx);
        let second = editor.settings.clone();
        editor.step(true);
        assert!(editor.settings == first);
        editor.step(true);
        assert!(editor.settings == original);
        editor.step(true);
        assert!(editor.settings == original);
        editor.step(false);
        editor.step(false);
        assert!(editor.settings == second);
        editor.step(true);
        editor.settings.upscaling = Upscaling::Lanczos;
        editor.record(&ctx);
        editor.step(false);
        assert_eq!(editor.settings.upscaling, Upscaling::Lanczos);
        assert_eq!(editor.settings.wallpaper.center, [0., 0.]);
    }

    #[test]
    fn display_drag_and_pan_use_separate_coordinate_spaces() {
        let ctx = Context::default();
        let mut editor = editor(&ctx, vec![display("a")], blank());
        editor.tool = Tool::Displays;
        let start = pos2(425., 295.);
        let end = start + vec2(70., -26.);
        run(&ctx, &mut editor, Modifiers::NONE, vec![]);
        run(
            &ctx,
            &mut editor,
            Modifiers::NONE,
            vec![Event::PointerMoved(start), button(start, true)],
        );
        run(
            &ctx,
            &mut editor,
            Modifiers::NONE,
            vec![Event::PointerMoved(start + vec2(3., 0.))],
        );
        run(
            &ctx,
            &mut editor,
            Modifiers::NONE,
            vec![Event::PointerMoved(end)],
        );
        let panel = editor.layout().panels[0];
        assert_eq!([panel.x, panel.y], [35., -13.]);
        let right = end + vec2(16., 18.);
        run(
            &ctx,
            &mut editor,
            Modifiers::CTRL,
            vec![Event::PointerMoved(right)],
        );
        let panel = editor.layout().panels[0];
        assert_eq!([panel.x, panel.y], [43., -13.]);
        let down = right + vec2(16., 18.);
        run(
            &ctx,
            &mut editor,
            Modifiers::CTRL | Modifiers::SHIFT,
            vec![Event::PointerMoved(down)],
        );
        let panel = editor.layout().panels[0];
        assert_eq!([panel.x, panel.y], [43., -4.]);
        run(
            &ctx,
            &mut editor,
            Modifiers::NONE,
            vec![button(down, false)],
        );
        let saved = editor.settings.clone();
        let middle = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Middle,
            pressed,
            modifiers: Modifiers::NONE,
        };
        run(
            &ctx,
            &mut editor,
            Modifiers::NONE,
            vec![Event::PointerMoved(start), middle(start, true)],
        );
        run(
            &ctx,
            &mut editor,
            Modifiers::NONE,
            vec![Event::PointerMoved(end)],
        );
        run(&ctx, &mut editor, Modifiers::NONE, vec![middle(end, false)]);
        assert!(saved == editor.settings);
        assert_eq!(editor.camera.offset, end - start);
    }
}
