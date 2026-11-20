use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}
impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(self) -> f32 {
        self.y + self.h
    }
    pub fn union(self, b: Self) -> Self {
        let x = self.x.min(b.x);
        let y = self.y.min(b.y);
        Self::new(
            x,
            y,
            self.right().max(b.right()) - x,
            self.bottom().max(b.bottom()) - y,
        )
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Display {
    pub id: String,
    pub name: String,
    pub primary: bool,
    pub px: Rect,
    pub mm: [f32; 2],
    pub estimated: bool,
}

#[derive(Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Calibration {
    pub offset: [f32; 2],
    pub position: Option<[f32; 2]>,
    pub size: Option<[f32; 2]>,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}
// Resampling filter used by the display mapping shader. Values match the
// shader's `upscaler` parameter; the kernels follow mpv's filter definitions.
#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Upscaling {
    None,
    #[default]
    CatmullRom,
    Spline36,
    Lanczos,
    EwaLanczosSharp,
}
impl Upscaling {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::CatmullRom,
        Self::Spline36,
        Self::Lanczos,
        Self::EwaLanczosSharp,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::CatmullRom => "Catmull-Rom",
            Self::Spline36 => "Spline36",
            Self::Lanczos => "Lanczos",
            Self::EwaLanczosSharp => "EWA Lanczos Sharp",
        }
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub media: Media,
    pub scale: f32,
    pub pan: [f32; 2],
    pub calibration: BTreeMap<String, Calibration>,
    #[serde(default)]
    pub upscaling: Upscaling,
}
impl Settings {
    pub fn new(media: Media) -> Self {
        Self {
            media,
            scale: 1.,
            pan: [0.; 2],
            calibration: BTreeMap::new(),
            upscaling: Upscaling::default(),
        }
    }
}

pub struct Layout {
    pub parents: Vec<usize>,
    pub panels: Vec<Rect>,
    pub bounds: Rect,
    pub desktop: Rect,
    pitch: f32,
}

// Preserve the nearest aligned edge or centre on the axis parallel to a seam.
fn parallel(a: f32, aw: f32, b: f32, bw: f32, physical_a: f32, physical_b: f32) -> f32 {
    let anchor = [0., 0.5, 1.]
        .into_iter()
        .min_by(|u, v| {
            let delta = |t: f32| ((b + bw * t) - (a + aw * t)).abs();
            delta(*u).total_cmp(&delta(*v))
        })
        .unwrap();
    (physical_a - physical_b) * anchor + ((b - a) + (bw - aw) * anchor) * physical_a / aw
}

impl Layout {
    // Displays are primary-first. Attach each new panel to its closest placed neighbour.
    pub fn new(displays: &[Display], calibration: &BTreeMap<String, Calibration>) -> Self {
        let mut panels: Vec<_> = displays
            .iter()
            .map(|d| {
                let [w, h] = calibration.get(&d.id).and_then(|c| c.size).unwrap_or(d.mm);
                Rect::new(0., 0., w, h)
            })
            .collect();
        let mut placed = vec![false; displays.len()];
        let mut parents = vec![0; displays.len()];
        placed[0] = true;
        if let Some(c) = calibration.get(&displays[0].id) {
            [panels[0].x, panels[0].y] = c.position.unwrap_or(c.offset);
        }
        for _ in 1..displays.len() {
            let mut best = (f32::INFINITY, 0, 0);
            for (i, a) in displays.iter().enumerate().filter(|(i, _)| placed[*i]) {
                for (j, b) in displays.iter().enumerate().filter(|(j, _)| !placed[*j]) {
                    let a = a.px;
                    let b = b.px;
                    let dx = (a.x - b.right()).max(b.x - a.right()).max(0.);
                    let dy = (a.y - b.bottom()).max(b.y - a.bottom()).max(0.);
                    let distance = (dx + dy) * 10000.
                        + (a.x + a.w / 2. - b.x - b.w / 2.).hypot(a.y + a.h / 2. - b.y - b.h / 2.);
                    if distance < best.0 {
                        best = (distance, i, j);
                    }
                }
            }
            let (_, i, j) = best;
            parents[j] = i;
            let (a, b) = (displays[i].px, displays[j].px);
            let (pa, pb) = (panels[i], panels[j]);
            let x_edge = (b.x - a.right()).max(a.x - b.right());
            let y_edge = (b.y - a.bottom()).max(a.y - b.bottom());
            let join = |a: f32, aw: f32, b: f32, bw: f32, am: f32, bm: f32| {
                if b >= a {
                    am + (b - a - aw) * am / aw
                } else {
                    (b + bw - a) * am / aw - bm
                }
            };
            let (x, y) = if x_edge >= y_edge {
                (
                    join(a.x, a.w, b.x, b.w, pa.w, pb.w),
                    parallel(a.y, a.h, b.y, b.h, pa.h, pb.h),
                )
            } else {
                (
                    parallel(a.x, a.w, b.x, b.w, pa.w, pb.w),
                    join(a.y, a.h, b.y, b.h, pa.h, pb.h),
                )
            };
            let offset = calibration
                .get(&displays[j].id)
                .map(|c| c.offset)
                .unwrap_or_default();
            panels[j].x = pa.x + x + offset[0];
            panels[j].y = pa.y + y + offset[1];
            if let Some(position) = calibration.get(&displays[j].id).and_then(|c| c.position) {
                [panels[j].x, panels[j].y] = position;
            }
            placed[j] = true;
        }
        Self {
            parents,
            bounds: panels.iter().copied().reduce(Rect::union).unwrap(),
            desktop: displays.iter().map(|d| d.px).reduce(Rect::union).unwrap(),
            pitch: panels[0].w / displays[0].px.w,
            panels,
        }
    }
    pub fn move_panel(
        &self,
        displays: &[Display],
        settings: &mut Settings,
        index: usize,
        delta: [f32; 2],
    ) {
        let mut panels = self.panels.clone();
        panels[index].x += delta[0];
        panels[index].y += delta[1];
        // Freeze this arrangement by monitor identity. Later Windows changes
        // only change the pixel destinations, not the calibrated desk.
        for (d, panel) in displays.iter().zip(&panels) {
            settings
                .calibration
                .entry(d.id.clone())
                .or_default()
                .position = Some([panel.x, panel.y]);
        }
        let bounds = panels.into_iter().reduce(Rect::union).unwrap();
        // Media placement is relative to the centre of the display bounds.
        settings.pan[0] += self.bounds.x - bounds.x + (self.bounds.w - bounds.w) / 2.;
        settings.pan[1] += self.bounds.y - bounds.y + (self.bounds.h - bounds.h) / 2.;
    }
    pub fn media(&self, settings: &Settings) -> Rect {
        let w = settings.media.width as f32 * self.pitch * settings.scale;
        let h = settings.media.height as f32 * self.pitch * settings.scale;
        Rect::new(
            self.bounds.x + (self.bounds.w - w) / 2. + settings.pan[0],
            self.bounds.y + (self.bounds.h - h) / 2. + settings.pan[1],
            w,
            h,
        )
    }
    pub fn cover(&self, settings: &mut Settings) {
        settings.scale = (self.bounds.w / settings.media.width as f32)
            .max(self.bounds.h / settings.media.height as f32)
            / self.pitch;
        settings.pan = [0.; 2];
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn display(id: &str, px: Rect, mm: [f32; 2]) -> Display {
        Display {
            id: id.into(),
            name: id.into(),
            primary: id == "a",
            px,
            mm,
            estimated: false,
        }
    }
    #[test]
    fn portrait_gap_and_negative_origin() {
        let displays = [
            display("a", Rect::new(0., 0., 2560., 1440.), [600., 340.]),
            display("b", Rect::new(-1080., -240., 1080., 1920.), [280., 530.]),
        ];
        let l = Layout::new(
            &displays,
            &BTreeMap::from([(
                "b".into(),
                Calibration {
                    offset: [-12., 3.],
                    position: None,
                    size: None,
                },
            )]),
        );
        assert_eq!(l.panels[1], Rect::new(-292., -92., 280., 530.));
        assert_eq!(l.desktop, Rect::new(-1080., -240., 3640., 1920.));
    }
    #[test]
    fn moved_panels_keep_their_physical_positions_and_the_wallpaper_stays_put() {
        let mut displays = [
            display("a", Rect::new(0., 0., 1920., 1080.), [480., 270.]),
            display("b", Rect::new(1920., 0., 1200., 1920.), [300., 480.]),
            display("c", Rect::new(3120., 0., 1920., 1080.), [480., 270.]),
        ];
        let mut settings = Settings::new(Media {
            path: "clip".into(),
            width: 3840,
            height: 2160,
        });
        let initial = Layout::new(&displays, &settings.calibration);
        let image = initial.media(&settings);
        let mut expected = initial.panels.clone();
        for index in [1, 0, 2] {
            let layout = Layout::new(&displays, &settings.calibration);
            layout.move_panel(&displays, &mut settings, index, [34., -11.]);
            expected[index].x += 34.;
            expected[index].y -= 11.;
            let moved = Layout::new(&displays, &settings.calibration);
            assert_eq!(moved.panels, expected);
            assert_eq!(moved.media(&settings), image);
        }
        let restored: Settings =
            serde_json::from_slice(&serde_json::to_vec(&settings).unwrap()).unwrap();
        displays[1].px.x = -1200.;
        displays.swap(1, 2);
        let refreshed = Layout::new(&displays, &restored.calibration);
        assert_eq!(
            refreshed.panels,
            vec![expected[0], expected[2], expected[1]]
        );
        assert_eq!(refreshed.media(&restored), image);
    }
    #[test]
    fn native_scale_and_cover() {
        let d = [display("a", Rect::new(0., 0., 1920., 1080.), [480., 270.])];
        let l = Layout::new(&d, &BTreeMap::new());
        let mut s = Settings::new(Media {
            path: "test".into(),
            width: 3840,
            height: 2160,
        });
        assert_eq!(l.media(&s).w, l.bounds.w * 2.);
        l.cover(&mut s);
        assert_eq!(l.media(&s), l.bounds);
    }
}
