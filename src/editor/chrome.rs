use super::{ACCENT, LINE, MUTED, PANEL, icons::Icon, popup::Hint};
use eframe::egui::*;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::{Dwm::*, Gdi::*},
    UI::{HiDpi::GetDpiForWindow, Shell::*, WindowsAndMessaging::*},
};

const HEIGHT: f32 = 46.;
const CONTROL_WIDTH: f32 = 46.;
const CONTROLS: f32 = 8.;
const DIVIDER_WIDTH: f32 = 11.;
const GALLERY: &str = "Gallery";

/// Default right-sidebar width so its left separator aligns pixel-for-pixel with
/// the title-bar divider between Redo and Gallery.
pub const SIDEBAR_WIDTH: f32 = 6. * CONTROL_WIDTH + (DIVIDER_WIDTH + 1.) / 2.;

pub struct Actions {
    pub settings: bool,
    pub undo: bool,
    pub redo: bool,
    pub gallery: bool,
    pub gallery_id: Id,
    pub profiles: bool,
    pub profiles_anchor: Rect,
}

pub fn install(cc: &eframe::CreationContext<'_>) {
    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win) = handle.as_raw() else {
        return;
    };
    let hwnd = win.hwnd.get() as HWND;
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE as u32,
            (&DWMWCP_ROUND as *const i32).cast(),
            size_of::<i32>() as u32,
        );
        SetWindowSubclass(hwnd, Some(window_proc), 1, 0);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _: usize,
) -> LRESULT {
    unsafe {
        if msg == WM_NCHITTEST {
            let mut pt = POINT {
                x: lp as i16 as i32,
                y: (lp >> 16) as i16 as i32,
            };
            let mut client: RECT = std::mem::zeroed();
            ScreenToClient(hwnd, &mut pt);
            GetClientRect(hwnd, &mut client);
            let dpi = GetDpiForWindow(hwnd) as f32 / 96.;
            let edge = (5. * dpi).ceil() as i32;
            if IsZoomed(hwnd) == 0 {
                let hit = match (
                    pt.x < edge,
                    pt.x >= client.right - edge,
                    pt.y < edge,
                    pt.y >= client.bottom - edge,
                ) {
                    (true, _, true, _) => HTTOPLEFT,
                    (_, true, true, _) => HTTOPRIGHT,
                    (true, _, _, true) => HTBOTTOMLEFT,
                    (_, true, _, true) => HTBOTTOMRIGHT,
                    (true, _, _, _) => HTLEFT,
                    (_, true, _, _) => HTRIGHT,
                    (_, _, true, _) => HTTOP,
                    (_, _, _, true) => HTBOTTOM,
                    _ => HTCLIENT,
                };
                if hit != HTCLIENT {
                    return hit as isize;
                }
            }
            if pt.y < (HEIGHT * dpi) as i32
                && pt.x < client.right - ((CONTROLS * CONTROL_WIDTH + DIVIDER_WIDTH) * dpi) as i32
            {
                return HTCAPTION as isize;
            }
        } else if msg == WM_GETMINMAXINFO {
            let result = DefSubclassProc(hwnd, msg, wp, lp);
            let mut monitor: MONITORINFO = std::mem::zeroed();
            monitor.cbSize = size_of::<MONITORINFO>() as u32;
            if GetMonitorInfoW(
                MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST),
                &mut monitor,
            ) != 0
            {
                let info = &mut *(lp as *mut MINMAXINFO);
                info.ptMaxPosition = POINT {
                    x: monitor.rcWork.left - monitor.rcMonitor.left,
                    y: monitor.rcWork.top - monitor.rcMonitor.top,
                };
                info.ptMaxSize = POINT {
                    x: monitor.rcWork.right - monitor.rcWork.left,
                    y: monitor.rcWork.bottom - monitor.rcWork.top,
                };
            }
            return result;
        } else if msg == WM_NCDESTROY {
            RemoveWindowSubclass(hwnd, Some(window_proc), id);
        }
        DefSubclassProc(hwnd, msg, wp, lp)
    }
}

pub fn show(
    ctx: &Context,
    brand: TextureId,
    close_to_tray: bool,
    can_undo: bool,
    can_redo: bool,
    profiles_open: bool,
    gallery_open: bool,
) -> Actions {
    TopBottomPanel::top("app bar")
        .exact_height(HEIGHT)
        .frame(Frame::new().fill(PANEL))
        .show(ctx, |ui| {
            let (area, _) =
                ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::hover());
            let draw = ui.painter().clone();
            draw.image(
                brand,
                Rect::from_center_size(area.left_center() + vec2(33., 0.), Vec2::splat(34.)),
                Rect::from_min_max(Pos2::ZERO, pos2(1., 1.)),
                Color32::WHITE,
            );
            let title =
                draw.layout_no_wrap("Multipapr".into(), super::heading_font(17.), Color32::WHITE);
            let title_origin = pos2(
                area.left() + 61.,
                area.center().y - title.mesh_bounds.center().y,
            );
            draw.galley(title_origin, title, Color32::WHITE);
            draw.hline(
                area.x_range(),
                area.bottom() - 0.5,
                Stroke::new(1.0_f32, LINE),
            );
            let slot = |n: f32| {
                let offset = if n > 6. { DIVIDER_WIDTH } else { 0. };
                Rect::from_min_size(
                    pos2(area.right() - n * CONTROL_WIDTH - offset, area.top()),
                    vec2(CONTROL_WIDTH, HEIGHT),
                )
            };
            let mut actions = Actions {
                settings: false,
                undo: false,
                redo: false,
                gallery: false,
                gallery_id: ui.id().with(GALLERY),
                profiles: false,
                profiles_anchor: slot(5.),
            };
            for (n, icon, label, enabled, action) in [
                (8., Icon::Undo, "Undo", can_undo, &mut actions.undo),
                (7., Icon::Redo, "Redo", can_redo, &mut actions.redo),
                (6., Icon::Gallery, GALLERY, true, &mut actions.gallery),
                (5., Icon::Profiles, "Profiles", true, &mut actions.profiles),
                (4., Icon::Cog, "Settings", true, &mut actions.settings),
            ] {
                let open = match icon {
                    Icon::Gallery => gallery_open,
                    Icon::Profiles => profiles_open,
                    _ => false,
                };
                let r = slot(n);
                let sense = if enabled {
                    Sense::click()
                } else {
                    Sense::hover()
                };
                let response = ui.interact(r, ui.id().with(label), sense);
                response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, enabled, label));
                if open || enabled && (response.hovered() || response.has_focus()) {
                    draw.rect_filled(r.shrink2(vec2(5., 6.)), 7, LINE);
                }
                let tint = if open {
                    ACCENT
                } else if enabled {
                    MUTED
                } else {
                    Color32::from_gray(78)
                };
                icon.paint(&draw, r.center(), tint);
                *action = response.clicked();
                if matches!(icon, Icon::Gallery | Icon::Profiles) && !open {
                    response.hint(label);
                }
            }
            let divider = slot(6.).left() - DIVIDER_WIDTH / 2.;
            draw.vline(
                divider,
                Rangef::new(area.center().y - 10., area.center().y + 10.),
                Stroke::new(1.0_f32, LINE),
            );
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            let labels = [
                "Minimise",
                if maximized { "Restore" } else { "Maximise" },
                if close_to_tray {
                    "Close to tray"
                } else {
                    "Close"
                },
            ];
            for (i, label) in labels.into_iter().enumerate() {
                let r = Rect::from_min_size(
                    pos2(area.right() - (3 - i) as f32 * CONTROL_WIDTH, area.top()),
                    vec2(CONTROL_WIDTH, HEIGHT),
                );
                let response = ui.interact(r, ui.id().with(i), Sense::click());
                response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
                let c = r.center();
                let pen = Stroke::new(
                    1.25_f32,
                    if response.hovered() {
                        Color32::WHITE
                    } else {
                        MUTED
                    },
                );
                if response.hovered() || response.has_focus() {
                    draw.rect_filled(
                        r,
                        0,
                        if i == 2 {
                            Color32::from_rgb(175, 58, 73)
                        } else {
                            LINE
                        },
                    );
                }
                match i {
                    0 => {
                        draw.line_segment([c + vec2(-5., 0.), c + vec2(5., 0.)], pen);
                    }
                    1 => {
                        let square = Rect::from_center_size(
                            c,
                            Vec2::splat(if maximized { 8. } else { 10. }),
                        );
                        if maximized {
                            draw.rect_stroke(
                                square.translate(vec2(1.5, -1.5)),
                                0,
                                pen,
                                StrokeKind::Inside,
                            );
                        }
                        draw.rect(
                            square.translate(if maximized { vec2(-1., 1.) } else { Vec2::ZERO }),
                            0,
                            if response.hovered() { LINE } else { PANEL },
                            pen,
                            StrokeKind::Inside,
                        );
                    }
                    _ => {
                        draw.line_segment([c + vec2(-5., -5.), c + vec2(5., 5.)], pen);
                        draw.line_segment([c + vec2(-5., 5.), c + vec2(5., -5.)], pen);
                    }
                }
                if response.clicked() {
                    ctx.send_viewport_cmd(match i {
                        0 => ViewportCommand::Minimized(true),
                        1 => ViewportCommand::Maximized(!maximized),
                        _ => ViewportCommand::Close,
                    });
                }
            }
            actions
        })
        .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caption_buttons_request_the_correct_window_action() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let run = |events, maximized| {
            let mut input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900., 600.))),
                events,
                ..Default::default()
            };
            input
                .viewports
                .get_mut(&ViewportId::ROOT)
                .unwrap()
                .maximized = Some(maximized);
            ctx.run(input, |ctx| {
                show(ctx, TextureId::default(), true, false, false, false, false);
            })
        };
        for (column, maximized, expected) in [
            (0, false, ViewportCommand::Minimized(true)),
            (1, false, ViewportCommand::Maximized(true)),
            (1, true, ViewportCommand::Maximized(false)),
            (2, false, ViewportCommand::Close),
        ] {
            let p = pos2(900. - (2.5 - column as f32) * CONTROL_WIDTH, HEIGHT / 2.);
            let button = |pressed| Event::PointerButton {
                pos: p,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            run(vec![Event::PointerMoved(p)], maximized);
            run(vec![button(true)], maximized);
            let out = run(vec![button(false)], maximized);
            assert!(
                out.viewport_output[&ViewportId::ROOT]
                    .commands
                    .contains(&expected)
            );
        }
    }

    #[test]
    fn title_bar_divider_has_even_padding_matching_button_gaps() {
        let ctx = Context::default();
        super::super::set_fonts(&ctx);
        let output = ctx.run(
            RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(900., 600.))),
                ..Default::default()
            },
            |ctx| {
                show(ctx, TextureId::default(), true, true, true, false, true);
            },
        );
        let divider_x = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                Shape::LineSegment { points, .. }
                    if (points[0].x - points[1].x).abs() < 0.01
                        && (points[1].y - points[0].y - 20.).abs() < 0.5 =>
                {
                    Some(points[0].x)
                }
                _ => None,
            })
            .expect("title bar divider");
        let gallery_hover = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                Shape::Rect(r) if r.fill == LINE && r.corner_radius == CornerRadius::same(7) => {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("open gallery highlight");
        let redo_right = 900. - 6. * CONTROL_WIDTH - DIVIDER_WIDTH - 5.;
        let left_pad = divider_x - redo_right;
        let right_pad = gallery_hover.left() - divider_x;
        assert!((left_pad - right_pad).abs() < 0.01, "{left_pad} vs {right_pad}");
        assert!((left_pad - 10.5).abs() < 0.01, "expected consistent padding around divider, got {left_pad}");
    }
}
