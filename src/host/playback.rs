use super::*;

fn covers(window: Rect, area: Rect) -> bool {
    window.x <= area.x
        && window.y <= area.y
        && window.right() >= area.right()
        && window.bottom() >= area.bottom()
}

pub(super) struct PlaybackPolicy {
    locked: bool,
    foreground: Option<Rect>,
    fullscreen: PlaybackAction,
    battery: PlaybackAction,
}

impl PlaybackPolicy {
    pub(super) fn detect(displays: &[Display], prefs: &Preferences) -> Self {
        let foreground =
            desktop::foreground_bounds().filter(|r| displays.iter().any(|d| covers(*r, d.px)));
        Self {
            locked: desktop::locked(),
            foreground,
            fullscreen: prefs.fullscreen,
            battery: if desktop::battery_saver() {
                prefs.battery_saver
            } else {
                PlaybackAction::Continue
            },
        }
    }
    fn stopped(&self) -> Option<&'static str> {
        if self.battery == PlaybackAction::Stop {
            Some("Stopped · Energy Saver")
        } else if self.foreground.is_some() && self.fullscreen == PlaybackAction::Stop {
            Some("Stopped · fullscreen app")
        } else {
            None
        }
    }
    fn paused(&self, area: Rect) -> Option<&'static str> {
        if self.locked {
            Some("Paused · desktop locked")
        } else if self.battery == PlaybackAction::Pause {
            Some("Paused · Energy Saver")
        } else if self
            .foreground
            .is_some_and(|r| self.fullscreen == PlaybackAction::Pause || covers(r, area))
        {
            Some("Paused · fullscreen app")
        } else {
            None
        }
    }
}

#[derive(Default)]
pub(super) struct Players {
    visible: Vec<Player>,
    loading: Vec<Player>,
    desired: Vec<PathBuf>,
    failures: BTreeMap<PathBuf, (Instant, String)>,
    fade: Option<Instant>,
}

impl Players {
    pub(super) fn discard_replacements(&mut self) {
        self.cancel_fade();
        self.loading.clear();
    }

    fn cancel_fade(&mut self) {
        if self.fade.take().is_some() {
            for p in &self.loading {
                let _ = p.surface.opacity(0);
            }
            for p in &self.visible {
                let _ = p.surface.opacity(255);
            }
        }
    }

    pub(super) fn reconcile(
        &mut self,
        settings: &Settings,
        displays: &[Display],
        policy: &PlaybackPolicy,
        mapping_changed: bool,
    ) -> Result<PlaybackReport> {
        let mut report = PlaybackReport {
            host: std::process::id(),
            layout_modified: None,
            loading: false,
            displays: BTreeMap::new(),
        };
        let plan = shader::plan(settings, displays);
        let desired: Vec<_> = plan
            .iter()
            .map(|part| player_pipe(report.host, settings.multi, part, displays))
            .collect();
        if let Some(reason) = policy.stopped() {
            *self = Self::default();
            for part in plan {
                report.record(&part, displays, reason);
            }
            return Ok(report);
        }
        if desired != self.desired {
            self.cancel_fade();
            self.loading.retain(|p| desired.contains(&p.pipe));
            self.failures.retain(|pipe, _| desired.contains(pipe));
            self.desired = desired;
        }

        let layout = Layout::new(displays, &settings.calibration);
        for (part, pipe) in plan.iter().zip(&self.desired) {
            let player = self
                .visible
                .iter_mut()
                .chain(&mut self.loading)
                .find(|p| &p.pipe == pipe);
            if let Some(p) = player {
                p.part.clone_from(part);
                if mapping_changed {
                    p.pending_mapping = Some(shader::parameters(&layout, settings, displays, part));
                }
            } else if self
                .failures
                .get(pipe)
                .is_none_or(|(when, _)| when.elapsed() >= Duration::from_secs(4))
            {
                match Player::open(part, settings, displays) {
                    Ok(p) => {
                        self.failures.remove(pipe);
                        self.loading.push(p);
                    }
                    Err(e) => {
                        self.failures
                            .insert(pipe.clone(), (Instant::now(), format!("{e:#}")));
                    }
                }
            }
        }
        let waiting_for_replacement = self.visible.iter().any(|p| !self.desired.contains(&p.pipe))
            && !self.failures.is_empty();
        for (group, waiting) in [
            (&mut self.visible, false),
            (&mut self.loading, waiting_for_replacement),
        ] {
            group.retain_mut(|p| {
                let paused = policy.paused(p.desktop).is_some() || (waiting && p.still.is_some());
                let result = p.check().and_then(|_| p.update(paused));
                if let Err(e) = result {
                    if self.desired.contains(&p.pipe) {
                        self.failures
                            .insert(p.pipe.clone(), (Instant::now(), format!("{e:#}")));
                    }
                    false
                } else {
                    true
                }
            });
        }
        for (part, pipe) in plan.iter().zip(&self.desired) {
            let status = if let Some((_, msg)) = self.failures.get(pipe) {
                msg.as_str()
            } else if let Some(p) = self.visible.iter().find(|p| &p.pipe == pipe) {
                if p.still == Some(true) {
                    "Still image"
                } else if p.covered {
                    policy.paused(p.desktop).unwrap_or("Paused")
                } else {
                    "Playing"
                }
            } else if self
                .loading
                .iter()
                .any(|p| &p.pipe == pipe && p.still.is_some())
                && !self.failures.is_empty()
            {
                "Waiting for another display"
            } else {
                "Loading wallpaper…"
            };
            report.record(part, displays, status);
        }
        Ok(report)
    }

    pub(super) fn advance(&mut self) -> Result<()> {
        if self.loading.is_empty() && self.visible.iter().all(|p| self.desired.contains(&p.pipe)) {
            self.fade = None;
            return Ok(());
        }
        self.loading.retain_mut(|p| match p.check() {
            Ok(()) => true,
            Err(e) => {
                self.failures
                    .insert(p.pipe.clone(), (Instant::now(), format!("{e:#}")));
                false
            }
        });
        let replacing = self.visible.iter().any(|p| !self.desired.contains(&p.pipe));
        let missing_replacement = replacing
            && self.desired.iter().any(|pipe| {
                !self
                    .visible
                    .iter()
                    .chain(&self.loading)
                    .any(|p| &p.pipe == pipe)
            });
        if missing_replacement || self.loading.iter().any(|p| p.still.is_none()) {
            self.cancel_fade();
            return Ok(());
        }
        if self.fade.is_none() {
            for p in &self.loading {
                p.surface.raise(p.desktop)?;
            }
        }
        let start = self.fade.get_or_insert_with(Instant::now);
        let t = (start.elapsed().as_secs_f32() / 0.32).min(1.);
        let alpha = (255. * t * t * (3. - 2. * t)).round() as u8;
        for p in &self.loading {
            p.surface.opacity(alpha)?;
        }
        for p in &self.visible {
            let covered_by_new = self
                .loading
                .iter()
                .any(|new| covers(new.desktop, p.desktop))
                || p.part
                    .displays
                    .iter()
                    .all(|i| self.loading.iter().any(|new| new.part.displays.contains(i)));
            if !self.desired.contains(&p.pipe) && !covered_by_new {
                p.surface.opacity(255 - alpha)?;
            }
        }
        if t >= 1. {
            self.visible.retain(|p| self.desired.contains(&p.pipe));
            self.visible.append(&mut self.loading);
            self.fade = None;
        }
        Ok(())
    }
    pub(super) fn is_loading(&self) -> bool {
        self.failures.is_empty()
            && (!self.loading.is_empty()
                || self.visible.iter().any(|p| !self.desired.contains(&p.pipe)))
    }

    pub(super) fn interval(&self) -> u128 {
        if self.fade.is_some() {
            16
        } else if self.loading.iter().any(|p| p.still.is_none()) {
            50
        } else {
            1000
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_switch_waits_for_the_requested_layout_and_its_fade() {
        let requested = SystemTime::now();
        let mut report = PlaybackReport {
            host: 1,
            layout_modified: Some(requested - Duration::from_secs(1)),
            loading: false,
            displays: BTreeMap::new(),
        };
        assert!(!report.switch_finished(requested));
        report.layout_modified = Some(requested);
        report.loading = true;
        assert!(!report.switch_finished(requested));
        report.loading = false;
        assert!(report.switch_finished(requested));
    }

    #[test]
    fn fullscreen_rules_follow_the_visible_area_and_combine_with_energy_saver() {
        let left = Rect::new(-1920., 0., 1920., 1080.);
        let right = Rect::new(0., 0., 2560., 1440.);
        let mut policy = PlaybackPolicy {
            locked: false,
            foreground: Some(left),
            fullscreen: PlaybackAction::Continue,
            battery: PlaybackAction::Continue,
        };
        assert!(policy.paused(left).is_some());
        assert_eq!(policy.paused(right), None);
        assert_eq!(policy.paused(left.union(right)), None);
        policy.fullscreen = PlaybackAction::Pause;
        assert!(policy.paused(right).is_some());
        policy.battery = PlaybackAction::Stop;
        assert_eq!(policy.stopped(), Some("Stopped · Energy Saver"));
        policy.battery = PlaybackAction::Pause;
        policy.fullscreen = PlaybackAction::Stop;
        assert_eq!(policy.stopped(), Some("Stopped · fullscreen app"));
        policy.foreground = None;
        assert_eq!(policy.stopped(), None);
        assert_eq!(policy.paused(right), Some("Paused · Energy Saver"));
        policy.battery = PlaybackAction::Continue;
        policy.locked = true;
        assert_eq!(policy.paused(right), Some("Paused · desktop locked"));
        policy.locked = false;
        assert_eq!(policy.paused(left), None);
    }
}
