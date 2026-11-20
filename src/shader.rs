use crate::layout::{Display, Layout, Settings};
use std::fmt::Write;

// Only the destination pixel rectangles are baked into the shader. The four
// source coordinates per display are uniforms, updated while the decoder runs.
pub fn generate(displays: &[Display], layout: &Layout) -> String {
    let desktop = layout.desktop;
    let mut code = String::new();
    for i in 0..displays.len() {
        for name in ["u", "v", "w", "h"] {
            writeln!(code, "//!PARAM {name}{i}\n//!TYPE DYNAMIC float\n0.0\n").unwrap();
        }
    }
    writeln!(code, "//!HOOK MAIN\n//!BIND HOOKED\n//!WIDTH {:.0}\n//!HEIGHT {:.0}\n//!DESC Multipapr display mapping\nvec4 hook() {{\n vec2 p = HOOKED_pos * vec2({:.1}, {:.1});",
        desktop.w, desktop.h, desktop.w, desktop.h).unwrap();
    for (i, display) in displays.iter().enumerate() {
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
        writeln!(code, "  vec2 uv = (p - vec2({x:.1}, {y:.1})) / vec2({:.1}, {:.1}) * vec2(w{i}, h{i}) + vec2(u{i}, v{i});", r.w, r.h).unwrap();
        code.push_str("  if (all(greaterThanEqual(uv, vec2(0.0))) && all(lessThan(uv, vec2(1.0)))) return HOOKED_tex(uv);\n }\n");
    }
    code.push_str(" return vec4(0.0, 0.0, 0.0, 1.0);\n}\n");
    code
}

pub fn parameters(layout: &Layout, settings: &Settings) -> String {
    let source = layout.media(settings);
    let mut values = Vec::with_capacity(layout.panels.len());
    for (i, panel) in layout.panels.iter().enumerate() {
        values.push(format!(
            "u{i}={:.9},v{i}={:.9},w{i}={:.9},h{i}={:.9}",
            (panel.x - source.x) / source.w,
            (panel.y - source.y) / source.h,
            panel.w / source.w,
            panel.h / source.h
        ));
    }
    values.join(",")
}
