#![windows_subsystem = "windows"]
mod desktop;
mod displays;
mod editor;
mod host;
mod layout;
mod shader;
mod storage;
mod tray;

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
            let media = storage::probe(Path::new(&file))?;
            let previous = storage::load().ok();
            let first = previous.is_none();
            let mut s = layout::Settings::new(media);
            if let Some(previous) = previous {
                s.calibration = previous.calibration;
                s.upscaling = previous.upscaling;
            }
            layout::Layout::new(&displays::detect()?, &s.calibration).cover(&mut s);
            storage::apply(&s)?;
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
