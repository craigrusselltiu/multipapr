#![windows_subsystem = "windows"]
mod desktop;
mod displays;
mod editor;
mod gallery;
mod host;
mod http;
mod layout;
mod process;
mod render;
mod shader;
mod storage;
mod tray;
mod update;
mod windows_bg;

use anyhow::{Context, Result};
use std::{env, path::Path};
use windows_sys::Win32::UI::HiDpi::*;

fn run() -> Result<()> {
    unsafe {
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut args = env::args_os().skip(1);
    let arg = args.next();
    match arg.as_deref().and_then(|a| a.to_str()) {
        None => host::run(true),
        Some("--background") => host::run(false),
        Some("--editor") => editor::open(),
        Some("--stop") => host::stop(),
        Some("--displays") => {
            println!("{}", serde_json::to_string_pretty(&displays::detect()?)?);
            Ok(())
        }
        Some("--apply") => {
            let file = args
                .next()
                .context("Usage: multipapr --apply <media path>")?;
            let media = gallery::import(Path::new(&file))?.saved.media;
            let previous = storage::load_optional()?;
            let mut profiles = storage::load_profiles().ok();
            let first = previous.as_ref().is_none_or(layout::Settings::is_empty)
                && profiles
                    .as_ref()
                    .is_some_and(|p| p.iter().all(layout::Profile::is_empty));
            let mut s = layout::Settings::new(media);
            if let Some(previous) = previous {
                s.calibration = previous.calibration;
                s.upscaling = previous.upscaling;
                s.pitch = previous.pitch;
                s.profile = previous.profile;
                s.screens = previous.screens;
            }
            let displays = displays::detect()?;
            let layout = layout::Layout::new(&displays, &s.calibration);
            let pitch = layout.pitch(&s);
            s.wallpaper.cover(pitch, layout.bounds);
            layout.freeze(&displays, &mut s);
            let set = layout::display_set(&displays);
            if let Some(profiles) = &mut profiles {
                layout::remember(profiles, &mut s, &set);
            } else {
                s.display_set = set;
            }
            storage::apply(&s)?;
            if let Some(profiles) = profiles {
                storage::save_profiles(&profiles)?;
            }
            if first {
                storage::startup(true)?;
            }
            host::start()?;
            Ok(())
        }
        Some(other) => anyhow::bail!("Unknown option {other}"),
    }
}
fn main() {
    if let Err(e) = run() {
        let message = format!("{e:#}");
        eprintln!("{message}");
        if let Ok(dir) = storage::home() {
            let _ = std::fs::write(dir.join("error.txt"), &message);
        }
        if env::args_os().len() == 1
            || env::args_os().nth(1).as_deref() == Some("--editor".as_ref())
        {
            rfd::MessageDialog::new()
                .set_title("Multipapr")
                .set_description(&message)
                .set_level(rfd::MessageLevel::Error)
                .show();
        }
        std::process::exit(1);
    }
}
