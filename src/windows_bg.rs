use crate::{
    desktop::wide,
    layout::{Display, Layout, Rect, Settings},
    render::{self, Crop, SourceImage},
    storage,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    ffi::c_void,
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    ptr::{null, null_mut},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
};
use windows_sys::{
    Win32::{
        Foundation::{RECT, RPC_E_CHANGED_MODE, S_FALSE, S_OK},
        System::Com::{
            CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize,
        },
        UI::Shell::{DWPOS_FILL, DesktopWallpaper as CLSID_DESKTOP_WALLPAPER},
    },
    core::{GUID, HRESULT, PCWSTR, PWSTR},
};

const IID_IDESKTOP_WALLPAPER: GUID = GUID::from_u128(0xb92b56a9_8b55_4e14_9a89_0199bbb6f93b);

#[repr(C)]
struct DesktopWallpaperVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: unsafe extern "system" fn(*mut c_void) -> u32,
    release: unsafe extern "system" fn(*mut c_void) -> u32,
    set_wallpaper: unsafe extern "system" fn(*mut c_void, PCWSTR, PCWSTR) -> HRESULT,
    get_wallpaper: unsafe extern "system" fn(*mut c_void, PCWSTR, *mut PWSTR) -> HRESULT,
    get_monitor_device_path_at: unsafe extern "system" fn(*mut c_void, u32, *mut PWSTR) -> HRESULT,
    get_monitor_device_path_count: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    get_monitor_rect: unsafe extern "system" fn(*mut c_void, PCWSTR, *mut RECT) -> HRESULT,
    set_background_color: unsafe extern "system" fn(*mut c_void, u32) -> HRESULT,
    get_background_color: unsafe extern "system" fn(*mut c_void, *mut u32) -> HRESULT,
    set_position: unsafe extern "system" fn(*mut c_void, i32) -> HRESULT,
    get_position: unsafe extern "system" fn(*mut c_void, *mut i32) -> HRESULT,
}

#[repr(C)]
struct DesktopWallpaperRaw {
    vtbl: *const DesktopWallpaperVtbl,
}

struct ComGuard(bool);
impl ComGuard {
    fn init() -> Result<Self> {
        let hr = unsafe { CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) };
        if hr == S_OK || hr == S_FALSE {
            Ok(Self(true))
        } else if hr == RPC_E_CHANGED_MODE {
            Ok(Self(false))
        } else {
            bail!("CoInitializeEx failed (0x{:08x})", hr as u32);
        }
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

struct CoTaskStr(PWSTR);
impl CoTaskStr {
    fn to_string_lossy(&self) -> String {
        if self.0.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        unsafe {
            while *self.0.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(self.0, len))
        }
    }
}
impl Drop for CoTaskStr {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(self.0.cast()) };
        }
    }
}

struct DesktopWallpaperApi {
    raw: *mut DesktopWallpaperRaw,
    _com: ComGuard,
}

impl DesktopWallpaperApi {
    fn open() -> Result<Self> {
        let com = ComGuard::init()?;
        let mut raw: *mut c_void = null_mut();
        let hr = unsafe {
            CoCreateInstance(
                &CLSID_DESKTOP_WALLPAPER,
                null_mut(),
                CLSCTX_ALL,
                &IID_IDESKTOP_WALLPAPER,
                &mut raw,
            )
        };
        if hr < 0 || raw.is_null() {
            bail!("Cannot open IDesktopWallpaper (0x{:08x})", hr as u32);
        }
        Ok(Self {
            raw: raw.cast(),
            _com: com,
        })
    }

    fn vtbl(&self) -> &DesktopWallpaperVtbl {
        unsafe { &*(*self.raw).vtbl }
    }

    fn monitors(&self) -> Result<Vec<(String, Option<Rect>)>> {
        let mut count = 0u32;
        let hr = unsafe { (self.vtbl().get_monitor_device_path_count)(self.raw.cast(), &mut count) };
        if hr < 0 {
            bail!("GetMonitorDevicePathCount failed (0x{:08x})", hr as u32);
        }
        let mut result = Vec::with_capacity(count as usize);
        for index in 0..count {
            let mut ptr: PWSTR = null_mut();
            let hr = unsafe {
                (self.vtbl().get_monitor_device_path_at)(self.raw.cast(), index, &mut ptr)
            };
            if hr < 0 || ptr.is_null() {
                continue;
            }
            let id = CoTaskStr(ptr).to_string_lossy();
            let wide_id = wide(&id);
            let mut rect: RECT = unsafe { std::mem::zeroed() };
            let rect_hr = unsafe {
                (self.vtbl().get_monitor_rect)(self.raw.cast(), wide_id.as_ptr(), &mut rect)
            };
            let bounds = (rect_hr >= 0).then(|| {
                Rect::new(
                    rect.left as f32,
                    rect.top as f32,
                    (rect.right - rect.left) as f32,
                    (rect.bottom - rect.top) as f32,
                )
            });
            result.push((id, bounds));
        }
        Ok(result)
    }

    fn get_wallpaper(&self, monitor_id: &str) -> Option<PathBuf> {
        let wide_id = wide(monitor_id);
        let mut ptr: PWSTR = null_mut();
        let hr = unsafe {
            (self.vtbl().get_wallpaper)(self.raw.cast(), wide_id.as_ptr(), &mut ptr)
        };
        if hr < 0 || ptr.is_null() {
            return None;
        }
        let path = CoTaskStr(ptr).to_string_lossy();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }

    fn set_wallpaper(&self, monitor_id: &str, path: &Path) -> Result<()> {
        let wide_id = wide(monitor_id);
        let wide_path: Vec<u16> = path
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain([0])
            .collect();
        let hr = unsafe {
            (self.vtbl().set_wallpaper)(self.raw.cast(), wide_id.as_ptr(), wide_path.as_ptr())
        };
        if hr < 0 {
            bail!("SetWallpaper failed (0x{:08x})", hr as u32);
        }
        Ok(())
    }

    fn get_position(&self) -> Option<i32> {
        let mut pos = 0i32;
        let hr = unsafe { (self.vtbl().get_position)(self.raw.cast(), &mut pos) };
        (hr >= 0).then_some(pos)
    }

    fn set_position(&self, pos: i32) -> Result<()> {
        let hr = unsafe { (self.vtbl().set_position)(self.raw.cast(), pos) };
        if hr < 0 {
            bail!("SetPosition failed (0x{:08x})", hr as u32);
        }
        Ok(())
    }
}

impl Drop for DesktopWallpaperApi {
    fn drop(&mut self) {
        unsafe {
            (self.vtbl().release)(self.raw.cast());
        }
    }
}

#[derive(Default, Serialize, Deserialize, PartialEq, Eq)]
struct StoredMonitor {
    fingerprint: u64,
    path: PathBuf,
}

#[derive(Default, Serialize, Deserialize)]
struct SyncCache {
    monitors: BTreeMap<String, StoredMonitor>,
}

fn bg_dir() -> Result<PathBuf> {
    let dir = storage::home()?.join("windows-bg");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn load_cache(dir: &Path) -> SyncCache {
    fs::read(dir.join("state.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_cache(dir: &Path, cache: &SyncCache) -> Result<()> {
    let dest = dir.join("state.json");
    let data = serde_json::to_vec_pretty(cache)?;
    let tmp = dir.join("state.json.tmp");
    fs::write(&tmp, data)?;
    let _ = fs::rename(&tmp, &dest).or_else(|_| {
        let _ = fs::remove_file(&dest);
        fs::rename(&tmp, &dest)
    });
    Ok(())
}

fn slot_paths(dir: &Path, monitor_id: &str) -> (PathBuf, PathBuf) {
    let mut hash = DefaultHasher::new();
    monitor_id.to_ascii_lowercase().hash(&mut hash);
    let key = format!("{:016x}", hash.finish());
    (
        dir.join(format!("bg-{key}-a.bmp")),
        dir.join(format!("bg-{key}-b.bmp")),
    )
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

fn choose_slot(dir: &Path, monitor_id: &str, current: Option<&Path>) -> (PathBuf, PathBuf) {
    let (a, b) = slot_paths(dir, monitor_id);
    if current.is_some_and(|cur| same_path(cur, &a)) {
        (b, a)
    } else {
        (a, b)
    }
}

fn sweep_unused(dir: &Path, keep: &HashSet<String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if name.eq_ignore_ascii_case("state.json") {
            continue;
        }
        let lower = path.to_string_lossy().to_ascii_lowercase();
        if !keep.contains(&lower) {
            let _ = fs::remove_file(&path);
        }
    }
}

fn match_monitors(displays: &[Display], monitors: &[(String, Option<Rect>)]) -> Vec<(String, usize)> {
    let mut matched = Vec::new();
    for (mon_id, mon_rect) in monitors {
        // Skip disconnected monitors that Windows still lists without a desktop rectangle.
        let Some(rect) = mon_rect else {
            continue;
        };
        let index = displays
            .iter()
            .position(|d| {
                d.id.eq_ignore_ascii_case(mon_id)
                    || d.mirror_ids.iter().any(|m| m.eq_ignore_ascii_case(mon_id))
            })
            .or_else(|| {
                displays.iter().position(|d| {
                    (d.px.x - rect.x).abs() <= 1.0
                        && (d.px.y - rect.y).abs() <= 1.0
                        && (d.px.w - rect.w).abs() <= 1.0
                        && (d.px.h - rect.h).abs() <= 1.0
                })
            });
        if let Some(i) = index {
            matched.push((mon_id.clone(), i));
        }
    }
    matched
}

fn display_fingerprint(
    display: &Display,
    settings: &Settings,
    crop: Option<Crop>,
) -> u64 {
    let mut hash = DefaultHasher::new();
    (display.px.w.round() as u32).hash(&mut hash);
    (display.px.h.round() as u32).hash(&mut hash);
    let shown = settings.shown(&display.id);
    if shown.is_empty() {
        0u8.hash(&mut hash);
    } else {
        1u8.hash(&mut hash);
        shown.media.path.hash(&mut hash);
        if let Ok(meta) = fs::metadata(&shown.media.path) {
            meta.len().hash(&mut hash);
            if let Ok(modified) = meta.modified() {
                modified.hash(&mut hash);
            }
        }
        if let Some(crop) = crop {
            [crop.u, crop.v, crop.w, crop.h]
                .map(|n| (n as f64 * 1_000_000.0).round() as i64)
                .hash(&mut hash);
        }
        (settings.upscaling as u8).hash(&mut hash);
    }
    hash.finish()
}

fn job_fingerprint(settings: &Settings, displays: &[Display]) -> u64 {
    let layout = Layout::new(displays, &settings.calibration);
    let mut hash = DefaultHasher::new();
    displays.len().hash(&mut hash);
    for (i, display) in displays.iter().enumerate() {
        display.id.hash(&mut hash);
        display.mirror_ids.hash(&mut hash);
        let crop = (!settings.shown(&display.id).is_empty())
            .then(|| render::display_crop(&layout, settings, displays, i));
        display_fingerprint(display, settings, crop).hash(&mut hash);
    }
    hash.finish()
}

fn apply_sync(settings: &Settings, displays: &[Display]) -> Result<bool> {
    let dir = bg_dir()?;
    let api = DesktopWallpaperApi::open()?;
    let monitors = api.monitors()?;
    let matched = match_monitors(displays, &monitors);
    if matched.is_empty() {
        return Ok(false);
    }
    let layout = Layout::new(displays, &settings.calibration);
    let mut cache = load_cache(&dir);
    let position_ok = api.get_position() == Some(DWPOS_FILL);
    let mut need_position = !position_ok;
    let mut keep = HashSet::<String>::new();
    let mut decoded = HashMap::<PathBuf, SourceImage>::new();
    let mut updated_any = false;
    let mut next_cache = SyncCache::default();

    struct PendingUpdate<'a> {
        monitor_id: String,
        display_index: usize,
        fingerprint: u64,
        crop: Option<Crop>,
        current: Option<PathBuf>,
        media: Option<&'a Path>,
    }

    let mut pending = Vec::new();
    for (monitor_id, index) in matched {
        let display = &displays[index];
        let shown = settings.shown(&display.id);
        let crop = (!shown.is_empty()).then(|| render::display_crop(&layout, settings, displays, index));
        let fingerprint = display_fingerprint(display, settings, crop);
        let current = api.get_wallpaper(&monitor_id);

        if let Some(stored) = cache.monitors.get(&monitor_id)
            && stored.fingerprint == fingerprint
            && stored.path.is_file()
            && current
                .as_deref()
                .is_some_and(|cur| same_path(cur, &stored.path))
        {
            keep.insert(stored.path.to_string_lossy().to_ascii_lowercase());
            next_cache.monitors.insert(
                monitor_id,
                StoredMonitor {
                    fingerprint,
                    path: stored.path.clone(),
                },
            );
            continue;
        }

        let media = (!shown.is_empty()).then_some(shown.media.path.as_path());
        if let Some(path) = media
            && !path.is_file()
        {
            if let Some(stored) = cache.monitors.remove(&monitor_id)
                && stored.path.is_file()
            {
                keep.insert(stored.path.to_string_lossy().to_ascii_lowercase());
                next_cache.monitors.insert(monitor_id, stored);
            }
            continue;
        }
        pending.push(PendingUpdate {
            monitor_id,
            display_index: index,
            fingerprint,
            crop,
            current,
            media,
        });
    }

    if !pending.is_empty() && need_position {
        api.set_position(DWPOS_FILL)?;
        need_position = false;
    }

    for item in pending {
        let display = &displays[item.display_index];
        let bmp = match (item.media, item.crop) {
            (Some(media_path), Some(crop)) => {
                if !decoded.contains_key(media_path) {
                    let source = render::decode_source(media_path)
                        .with_context(|| format!("Cannot decode {}", media_path.display()))?;
                    decoded.insert(media_path.to_path_buf(), source);
                }
                let src = &decoded[media_path];
                render::render_display_bmp(
                    src,
                    crop,
                    settings.upscaling,
                    display.px.w.round().max(1.0) as u32,
                    display.px.h.round().max(1.0) as u32,
                )?
            }
            _ => render::black_bmp(),
        };
        let (target, other) = choose_slot(&dir, &item.monitor_id, item.current.as_deref());
        fs::write(&target, &bmp)
            .with_context(|| format!("Cannot write {}", target.display()))?;
        api.set_wallpaper(&item.monitor_id, &target)?;
        let _ = fs::remove_file(&other);
        keep.insert(target.to_string_lossy().to_ascii_lowercase());
        next_cache.monitors.insert(
            item.monitor_id,
            StoredMonitor {
                fingerprint: item.fingerprint,
                path: target,
            },
        );
        updated_any = true;
    }

    if need_position && !next_cache.monitors.is_empty() {
        api.set_position(DWPOS_FILL)?;
        updated_any = true;
    }
    sweep_unused(&dir, &keep);
    let _ = save_cache(&dir, &next_cache);
    Ok(updated_any)
}

#[derive(Default)]
struct WorkerState {
    pending: Option<(Settings, Vec<Display>)>,
    shutdown: bool,
}

pub struct Syncer {
    state: Arc<(Mutex<WorkerState>, Condvar)>,
    completed: Arc<AtomicBool>,
    retry: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    last_submitted: Option<u64>,
}

impl Default for Syncer {
    fn default() -> Self {
        Self {
            state: Arc::new((Mutex::new(WorkerState::default()), Condvar::new())),
            completed: Arc::new(AtomicBool::new(false)),
            retry: Arc::new(AtomicBool::new(false)),
            worker: None,
            last_submitted: None,
        }
    }
}

impl Syncer {
    fn ensure_worker(&mut self) {
        if self.worker.is_some() {
            return;
        }
        let state = Arc::clone(&self.state);
        let completed = Arc::clone(&self.completed);
        let retry = Arc::clone(&self.retry);
        self.worker = Some(thread::spawn(move || {
            loop {
                let job = {
                    let (lock, cvar) = &*state;
                    let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
                    while guard.pending.is_none() && !guard.shutdown {
                        guard = cvar.wait(guard).unwrap_or_else(|e| e.into_inner());
                    }
                    if guard.shutdown {
                        break;
                    }
                    guard.pending.take()
                };
                if let Some((settings, displays)) = job {
                    match apply_sync(&settings, &displays) {
                        Ok(true) => {
                            retry.store(false, Ordering::Release);
                            completed.store(true, Ordering::Release);
                        }
                        Ok(false) => {
                            retry.store(false, Ordering::Release);
                        }
                        Err(_) => {
                            retry.store(true, Ordering::Release);
                        }
                    }
                }
            }
        }));
    }

    pub fn sync(&mut self, settings: &Settings, displays: &[Display]) {
        if displays.is_empty() {
            return;
        }
        if self.retry.swap(false, Ordering::AcqRel) {
            self.last_submitted = None;
        }
        let fingerprint = job_fingerprint(settings, displays);
        if self.last_submitted == Some(fingerprint) {
            return;
        }
        self.ensure_worker();
        self.last_submitted = Some(fingerprint);
        let (lock, cvar) = &*self.state;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        guard.pending = Some((settings.clone(), displays.to_vec()));
        cvar.notify_one();
    }

    pub fn disable(&mut self) {
        self.last_submitted = None;
        self.retry.store(false, Ordering::Release);
        let (lock, _) = &*self.state;
        let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        guard.pending = None;
    }

    pub fn take_completed(&self) -> bool {
        self.completed.swap(false, Ordering::AcqRel)
    }
}

impl Drop for Syncer {
    fn drop(&mut self) {
        {
            let (lock, cvar) = &*self.state;
            let mut guard = lock.lock().unwrap_or_else(|e| e.into_inner());
            guard.shutdown = true;
            guard.pending = None;
            cvar.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::Media;

    fn display(id: &str, px: Rect, mirror_ids: Vec<String>) -> Display {
        Display {
            id: id.into(),
            name: id.into(),
            primary: id == "a",
            px,
            mm: [480., 270.],
            estimated: false,
            mirrors: Vec::new(),
            mirror_ids,
        }
    }

    #[test]
    fn matches_monitors_by_device_id_mirror_and_bounds_and_skips_disconnected() {
        let displays = [
            display(r"\\?\DISPLAY#DEL1#1", Rect::new(0., 0., 1920., 1080.), vec![r"\\?\DISPLAY#MIRROR#2".into()]),
            display(r"\\?\DISPLAY#LG2#3", Rect::new(1920., 0., 2560., 1440.), Vec::new()),
        ];
        let monitors = vec![
            (r"\\?\display#del1#1".into(), Some(Rect::new(0., 0., 1920., 1080.))),
            (r"\\?\DISPLAY#MIRROR#2".into(), Some(Rect::new(0., 0., 1920., 1080.))),
            (r"\\?\DISPLAY#OTHER#9".into(), Some(Rect::new(1920., 0., 2560., 1440.))),
            (r"\\?\DISPLAY#OFFLINE#4".into(), None),
        ];
        let matched = match_monitors(&displays, &monitors);
        assert_eq!(
            matched,
            vec![
                (r"\\?\display#del1#1".into(), 0),
                (r"\\?\DISPLAY#MIRROR#2".into(), 0),
                (r"\\?\DISPLAY#OTHER#9".into(), 1),
            ]
        );
    }

    #[test]
    fn slot_alternates_and_sweep_leaves_only_active_files() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = choose_slot(dir.path(), "mon-1", None);
        assert_ne!(first, second);
        fs::write(&first, b"one").unwrap();
        let (next, old) = choose_slot(dir.path(), "mon-1", Some(&first));
        assert_eq!(next, second);
        assert_eq!(old, first);
        fs::write(&next, b"two").unwrap();
        let stale = dir.path().join("bg-deadbeef00000000-a.bmp");
        fs::write(&stale, b"stale").unwrap();
        fs::write(dir.path().join("state.json"), b"{}").unwrap();

        let keep = HashSet::from([next.to_string_lossy().to_ascii_lowercase()]);
        sweep_unused(dir.path(), &keep);

        assert!(next.is_file());
        assert!(dir.path().join("state.json").is_file());
        assert!(!first.is_file());
        assert!(!stale.is_file());
    }

    #[test]
    fn job_fingerprint_ignores_paused_state_and_tracks_crop_changes() {
        let displays = [display("a", Rect::new(0., 0., 1920., 1080.), Vec::new())];
        let mut s = Settings::new(Media {
            path: "clip.mp4".into(),
            width: 1920,
            height: 1080,
        });
        let playing = job_fingerprint(&s, &displays);
        s.wallpaper.paused = true;
        assert_eq!(job_fingerprint(&s, &displays), playing);
        s.wallpaper.scale = 1.5;
        assert_ne!(job_fingerprint(&s, &displays), playing);
    }
}
