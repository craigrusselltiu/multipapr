use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

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
    pub fn center(self) -> [f32; 2] {
        [self.x + self.w / 2., self.y + self.h / 2.]
    }
    pub fn overlaps(self, b: Self) -> bool {
        self.x < b.right() - 0.5
            && b.x < self.right() - 0.5
            && self.y < b.bottom() - 0.5
            && b.y < self.bottom() - 0.5
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
    pub mirrors: Vec<String>,
    #[serde(skip)]
    pub mirror_ids: Vec<String>,
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

impl Default for Media {
    fn default() -> Self {
        Self {
            path: PathBuf::new(),
            width: 1920,
            height: 1080,
        }
    }
}
#[derive(Clone, Copy, Default, Debug, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum Upscaling {
    None = 0,
    #[default]
    CatmullRom = 1,
    Spline36 = 2,
    Lanczos = 3,
    EwaLanczosSharp = 4,
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
fn one() -> f32 {
    1.
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Wallpaper {
    pub media: Media,
    pub scale: f32,
    #[serde(default = "one")]
    pub stretch: f32,
    #[serde(default)]
    pub center: [f32; 2],
    #[serde(default)]
    pub paused: bool,
}
impl Wallpaper {
    pub fn new(media: Media) -> Self {
        Self {
            media,
            scale: 1.,
            stretch: 1.,
            center: [0.; 2],
            paused: false,
        }
    }
    pub fn is_empty(&self) -> bool {
        self.media.path.as_os_str().is_empty()
    }
    pub fn clear(&mut self) {
        *self = Self::new(Media::default());
    }
    pub fn clear_if(&mut self, matches: &impl Fn(&Path) -> bool) -> bool {
        if self.is_empty() || !matches(&self.media.path) {
            return false;
        }
        self.clear();
        true
    }
    pub fn rect(&self, pitch: f32) -> Rect {
        let w = self.media.width as f32 * pitch * self.scale * self.stretch;
        let h = self.media.height as f32 * pitch * self.scale;
        let [x, y] = self.center;
        Rect::new(x - w / 2., y - h / 2., w, h)
    }
    pub fn fill(&mut self, target: Rect) {
        self.scale = 1.;
        self.stretch = 1.;
        self.center = target.center();
    }
    pub fn cover(&mut self, pitch: f32, target: Rect) {
        self.scale =
            (target.w / self.media.width as f32).max(target.h / self.media.height as f32) / pitch;
        self.stretch = 1.;
        self.center = target.center();
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    #[serde(flatten)]
    pub wallpaper: Wallpaper,
    #[serde(default)]
    pub pitch: Option<f32>,
    pub calibration: BTreeMap<String, Calibration>,
    #[serde(default)]
    pub upscaling: Upscaling,
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub display_set: String,
    #[serde(default)]
    pub multi: bool,
    #[serde(default)]
    pub screens: BTreeMap<String, Wallpaper>,
}
impl Settings {
    pub fn is_empty(&self) -> bool {
        self.wallpaper.is_empty() && self.screens.values().all(Wallpaper::is_empty)
    }
    pub fn new(media: Media) -> Self {
        Self {
            wallpaper: Wallpaper::new(media),
            pitch: None,
            calibration: BTreeMap::new(),
            upscaling: Upscaling::default(),
            profile: None,
            display_set: String::new(),
            multi: false,
            screens: BTreeMap::new(),
        }
    }
    pub fn shown(&self, id: &str) -> &Wallpaper {
        match self.screens.get(id) {
            Some(w) if self.multi => w,
            _ => &self.wallpaper,
        }
    }
    pub fn restore_multi_wallpapers(
        &mut self,
        displays: &[Display],
        layout: &Layout,
        available: impl Fn(&Path) -> bool,
    ) {
        if !self.multi {
            return;
        }
        self.wallpaper.clear_if(&|path| !available(path));
        let pitch = layout.pitch(self);
        for (display, panel) in displays.iter().zip(&layout.panels) {
            if self
                .screens
                .get(&display.id)
                .is_some_and(|wallpaper| wallpaper.is_empty() || available(&wallpaper.media.path))
            {
                continue;
            }
            if self.wallpaper.is_empty() {
                self.screens.remove(&display.id);
            } else {
                let mut wallpaper = Wallpaper::new(self.wallpaper.media.clone());
                wallpaper.paused = self.wallpaper.paused;
                wallpaper.cover(pitch, *panel);
                self.screens.insert(display.id.clone(), wallpaper);
            }
        }
    }
    pub fn clear_wallpapers(&mut self, matches: &impl Fn(&Path) -> bool) {
        self.wallpaper.clear_if(matches);
        forget_screen_wallpapers(&mut self.screens, self.multi, matches);
    }
}

fn forget_screen_wallpapers(
    screens: &mut BTreeMap<String, Wallpaper>,
    multi: bool,
    matches: &impl Fn(&Path) -> bool,
) -> bool {
    if multi {
        let mut changed = false;
        for wallpaper in screens.values_mut() {
            changed |= wallpaper.clear_if(matches);
        }
        changed
    } else {
        let before = screens.len();
        screens.retain(|_, wallpaper| wallpaper.is_empty() || !matches(&wallpaper.media.path));
        screens.len() != before
    }
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Framing {
    pub scale: f32,
    #[serde(default = "one")]
    pub stretch: f32,
    pub center: [f32; 2],
}
impl Framing {
    fn apply(&self, wallpaper: &mut Wallpaper) {
        wallpaper.scale = self.scale;
        wallpaper.stretch = self.stretch;
        wallpaper.center = self.center;
    }
}

pub const DEFAULT_PROFILE: &str = "Default";

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub media: Media,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub upscaling: Upscaling,
    #[serde(default)]
    pub framings: BTreeMap<String, Framing>,
    #[serde(default)]
    pub multi: bool,
    #[serde(default)]
    pub screens: BTreeMap<String, Wallpaper>,
}
impl Profile {
    pub fn is_empty(&self) -> bool {
        self.media.path.as_os_str().is_empty() && self.screens.values().all(Wallpaper::is_empty)
    }
    pub fn clear_wallpapers(&mut self, matches: &impl Fn(&Path) -> bool) -> bool {
        let mut changed = !self.media.path.as_os_str().is_empty() && matches(&self.media.path);
        if changed {
            self.media = Media::default();
            self.paused = false;
            self.framings.clear();
        }
        changed |= forget_screen_wallpapers(&mut self.screens, self.multi, matches);
        changed
    }
    pub fn new(name: &str, s: &Settings, set: &str) -> Self {
        let mut profile = Self {
            name: name.into(),
            media: s.wallpaper.media.clone(),
            paused: s.wallpaper.paused,
            upscaling: s.upscaling,
            framings: BTreeMap::new(),
            multi: s.multi,
            screens: BTreeMap::new(),
        };
        profile.record(s, set);
        profile
    }
    pub fn record(&mut self, s: &Settings, set: &str) {
        if self.media != s.wallpaper.media {
            self.framings.clear();
        }
        self.media = s.wallpaper.media.clone();
        self.paused = s.wallpaper.paused;
        self.upscaling = s.upscaling;
        self.multi = s.multi;
        self.screens = s.screens.clone();
        let w = &s.wallpaper;
        self.framings.insert(
            set.into(),
            Framing {
                scale: w.scale,
                stretch: w.stretch,
                center: w.center,
            },
        );
    }
    pub fn apply(&self, s: &mut Settings, layout: &Layout, set: &str) {
        s.wallpaper.media = self.media.clone();
        s.wallpaper.paused = self.paused;
        s.upscaling = self.upscaling;
        s.profile = Some(self.name.clone());
        s.screens = self.screens.clone();
        s.display_set = set.into();
        match self.framings.get(set) {
            Some(f) => f.apply(&mut s.wallpaper),
            None => {
                let pitch = layout.pitch(s);
                s.wallpaper.cover(pitch, layout.bounds);
            }
        }
    }
}
pub fn display_set(displays: &[Display]) -> String {
    let mut ids: Vec<String> = displays
        .iter()
        .map(|d| {
            let mut group = d.mirror_ids.clone();
            group.sort_unstable();
            group.insert(0, d.id.clone());
            group.join("+")
        })
        .collect();
    ids.sort_unstable();
    ids.join("|")
}
pub fn unique_name(profiles: &[Profile], base: &str) -> String {
    let taken = |name: &str| profiles.iter().any(|p| p.name == name);
    if !taken(base) {
        return base.into();
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| !taken(name))
        .unwrap()
}
pub fn ensure_default(profiles: &mut Vec<Profile>, s: &Settings, set: &str) -> bool {
    match profiles.iter().position(|p| p.name == DEFAULT_PROFILE) {
        Some(0) => false,
        Some(i) => {
            let default = profiles.remove(i);
            profiles.insert(0, default);
            true
        }
        None => {
            profiles.insert(0, Profile::new(DEFAULT_PROFILE, s, set));
            true
        }
    }
}
pub fn remember(profiles: &mut Vec<Profile>, s: &mut Settings, set: &str) {
    s.display_set = set.into();
    ensure_default(profiles, s, set);
    let active = s
        .profile
        .as_ref()
        .and_then(|name| profiles.iter().position(|p| &p.name == name));
    let i = active.unwrap_or_else(|| {
        s.profile = Some(DEFAULT_PROFILE.into());
        0
    });
    profiles[i].record(s, set);
}

pub fn follow_displays(profiles: &mut Vec<Profile>, s: &mut Settings, set: &str) {
    if !s.display_set.is_empty() && s.display_set != set {
        let previous = s.display_set.clone();
        remember(profiles, s, &previous);
    }
    if s.display_set != set
        && let Some(profile) = profiles
            .iter()
            .find(|p| Some(&p.name) == s.profile.as_ref())
        && profile.media == s.wallpaper.media
        && let Some(framing) = profile.framings.get(set)
    {
        framing.apply(&mut s.wallpaper);
    }
    remember(profiles, s, set);
}

pub struct Layout {
    pub parents: Vec<usize>,
    pub panels: Vec<Rect>,
    pub bounds: Rect,
    pitch: f32,
}

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
    pub fn new(displays: &[Display], calibration: &BTreeMap<String, Calibration>) -> Self {
        Self::arrange(displays, calibration, true)
    }
    pub fn legacy(displays: &[Display], calibration: &BTreeMap<String, Calibration>) -> Self {
        Self::arrange(displays, calibration, false)
    }
    fn arrange(
        displays: &[Display],
        calibration: &BTreeMap<String, Calibration>,
        use_saved_desk: bool,
    ) -> Self {
        let mut panels: Vec<_> = displays
            .iter()
            .map(|d| {
                let [w, h] = calibration.get(&d.id).and_then(|c| c.size).unwrap_or(d.mm);
                Rect::new(0., 0., w, h)
            })
            .collect();
        let mut placed = vec![false; displays.len()];
        let mut parents = vec![0; displays.len()];
        let mut saved = vec![false; displays.len()];
        for (k, d) in displays.iter().enumerate() {
            if let Some(position) = calibration.get(&d.id).and_then(|c| c.position) {
                [panels[k].x, panels[k].y] = position;
                saved[k] = use_saved_desk;
            }
        }
        let root = if saved[0] {
            0
        } else {
            saved.iter().position(|&s| s).unwrap_or(0)
        };
        placed[root] = true;
        parents[root] = root;
        let saved_desk = saved.iter().any(|&s| s);
        if root == 0
            && let Some(c) = calibration.get(&displays[0].id)
        {
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
            } else if saved_desk
                && calibration
                    .get(&displays[j].id)
                    .is_none_or(|c| c.offset == [0.; 2])
            {
                for _ in 0..displays.len() {
                    let Some(o) = (0..displays.len())
                        .filter(|&k| k != j && (saved[k] || placed[k]))
                        .filter(|&k| panels[k].overlaps(panels[j]))
                        .map(|k| panels[k])
                        .next()
                    else {
                        break;
                    };
                    if x_edge >= y_edge {
                        panels[j].x = if b.x >= a.x {
                            o.right()
                        } else {
                            o.x - panels[j].w
                        };
                    } else {
                        panels[j].y = if b.y >= a.y {
                            o.bottom()
                        } else {
                            o.y - panels[j].h
                        };
                    }
                }
            }
            placed[j] = true;
        }
        Self {
            parents,
            bounds: panels.iter().copied().reduce(Rect::union).unwrap(),
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
        self.freeze(displays, settings);
        let p = self.panels[index];
        settings
            .calibration
            .get_mut(&displays[index].id)
            .unwrap()
            .position = Some([p.x + delta[0], p.y + delta[1]]);
    }
    pub fn freeze(&self, displays: &[Display], settings: &mut Settings) {
        for (d, panel) in displays.iter().zip(&self.panels) {
            settings
                .calibration
                .entry(d.id.clone())
                .or_default()
                .position = Some([panel.x, panel.y]);
        }
        settings.pitch.get_or_insert(self.pitch);
    }
    pub fn pitch(&self, settings: &Settings) -> f32 {
        settings.pitch.unwrap_or(self.pitch)
    }
    pub fn resize_panel(
        &self,
        displays: &[Display],
        settings: &mut Settings,
        index: usize,
        size: [f32; 2],
    ) {
        self.freeze(displays, settings);
        settings
            .calibration
            .get_mut(&displays[index].id)
            .unwrap()
            .size = Some(size.map(|n| n.max(10.)));
    }
    pub fn overlapping(&self, i: usize) -> Vec<usize> {
        (0..self.panels.len())
            .filter(|&j| j != i && self.panels[i].overlaps(self.panels[j]))
            .collect()
    }
    pub fn media(&self, settings: &Settings, wallpaper: &Wallpaper) -> Rect {
        wallpaper.rect(self.pitch(settings))
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
            mirrors: Vec::new(),
            mirror_ids: Vec::new(),
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
        let image = initial.media(&settings, &settings.wallpaper);
        let mut expected = initial.panels.clone();
        for index in [1, 0, 2] {
            let layout = Layout::new(&displays, &settings.calibration);
            layout.move_panel(&displays, &mut settings, index, [34., -11.]);
            expected[index].x += 34.;
            expected[index].y -= 11.;
            let moved = Layout::new(&displays, &settings.calibration);
            assert_eq!(moved.panels, expected);
            assert_eq!(moved.media(&settings, &settings.wallpaper), image);
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
        assert_eq!(refreshed.media(&restored, &restored.wallpaper), image);
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
        assert_eq!(l.media(&s, &s.wallpaper).w, l.bounds.w * 2.);
        s.wallpaper.cover(l.pitch(&s), l.bounds);
        assert_eq!(l.media(&s, &s.wallpaper), l.bounds);
    }
    #[test]
    fn connecting_a_display_leaves_the_wallpaper_and_moved_panels_in_place() {
        let a = display("a", Rect::new(0., 0., 1920., 1080.), [480., 270.]);
        let b = display("b", Rect::new(1920., 0., 1920., 1080.), [480., 270.]);
        let mut s = Settings::new(Media {
            path: "clip".into(),
            width: 3840,
            height: 1080,
        });
        let single = Layout::new(std::slice::from_ref(&a), &s.calibration);
        s.wallpaper.cover(single.pitch(&s), single.bounds);
        single.move_panel(std::slice::from_ref(&a), &mut s, 0, [-60., 0.]);
        let image = single.media(&s, &s.wallpaper);
        let both = Layout::new(&[a.clone(), b], &s.calibration);
        assert_eq!(both.media(&s, &s.wallpaper), image);
        assert_eq!(both.panels[0], Rect::new(-60., 0., 480., 270.));
        assert_eq!(both.panels[1], Rect::new(420., 0., 480., 270.));
        assert!(both.overlapping(1).is_empty());
    }
    #[test]
    fn a_new_display_slides_clear_of_a_moved_panel() {
        let displays = [
            display("a", Rect::new(0., 0., 1920., 1080.), [480., 270.]),
            display("b", Rect::new(1920., 0., 1920., 1080.), [480., 270.]),
            display("c", Rect::new(3840., 0., 1920., 1080.), [480., 270.]),
        ];
        let at = |x: f32| Calibration {
            position: Some([x, 0.]),
            ..Default::default()
        };
        let calibration = BTreeMap::from([("a".into(), at(500.)), ("b".into(), at(0.))]);
        let l = Layout::new(&displays, &calibration);
        assert_eq!(l.panels[2], Rect::new(980., 0., 480., 270.));
        assert!((0..3).all(|i| l.overlapping(i).is_empty()));
    }
    #[test]
    fn unplugging_the_primary_display_leaves_the_rest_of_the_desk() {
        let a = display("a", Rect::new(0., 0., 1920., 1080.), [480., 270.]);
        let b = display("b", Rect::new(1920., -420., 1080., 1920.), [300., 530.]);
        let mut s = Settings::new(Media {
            path: "clip".into(),
            width: 3840,
            height: 2160,
        });
        let displays = [a, b.clone()];
        let both = Layout::new(&displays, &s.calibration);
        s.wallpaper.cover(both.pitch(&s), both.bounds);
        both.freeze(&displays, &mut s);
        let image = both.media(&s, &s.wallpaper);
        let mut alone = b;
        alone.px.x = 0.;
        alone.primary = true;
        let after = Layout::new(&[alone], &s.calibration);
        assert_eq!(after.panels[0], both.panels[1]);
        assert_eq!(after.media(&s, &s.wallpaper), image);
    }
    #[test]
    fn hotplug_keeps_surviving_crops_and_restores_known_framings() {
        let a = display("a", Rect::new(0., 0., 2560., 1440.), [600., 340.]);
        let b = display("b", Rect::new(2560., -240., 1080., 1920.), [270., 480.]);
        let c = display("c", Rect::new(3640., 0., 1920., 1080.), [500., 280.]);
        let trio = [a, b.clone(), c.clone()];
        let mut s = Settings::new(Media {
            path: "span.png".into(),
            width: 5760,
            height: 2160,
        });
        let layout = Layout::new(&trio, &s.calibration);
        layout.move_panel(&trio, &mut s, 1, [12., -18.]);
        let layout = Layout::new(&trio, &s.calibration);
        let panels = [layout.panels[1], layout.panels[2]];
        s.wallpaper.center = [650., 105.];
        s.wallpaper.scale = 1.25;
        s.wallpaper.stretch = 0.8;
        let trio_image = layout.media(&s, &s.wallpaper);
        let mut profiles = Vec::new();
        remember(&mut profiles, &mut s, &display_set(&trio));

        let mut pair = [b, c];
        pair[0].primary = true;
        for d in &mut pair {
            d.px.x -= 2560.;
            d.px.y += 240.;
        }
        follow_displays(&mut profiles, &mut s, &display_set(&pair));
        let layout = Layout::new(&pair, &s.calibration);
        assert_eq!(layout.panels, panels);
        assert_eq!(layout.media(&s, &s.wallpaper), trio_image);

        s.wallpaper.scale = 0.9;
        s.wallpaper.center = [810., 220.];
        let pair_image = layout.media(&s, &s.wallpaper);
        let d = display("d", Rect::new(-1920., 0., 1920., 1080.), [480., 270.]);
        let expanded = [pair[0].clone(), pair[1].clone(), d];
        follow_displays(&mut profiles, &mut s, &display_set(&expanded));
        let layout = Layout::new(&expanded, &s.calibration);
        assert_eq!(layout.panels[..2], panels);
        assert_eq!(layout.media(&s, &s.wallpaper), pair_image);

        follow_displays(&mut profiles, &mut s, &display_set(&trio));
        assert_eq!(
            Layout::new(&trio, &s.calibration).media(&s, &s.wallpaper),
            trio_image
        );
        follow_displays(&mut profiles, &mut s, &display_set(&pair));
        assert_eq!(
            Layout::new(&pair, &s.calibration).media(&s, &s.wallpaper),
            pair_image
        );
    }

    #[test]
    fn profiles_remember_a_framing_for_each_display_set() {
        let a = display("a", Rect::new(0., 0., 1920., 1080.), [480., 270.]);
        let b = display("b", Rect::new(1920., 0., 1920., 1080.), [480., 270.]);
        let mut s = Settings::new(Media {
            path: "city.mp4".into(),
            width: 3840,
            height: 1080,
        });
        s.wallpaper.scale = 0.5;
        let mut profiles = Vec::new();
        let alone = display_set(std::slice::from_ref(&a));
        remember(&mut profiles, &mut s, &alone);
        assert_eq!(s.profile.as_deref(), Some("Default"));
        let docked = [a.clone(), b];
        let layout = Layout::new(&docked, &s.calibration);
        profiles[0].apply(&mut s, &layout, &display_set(&docked));
        assert_eq!(
            layout.media(&s, &s.wallpaper),
            Rect::new(0., 0., 960., 270.)
        );
        s.wallpaper.center = [500., 100.];
        remember(&mut profiles, &mut s, &display_set(&docked));
        let single = Layout::new(std::slice::from_ref(&a), &s.calibration);
        profiles[0].apply(&mut s, &single, &alone);
        assert_eq!((s.wallpaper.scale, s.wallpaper.center), (0.5, [0., 0.]));
        profiles[0].apply(&mut s, &layout, &display_set(&docked));
        assert_eq!(s.wallpaper.center, [500., 100.]);
        s.wallpaper.media.path = "forest.png".into();
        remember(&mut profiles, &mut s, &alone);
        assert_eq!(profiles[0].framings.len(), 1);
        assert_eq!(unique_name(&profiles, "Default"), "Default 2");
    }
    #[test]
    fn default_profile_comes_first_and_is_reused() {
        let mut s = Settings::new(Media {
            path: "night.png".into(),
            width: 100,
            height: 100,
        });
        let mut profiles = vec![Profile::new("Night", &s, "a")];
        remember(&mut profiles, &mut s, "a");
        let names = |p: &[Profile]| p.iter().map(|p| p.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(&profiles), ["Default", "Night"]);
        assert_eq!(s.profile.as_deref(), Some(DEFAULT_PROFILE));
        s.profile = Some("Gone".into());
        remember(&mut profiles, &mut s, "a");
        assert_eq!(names(&profiles), ["Default", "Night"]);
        let mut moved = vec![profiles[1].clone(), profiles[0].clone()];
        assert!(ensure_default(&mut moved, &s, "a"));
        assert_eq!(names(&moved), ["Default", "Night"]);
    }
    #[test]
    fn following_monitors_preserves_newer_layout_edits_and_distinguishes_duplicate_mode() {
        let mut a = display("a", Rect::new(0., 0., 1920., 1080.), [480., 270.]);
        let mut s = Settings::new(Media {
            path: "old.png".into(),
            width: 1920,
            height: 1080,
        });
        let mut profiles = Vec::new();
        remember(&mut profiles, &mut s, "a");
        s.wallpaper.media.path = "new.png".into();
        s.wallpaper.center = [10., 20.];
        follow_displays(&mut profiles, &mut s, "a");
        assert_eq!(s.wallpaper.media.path.to_str(), Some("new.png"));
        assert_eq!(s.wallpaper.center, [10., 20.]);
        assert_eq!(profiles[0].media.path, s.wallpaper.media.path);
        a.mirror_ids.push("b".into());
        let duplicated = display_set(std::slice::from_ref(&a));
        assert_ne!(duplicated, "a");
        assert_ne!(duplicated, "a|b");
        follow_displays(&mut profiles, &mut s, &duplicated);
        assert_eq!(s.wallpaper.center, [10., 20.]);
        s.wallpaper.center = [30., 40.];
        follow_displays(&mut profiles, &mut s, "a");
        assert_eq!(s.wallpaper.center, [10., 20.]);
        let old = br#"{"name":"Old","media":{"path":"wall","width":100,"height":100},"framings":{"a":{"scale":2,"center":[0,0]}}}"#;
        let p: Profile = serde_json::from_slice(old).unwrap();
        assert!(!p.multi && p.screens.is_empty());
        assert_eq!(p.framings["a"].stretch, 1.);
        assert!(
            serde_json::from_slice::<Vec<Profile>>(&serde_json::to_vec(&profiles).unwrap())
                .unwrap()
                == profiles
        );
    }
    #[test]
    fn stretching_changes_width_only_and_fitting_undoes_it() {
        let mut w = Wallpaper::new(Media {
            path: "wide.png".into(),
            width: 200,
            height: 100,
        });
        w.stretch = 2.;
        let r = w.rect(1.);
        assert_eq!((r.w, r.h), (400., 100.));
        w.cover(1., Rect::new(0., 0., 100., 100.));
        assert_eq!(w.stretch, 1.);
        assert_eq!(w.rect(1.), Rect::new(-50., 0., 200., 100.));
    }
    #[test]
    fn a_new_primary_display_joins_the_saved_desk() {
        let displays = [
            display("b", Rect::new(0., 0., 1920., 1080.), [480., 270.]),
            display("a", Rect::new(1920., 0., 1920., 1080.), [480., 270.]),
        ];
        let calibration = BTreeMap::from([(
            "a".into(),
            Calibration {
                position: Some([0., 0.]),
                ..Default::default()
            },
        )]);
        let l = Layout::new(&displays, &calibration);
        assert_eq!(
            l.panels,
            vec![
                Rect::new(-480., 0., 480., 270.),
                Rect::new(0., 0., 480., 270.)
            ]
        );
        let old = Layout::legacy(&displays, &calibration);
        assert_eq!(old.panels[0], old.panels[1]);
    }
    #[test]
    fn a_new_display_avoids_saved_panels_placed_after_it() {
        let displays = [
            display("a", Rect::new(0., 0., 1920., 1080.), [480., 270.]),
            display("b", Rect::new(1920., 0., 1920., 1080.), [480., 270.]),
            display("c", Rect::new(3840., 0., 1920., 1080.), [480., 270.]),
        ];
        let at = |x: f32| Calibration {
            position: Some([x, 0.]),
            ..Default::default()
        };
        let calibration = BTreeMap::from([("a".into(), at(0.)), ("c".into(), at(480.))]);
        let l = Layout::new(&displays, &calibration);
        assert_eq!(l.panels[1], Rect::new(960., 0., 480., 270.));
        assert!((0..3).all(|i| l.overlapping(i).is_empty()));
    }

    #[test]
    fn two_new_displays_choose_distinct_positions_on_a_saved_desk() {
        let displays: Vec<_> = ["a", "b", "c", "d"]
            .into_iter()
            .enumerate()
            .map(|(i, id)| {
                display(
                    id,
                    Rect::new(1920. * i as f32, 0., 1920., 1080.),
                    [480., 270.],
                )
            })
            .collect();
        let at_origin = Calibration {
            position: Some([0., 0.]),
            ..Default::default()
        };
        let calibration =
            BTreeMap::from([("a".into(), at_origin.clone()), ("c".into(), at_origin)]);
        let l = Layout::new(&displays, &calibration);
        assert_eq!(l.panels[0], l.panels[2]);
        assert!(l.overlapping(1).is_empty() && l.overlapping(3).is_empty());
    }
}
