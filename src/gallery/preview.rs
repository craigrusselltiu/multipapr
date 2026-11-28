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
const MAX_LONG_EDGE: usize = 256;
const MAX_SHORT_EDGE: usize = 144;
const MAX_HEADER_BYTES: usize = 32;
const MAX_FRAME_BYTES: usize = MAX_LONG_EDGE * MAX_SHORT_EDGE * 3 + MAX_HEADER_BYTES;

pub struct PreviewFrames {
    size: [usize; 2],
    header_len: usize,
    data: Vec<u8>,
}

impl PreviewFrames {
    pub const FPS: f64 = 10.;

    fn from_ppm_frames(data: Vec<u8>) -> Option<Self> {
        let header = &data[..data.len().min(MAX_HEADER_BYTES)];
        let mut lines = header.splitn(4, |&byte| byte == b'\n');
        if lines.next()? != b"P6" {
            return None;
        }
        let (width, height) = std::str::from_utf8(lines.next()?).ok()?.split_once(' ')?;
        let width: usize = width.parse().ok()?;
        let height: usize = height.parse().ok()?;
        if lines.next()? != b"255"
            || width.min(height) == 0
            || width.max(height) > MAX_LONG_EDGE
            || width.min(height) > MAX_SHORT_EDGE
        {
            return None;
        }
        let header_len = header.len() - lines.next()?.len();
        let prefix = &data[..header_len];
        let mut frames = data.chunks_exact(header_len + width * height * 3);
        if !(2..=MAX_FRAMES).contains(&frames.len())
            || !frames.remainder().is_empty()
            || !frames.all(|frame| frame.starts_with(prefix))
        {
            return None;
        }
        Some(Self {
            size: [width, height],
            header_len,
            data,
        })
    }

    pub fn size(&self) -> [usize; 2] {
        self.size
    }

    pub fn frame(&self, index: usize) -> (usize, &[u8]) {
        let stride = self.header_len + self.size[0] * self.size[1] * 3;
        let index = index % (self.data.len() / stride);
        let start = index * stride + self.header_len;
        (index, &self.data[start..(index + 1) * stride])
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
    decode(storage::media_demuxer(path), Some(path), Vec::new())
}

fn decode(demuxer: Option<&str>, path: Option<&Path>, bytes: Vec<u8>) -> Result<PreviewFrames> {
    let fps = PreviewFrames::FPS;
    let max_pixels = if path.is_some() {
        storage::MAX_MEDIA_PIXELS
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
            "--of=image2pipe",
            "--ovc=ppm",
        ])
        .arg(format!("--vd-lavc-o=max_pixels={max_pixels}"))
        .arg(format!("--frames={MAX_FRAMES}"))
        // PPM frames carry their dimensions after mpv applies rotation metadata.
        .arg(format!(
            "--vf=lavfi=[fps={fps},\
             scale=w='if(gte(dar,1),{MAX_LONG_EDGE},{MAX_SHORT_EDGE})':\
             h='if(gte(dar,1),{MAX_SHORT_EDGE},{MAX_LONG_EDGE})':\
             force_original_aspect_ratio=decrease:reset_sar=1,format=rgb24]"
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
                .take((MAX_FRAMES * MAX_FRAME_BYTES + 1) as u64)
                .read_to_end(&mut frames)?;
            Ok::<_, std::io::Error>(frames)
        });
        let success =
            process::decoder_succeeded(&child, if path.is_some() { 12_000 } else { 8_000 });
        // The decoder can close stdin before consuming the whole clip once it has enough frames.
        let _ = writer.join();
        let data = reader
            .join()
            .map_err(|_| anyhow::anyhow!("Preview reader stopped"))??;
        ensure!(success, "Cannot decode the video preview");
        PreviewFrames::from_ppm_frames(data).context("Invalid video preview frames")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portrait_frames_use_the_decoded_dimensions_and_strip_the_headers() {
        let header = b"P6\n3 5\n255\n";
        let first = [10; 3 * 5 * 3];
        let second = [20; 3 * 5 * 3];
        let data = [header.as_slice(), &first, header.as_slice(), &second].concat();
        let frames = PreviewFrames::from_ppm_frames(data.clone()).unwrap();
        assert_eq!(frames.size(), [3, 5]);
        assert_eq!(frames.frame(0), (0, first.as_slice()));
        assert_eq!(frames.frame(3), (1, second.as_slice()));
        assert!(PreviewFrames::from_ppm_frames(data[..data.len() - 1].to_vec()).is_none());
    }

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
