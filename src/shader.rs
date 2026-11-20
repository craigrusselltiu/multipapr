use crate::layout::{Display, Layout, Settings};
use std::fmt::Write;

// mpv's MAIN hook operates on the decoded RGB texture. Render directly at
// virtual-desktop resolution, so the final presentation needs no extra resize.
pub fn generate(displays: &[Display], layout: &Layout, settings: &Settings) -> String {
    let desktop = layout.desktop;
    let source = layout.media(settings);
    let mut code = format!(
        "//!HOOK MAIN\n//!BIND HOOKED\n//!WIDTH {:.0}\n//!HEIGHT {:.0}\n//!DESC Multipapr physical display mapping\nvec4 hook() {{\n vec2 p = HOOKED_pos * vec2({:.1}, {:.1});\n",
        desktop.w, desktop.h, desktop.w, desktop.h
    );
    for (display, panel) in displays.iter().zip(&layout.panels) {
        let r = display.px;
        let x = r.x - desktop.x;
        let y = r.y - desktop.y;
        writeln!(
            code,
            " if (p.x >= {x:.1} && p.y >= {y:.1} && p.x < {:.1} && p.y < {:.1}) {{",
            x + r.w,
            y + r.h
        )
        .unwrap();
        writeln!(
            code,
            "  vec2 uv = (p - vec2({x:.1}, {y:.1})) * vec2({:.12}, {:.12}) + vec2({:.12}, {:.12});",
            panel.w / (r.w * source.w),
            panel.h / (r.h * source.h),
            (panel.x - source.x) / source.w,
            (panel.y - source.y) / source.h
        )
        .unwrap();
        code.push_str("  if (all(greaterThanEqual(uv, vec2(0.0))) && all(lessThan(uv, vec2(1.0)))) return HOOKED_tex(uv);\n }\n");
    }
    code.push_str(" return vec4(0.0, 0.0, 0.0, 1.0);\n}\n");
    code
}
