use crate::layout::{Display, Layout, Rect, Settings};
use std::fmt::Write;
use std::path::PathBuf;

// Definitions follow mpv's filter_kernels.c.
const SAMPLING: &str = r#"
float bessel_j1(float x) {
    float ax = abs(x);
    if (ax < 8.0) {
        float y = x * x;
        float a = x * (72362614232.0 + y * (-7895059235.0 + y * (242396853.1 + y * (-2972611.439 + y * (15704.48260 + y * -30.16036606)))));
        float b = 144725228442.0 + y * (2300535178.0 + y * (18583304.74 + y * (99447.43394 + y * (376.9991397 + y))));
        return a / b;
    }
    float z = 8.0 / ax;
    float y = z * z;
    float xx = ax - 2.356194491;
    float a = 1.0 + y * (0.183105e-2 + y * (-0.3516396496e-4 + y * (0.2457520174e-5 + y * -0.240337019e-6)));
    float b = 0.04687499995 + y * (-0.2002690873e-3 + y * (0.8449199096e-5 + y * (-0.88228987e-6 + y * 0.105787412e-6)));
    float r = sqrt(0.636619772 / ax) * (cos(xx) * a - z * sin(xx) * b);
    return x < 0.0 ? -r : r;
}
float jinc(float x) {
    if (x < 1e-5) return 1.0;
    float p = 3.14159265 * x;
    return 2.0 * bessel_j1(p) / p;
}
float kernel(float x) {
    x = abs(x);
    if (upscaler == 1) {
        if (x < 1.0) return (1.5 * x - 2.5) * x * x + 1.0;
        if (x < 2.0) return ((-0.5 * x + 2.5) * x - 4.0) * x + 2.0;
        return 0.0;
    }
    if (upscaler == 2) {
        if (x < 1.0) return ((13.0 / 11.0 * x - 453.0 / 209.0) * x - 3.0 / 209.0) * x + 1.0;
        if (x < 2.0) { x -= 1.0; return ((-6.0 / 11.0 * x + 270.0 / 209.0) * x - 156.0 / 209.0) * x; }
        if (x < 3.0) { x -= 2.0; return ((1.0 / 11.0 * x - 45.0 / 209.0) * x + 26.0 / 209.0) * x; }
        return 0.0;
    }
    if (x < 1e-5) return 1.0;
    if (x >= 3.0) return 0.0;
    float p = 3.14159265 * x;
    return 3.0 * sin(p) * sin(p / 3.0) / (p * p);
}
vec4 source(vec2 uv) {
    if (upscaler == 0) return HOOKED_tex(uv);
    vec2 pos = uv * HOOKED_size - 0.5;
    vec2 base = floor(pos);
    vec2 f = pos - base;
    int r = upscaler == 1 ? 2 : upscaler == 4 ? 4 : 3;
    vec4 sum = vec4(0.0);
    float total = 0.0;
    for (int y = 1 - r; y <= r; y++) {
        for (int x = 1 - r; x <= r; x++) {
            vec2 d = vec2(float(x), float(y)) - f;
            float w;
            if (upscaler == 4) {
                const float radius = 3.2383154841662362;
                float dist = length(d);
                if (dist >= radius) continue;
                w = jinc(dist / 0.9812505644269356) * jinc(dist * 1.2196698912665045 / radius);
            } else {
                w = kernel(d.x) * kernel(d.y);
            }
            sum += w * HOOKED_tex((base + vec2(float(x), float(y)) + 0.5) * HOOKED_pt);
            total += w;
        }
    }
    return sum / total;
}
"#;

#[derive(Clone, PartialEq)]
pub struct Part {
    pub media: PathBuf,
    pub displays: Vec<usize>,
}
impl Part {
    pub fn missing_wallpaper(&self, multi: bool) -> String {
        missing_wallpaper(
            self.displays.first().copied().filter(|_| multi),
            self.media.file_name(),
        )
    }
    pub fn desktop(&self, displays: &[Display]) -> Rect {
        self.displays
            .iter()
            .map(|&i| displays[i].px)
            .reduce(Rect::union)
            .unwrap()
    }
}

pub fn missing_wallpaper(display: Option<usize>, name: Option<&std::ffi::OsStr>) -> String {
    let target = display.map_or_else(|| "span".into(), |index| format!("Display {}", index + 1));
    let name = name.map_or_else(String::new, |name| format!(" ({})", name.to_string_lossy()));
    format!("Missing {target} wallpaper{name}. Please reselect one or choose from the Gallery.")
}
pub fn plan(settings: &Settings, displays: &[Display]) -> Vec<Part> {
    let parts = if settings.multi {
        (0..displays.len())
            .map(|i| Part {
                media: settings.shown(&displays[i].id).media.path.clone(),
                displays: vec![i],
            })
            .collect()
    } else {
        vec![Part {
            media: settings.wallpaper.media.path.clone(),
            displays: (0..displays.len()).collect(),
        }]
    };
    parts
        .into_iter()
        .filter(|p| !p.media.as_os_str().is_empty())
        .collect()
}

pub fn generate(displays: &[Display], part: &Part) -> String {
    let desktop = part.desktop(displays);
    let mut code = String::new();
    for k in 0..part.displays.len() {
        for name in ["u", "v", "w", "h"] {
            writeln!(code, "//!PARAM {name}{k}\n//!TYPE DYNAMIC float\n0.0\n").unwrap();
        }
    }
    code.push_str("//!PARAM upscaler\n//!TYPE DYNAMIC int\n1\n\n");
    writeln!(code, "//!HOOK MAIN\n//!BIND HOOKED\n//!WIDTH {:.0}\n//!HEIGHT {:.0}\n//!DESC Multipapr display mapping\n{SAMPLING}\nvec4 hook() {{\n vec2 p = HOOKED_pos * vec2({:.1}, {:.1});",
        desktop.w, desktop.h, desktop.w, desktop.h).unwrap();
    for (k, &i) in part.displays.iter().enumerate() {
        let r = displays[i].px;
        let x = r.x - desktop.x;
        let y = r.y - desktop.y;
        writeln!(
            code,
            " if (p.x >= {x:.1} && p.y >= {y:.1} && p.x < {:.1} && p.y < {:.1}) {{",
            x + r.w,
            y + r.h
        )
        .unwrap();
        writeln!(code, "  vec2 uv = (p - vec2({x:.1}, {y:.1})) / vec2({:.1}, {:.1}) * vec2(w{k}, h{k}) + vec2(u{k}, v{k});", r.w, r.h).unwrap();
        code.push_str("  if (all(greaterThanEqual(uv, vec2(0.0))) && all(lessThan(uv, vec2(1.0)))) return source(uv);\n }\n");
    }
    code.push_str(" return vec4(0.0, 0.0, 0.0, 1.0);\n}\n");
    code
}

pub fn parameters(
    layout: &Layout,
    settings: &Settings,
    displays: &[Display],
    part: &Part,
) -> String {
    let mut values = Vec::with_capacity(part.displays.len() + 1);
    for (k, &i) in part.displays.iter().enumerate() {
        let panel = layout.panels[i];
        let source = layout.media(settings, settings.shown(&displays[i].id));
        values.push(format!(
            "u{k}={:.9},v{k}={:.9},w{k}={:.9},h{k}={:.9}",
            (panel.x - source.x) / source.w,
            (panel.y - source.y) / source.h,
            panel.w / source.w,
            panel.h / source.h
        ));
    }
    values.push(format!("upscaler={}", settings.upscaling as u8));
    values.join(",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Media, Wallpaper};

    fn display(id: &str, x: f32) -> Display {
        Display {
            id: id.into(),
            name: id.into(),
            primary: id == "a",
            px: Rect::new(x, 0., 1920., 1080.),
            mm: [480., 270.],
            estimated: false,
            mirrors: Vec::new(),
            mirror_ids: Vec::new(),
        }
    }

    #[test]
    fn multi_mode_plays_each_display_on_its_own_and_skips_empty_ones() {
        let displays = [display("a", 0.), display("b", 1920.), display("c", 3840.)];
        let media = |path: &str| Media {
            path: path.into(),
            width: 1920,
            height: 1080,
        };
        let mut settings = Settings::new(media("span.mp4"));
        assert!(
            plan(&settings, &displays)
                == [Part {
                    media: "span.mp4".into(),
                    displays: vec![0, 1, 2],
                }]
        );
        settings.multi = true;
        settings
            .screens
            .insert("b".into(), Wallpaper::new(media("b.png")));
        settings
            .screens
            .insert("c".into(), Wallpaper::new(media("")));
        let parts = plan(&settings, &displays);
        let files: Vec<_> = parts.iter().map(|p| p.media.to_str().unwrap()).collect();
        assert_eq!(files, ["span.mp4", "b.png"]);
        assert_eq!(parts[1].displays, [1]);
        assert_eq!(parts[1].desktop(&displays), displays[1].px);
        let code = generate(&displays, &parts[1]);
        assert!(code.contains("//!PARAM u0") && !code.contains("//!PARAM u1"));
        let pipe = crate::host::player_pipe(42, settings.multi, &parts[1], &displays);
        assert_ne!(
            pipe,
            crate::host::player_pipe(42, !settings.multi, &parts[1], &displays)
        );
        let layout = Layout::new(&displays, &settings.calibration);
        let pitch = layout.pitch(&settings);
        settings
            .screens
            .get_mut("b")
            .unwrap()
            .cover(pitch, layout.panels[1]);
        assert!(
            parameters(&layout, &settings, &displays, &parts[1])
                .starts_with("u0=0.000000000,v0=0.000000000,w0=1.000000000,h0=1.000000000")
        );
        settings.screens.get_mut("b").unwrap().media.path = "other.jpg".into();
        assert_ne!(
            crate::host::player_pipe(
                42,
                settings.multi,
                &plan(&settings, &displays)[1],
                &displays
            ),
            pipe
        );
        let mut moved = displays.clone();
        moved[1].px.x = -1920.;
        assert_ne!(
            crate::host::player_pipe(42, settings.multi, &parts[1], &moved),
            pipe
        );
    }
}
