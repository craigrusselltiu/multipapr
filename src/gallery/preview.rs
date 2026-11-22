use super::{
    Format, Listing, Source, desktophut, get, motionbgs, read_wallpaper_header, trusted_url,
};
use crate::{http::Cancellation, process, storage};
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{Read, Write},
    path::Path,
    thread,
};
use url::Url;
use windows_sys::Win32::Networking::WinHttp::WINHTTP_QUERY_CONTENT_TYPE;

const MAX_VIDEO_BYTES: usize = 8 * 1024 * 1024;
const MAX_FRAMES: usize = 36;

pub struct PreviewFrames(Vec<u8>);

impl PreviewFrames {
    pub const SIZE: [usize; 2] = [256, 144];
    pub const FPS: f64 = 10.;
    const FRAME_BYTES: usize = Self::SIZE[0] * Self::SIZE[1] * 3;

    pub fn frame(&self, index: usize) -> (usize, &[u8]) {
        let index = index % (self.0.len() / Self::FRAME_BYTES);
        let start = index * Self::FRAME_BYTES;
        (index, &self.0[start..start + Self::FRAME_BYTES])
    }
}

pub(super) fn video_url(source: Source, raw: &str) -> Option<Url> {
    match source {
        Source::DesktopHut => {
            trusted_url(raw, desktophut::PREVIEWS).filter(|url| url.path().ends_with(".mp4"))
        }
        Source::MotionBgs => trusted_url(raw, &[motionbgs::PREVIEW]).filter(|url| {
            url.path()
                .strip_prefix("/media/")
                .and_then(|path| path.split_once('/'))
                .is_some_and(|(id, name)| {
                    !id.is_empty()
                        && id.bytes().all(|byte| byte.is_ascii_digit())
                        && !name.contains('/')
                        && name.ends_with(".960x540.mp4")
                })
        }),
        Source::Wallhaven => None,
    }
}

pub fn motion_preview(listing: &Listing, cancellation: &Cancellation) -> Result<PreviewFrames> {
    ensure!(listing.animated(), "Expected an animated wallpaper");
    let url = if let Some(raw) = &listing.preview {
        video_url(listing.source, raw).context("Unrecognized video preview URL")?
    } else {
        let page = listing
            .source
            .page(&listing.page)
            .context("Unrecognized wallpaper page")?;
        match listing.source {
            Source::MotionBgs => motionbgs::resolve_preview(&page, cancellation)?,
            _ => bail!("Wallpaper has no video preview"),
        }
    };
    let response = get(&url, 10_000, Some(cancellation))?;
    let mime = response
        .header_string(WINHTTP_QUERY_CONTENT_TYPE)
        .unwrap_or_default();
    ensure!(
        Format::from_mime(&mime) == Some(Format::Mp4),
        "Unsupported preview content type"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|size| size <= MAX_VIDEO_BYTES as u64),
        "Video preview is too large"
    );
    let bytes = response.read_all(MAX_VIDEO_BYTES)?;
    cancellation.check()?;
    read_wallpaper_header(Format::Mp4, &mut bytes.as_slice())?;
    decode(Some("mov"), None, bytes)
}

pub fn local_motion_preview(path: &Path) -> Result<PreviewFrames> {
    let demuxer = path
        .extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| match ext.to_ascii_lowercase().as_str() {
            "mp4" | "m4v" | "mov" => Some("mov"),
            "mkv" | "webm" => Some("matroska"),
            "ogv" => Some("ogg"),
            _ => None,
        });
    decode(demuxer, Some(path), Vec::new())
}

fn decode(demuxer: Option<&str>, path: Option<&Path>, bytes: Vec<u8>) -> Result<PreviewFrames> {
    let [width, height] = PreviewFrames::SIZE;
    let fps = PreviewFrames::FPS;
    let max_pixels = if path.is_some() {
        33_177_600
    } else {
        4_147_200
    };
    let mut command = storage::preview_media_command(demuxer)?;
    command
        .args([
            "--no-config",
            "--terminal=no",
            "--no-audio",
            "--no-sub",
            "--vd-lavc-threads=2",
            "--o=pipe:1",
            "--of=rawvideo",
            "--ovc=rawvideo",
        ])
        .arg(format!("--vd-lavc-o=max_pixels={max_pixels}"))
        .arg(format!("--frames={MAX_FRAMES}"))
        .arg(format!(
            "--vf=lavfi=[fps={fps},scale={width}:{height}:force_original_aspect_ratio=increase,crop={width}:{height},setsar=1,format=rgb24]"
        ));
    if let Some(path) = path {
        command.arg("--").arg(path);
    } else {
        command.arg("-");
    }
    let (child, mut input, output) =
        process::spawn_preview_decoder(&command).context("Cannot start the preview decoder")?;
    thread::scope(|scope| {
        let writer = scope.spawn(move || input.write_all(&bytes));
        let reader = scope.spawn(move || {
            let mut frames = Vec::new();
            output
                .take((MAX_FRAMES * PreviewFrames::FRAME_BYTES + 1) as u64)
                .read_to_end(&mut frames)?;
            Ok::<_, std::io::Error>(frames)
        });
        let success =
            process::decoder_succeeded(&child, if path.is_some() { 12_000 } else { 8_000 });
        // The decoder can close stdin before consuming the whole clip once it has enough frames.
        let _ = writer.join();
        let frames = reader
            .join()
            .map_err(|_| anyhow::anyhow!("Preview reader stopped"))??;
        ensure!(
            success
                && frames.len().is_multiple_of(PreviewFrames::FRAME_BYTES)
                && (2 * PreviewFrames::FRAME_BYTES..=MAX_FRAMES * PreviewFrames::FRAME_BYTES)
                    .contains(&frames.len()),
            "Cannot decode the video preview"
        );
        Ok(PreviewFrames(frames))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_urls_accept_only_supported_media_from_the_provider() {
        for (source, preview, other) in [
            (
                Source::MotionBgs,
                "https://motionbgs.com/media/46/forest.960x540.mp4",
                "https://motionbgs.com/media/46/forest.3840x2160.mp4",
            ),
            (
                Source::DesktopHut,
                "https://media.desktophut.com/previews/forest-preview.mp4",
                "https://media.desktophut.com/files/forest.mp4",
            ),
        ] {
            assert!(video_url(source, preview).is_some());
            assert!(video_url(source, other).is_none());
            assert!(video_url(source, "https://example.org/previews/wallpaper.mp4").is_none());
            assert!(video_url(source, &format!("{preview}.exe")).is_none());
        }
    }
}
