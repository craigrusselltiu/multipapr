use crate::{
    layout::{Display, Layout, Settings, Upscaling},
    process,
    shader::Part,
    storage,
};
use anyhow::{Context, Result, ensure};
use image::{DynamicImage, ImageReader, imageops::FilterType};
use std::{fs, path::Path, thread};

const STRIP_ROWS: usize = 64;

pub struct SourceImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Crop {
    pub u: f32,
    pub v: f32,
    pub w: f32,
    pub h: f32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ResampleFilter {
    Bilinear,
    CatmullRom,
    Spline36,
    Lanczos3,
}

#[derive(Clone, Copy)]
struct Tap {
    indices: [usize; 6],
    weights: [f32; 6],
    count: usize,
}

pub fn display_crop(
    layout: &Layout,
    settings: &Settings,
    displays: &[Display],
    index: usize,
) -> Crop {
    let panel = layout.panels[index];
    let source = layout.media(settings, settings.shown(&displays[index].id));
    Crop {
        u: (panel.x - source.x) / source.w,
        v: (panel.y - source.y) / source.h,
        w: panel.w / source.w,
        h: panel.h / source.h,
    }
}

pub fn decode_source(path: &Path) -> Result<SourceImage> {
    if let Ok(reader) = ImageReader::open(path).and_then(ImageReader::with_guessed_format)
        && let Ok(decoded) = reader.decode()
    {
        return Ok(flatten_to_rgb(decoded));
    }
    decode_video_first_frame(path)
}

fn flatten_to_rgb(img: DynamicImage) -> SourceImage {
    let width = img.width();
    let height = img.height();
    let rgb = match img {
        DynamicImage::ImageRgb8(buf) => buf.into_raw(),
        other => {
            let rgba = other.into_rgba8();
            let mut out = Vec::with_capacity(width as usize * height as usize * 3);
            for px in rgba.pixels() {
                let a = u16::from(px[3]);
                if a == 255 {
                    out.extend_from_slice(&px.0[..3]);
                } else if a == 0 {
                    out.extend_from_slice(&[0, 0, 0]);
                } else {
                    out.push(((u16::from(px[0]) * a + 127) / 255) as u8);
                    out.push(((u16::from(px[1]) * a + 127) / 255) as u8);
                    out.push(((u16::from(px[2]) * a + 127) / 255) as u8);
                }
            }
            out
        }
    };
    SourceImage { width, height, rgb }
}

fn parse_video_params(stdout: &[u8]) -> Option<(u32, u32, u32)> {
    let text = String::from_utf8_lossy(stdout);
    text.lines().find_map(|line| {
        let (_, rest) = line.split_once("MULTIPAPR:")?;
        let mut values = rest.trim().split(':');
        let w = values.next()?.parse::<u32>().ok()?;
        let h = values.next()?.parse::<u32>().ok()?;
        let rotation = values
            .next()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(0);
        (w > 0 && h > 0).then_some((w, h, rotation % 360))
    })
}

fn decode_video_first_frame(path: &Path) -> Result<SourceImage> {
    let dir = storage::home()?;
    let temp = tempfile::Builder::new()
        .prefix("frame-")
        .tempdir_in(&dir)?;
    let mut command = storage::wallpaper_command(path)?;
    command
        .args([
            "--no-config",
            "--terminal=yes",
            "--input-terminal=no",
            "--hwdec=no",
            "--video-rotate=no",
            "--vo=image",
            "--vo-image-format=png",
            "--vo-image-png-compression=0",
            "--vo-image-png-filter=0",
            "--vo-image-high-bit-depth=no",
            "--frames=1",
            "--ao=null",
            "--no-audio",
            "--no-sub",
            "--image-display-duration=0",
            "--term-status-msg=MULTIPAPR:${video-params/dw}:${video-params/dh}:${video-params/rotate}",
        ])
        .arg(format!("--vo-image-outdir={}", temp.path().display()))
        .arg("--")
        .arg(path);
    let output = process::decoder_output(&command).context("Cannot decode first frame with mpv")?;
    let frame_path = fs::read_dir(temp.path())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("png"))
        .context("mpv did not write a first frame")?;
    let mut img = ImageReader::open(&frame_path)?
        .with_guessed_format()?
        .decode()
        .context("Cannot read extracted video frame")?;
    if let Some((dw, dh, rotation)) = parse_video_params(&output) {
        if (img.width(), img.height()) != (dw, dh) {
            img = img.resize_exact(dw, dh, FilterType::Triangle);
        }
        img = match rotation {
            90 => img.rotate90(),
            180 => img.rotate180(),
            270 => img.rotate270(),
            _ => img,
        };
    }
    Ok(flatten_to_rgb(img))
}

fn choose_filter(src: &SourceImage, crop: Crop, upscaling: Upscaling, out_w: usize, out_h: usize) -> ResampleFilter {
    if crop.w * src.width as f32 >= out_w as f32 && crop.h * src.height as f32 >= out_h as f32 {
        return ResampleFilter::Bilinear;
    }
    match upscaling {
        Upscaling::None => ResampleFilter::Bilinear,
        Upscaling::CatmullRom => ResampleFilter::CatmullRom,
        Upscaling::Spline36 => ResampleFilter::Spline36,
        Upscaling::Lanczos | Upscaling::EwaLanczosSharp => ResampleFilter::Lanczos3,
    }
}

fn spline36(x: f32) -> f32 {
    let x = x.abs();
    if x < 1.0 {
        ((13.0 / 11.0 * x - 453.0 / 209.0) * x - 3.0 / 209.0) * x + 1.0
    } else if x < 2.0 {
        let x = x - 1.0;
        ((-6.0 / 11.0 * x + 270.0 / 209.0) * x - 156.0 / 209.0) * x
    } else if x < 3.0 {
        let x = x - 2.0;
        ((1.0 / 11.0 * x - 45.0 / 209.0) * x + 26.0 / 209.0) * x
    } else {
        0.0
    }
}

fn lanczos3(x: f32) -> f32 {
    let x = x.abs();
    if x < 1e-5 {
        return 1.0;
    }
    if x >= 3.0 {
        return 0.0;
    }
    let p = std::f32::consts::PI * x;
    3.0 * p.sin() * (p / 3.0).sin() / (p * p)
}

fn build_taps(
    out_len: usize,
    src_len: usize,
    offset: f32,
    span: f32,
    filter: ResampleFilter,
) -> (usize, Vec<Tap>) {
    if out_len == 0 || src_len == 0 || span <= 0.0 {
        return (0, Vec::new());
    }
    let max_idx = (src_len - 1) as i32;
    let mut start = out_len;
    let mut end = 0usize;
    for i in 0..out_len {
        let uv = ((i as f32 + 0.5) / out_len as f32) * span + offset;
        if (0.0..1.0).contains(&uv) {
            if start == out_len {
                start = i;
            }
            end = i + 1;
        }
    }
    if start >= end {
        return (0, Vec::new());
    }
    let mut taps = Vec::with_capacity(end - start);
    for i in start..end {
        let uv = ((i as f32 + 0.5) / out_len as f32) * span + offset;
        let pos = uv * src_len as f32 - 0.5;
        let base = pos.floor() as i32;
        let f = pos - base as f32;
        let mut indices = [0usize; 6];
        let mut weights = [0.0f32; 6];
        let count = match filter {
            ResampleFilter::Bilinear => {
                indices[0] = base.clamp(0, max_idx) as usize;
                indices[1] = (base + 1).clamp(0, max_idx) as usize;
                weights[0] = 1.0 - f;
                weights[1] = f;
                2
            }
            ResampleFilter::CatmullRom => {
                let w = [
                    f * (-0.5 + f * (1.0 - 0.5 * f)),
                    1.0 + f * f * (-2.5 + 1.5 * f),
                    f * (0.5 + f * (2.0 - 1.5 * f)),
                    f * f * (-0.5 + 0.5 * f),
                ];
                for k in 0..4 {
                    indices[k] = (base + k as i32 - 1).clamp(0, max_idx) as usize;
                    weights[k] = w[k];
                }
                4
            }
            ResampleFilter::Spline36 | ResampleFilter::Lanczos3 => {
                let mut sum = 0.0f32;
                for k in 0..6 {
                    let d = (k as i32 - 2) as f32 - f;
                    let w = if filter == ResampleFilter::Spline36 {
                        spline36(d)
                    } else {
                        lanczos3(d)
                    };
                    indices[k] = (base + k as i32 - 2).clamp(0, max_idx) as usize;
                    weights[k] = w;
                    sum += w;
                }
                if sum != 0.0 {
                    let inv = 1.0 / sum;
                    for w in &mut weights {
                        *w *= inv;
                    }
                }
                6
            }
        };
        taps.push(Tap {
            indices,
            weights,
            count,
        });
    }
    (start, taps)
}

#[allow(clippy::too_many_arguments)]
fn render_band(
    src: &SourceImage,
    x_start: usize,
    x_taps: &[Tap],
    y_taps: &[Tap],
    dst_rows: &mut [u8],
    dst_stride: usize,
    dst_x: usize,
    bpp: usize,
    bottom_up: bool,
) {
    let active_w = x_taps.len();
    let src_stride = src.width as usize * 3;
    let total_rows = y_taps.len();
    let mut horiz = Vec::<f32>::new();

    for strip_start in (0..total_rows).step_by(STRIP_ROWS) {
        let strip_end = (strip_start + STRIP_ROWS).min(total_rows);
        let strip_y = &y_taps[strip_start..strip_end];
        let mut sy_min = usize::MAX;
        let mut sy_max = 0usize;
        for tap in strip_y {
            for &idx in &tap.indices[..tap.count] {
                sy_min = sy_min.min(idx);
                sy_max = sy_max.max(idx);
            }
        }
        let sy_count = sy_max - sy_min + 1;
        let horiz_stride = active_w * 3;
        horiz.resize(sy_count * horiz_stride, 0.0);

        for (local_y, sy) in (sy_min..=sy_max).enumerate() {
            let src_row = &src.rgb[sy * src_stride..(sy + 1) * src_stride];
            let h_row = &mut horiz[local_y * horiz_stride..(local_y + 1) * horiz_stride];
            for (x_i, tap) in x_taps.iter().enumerate() {
                let mut r = 0.0f32;
                let mut g = 0.0f32;
                let mut b = 0.0f32;
                for k in 0..tap.count {
                    let off = tap.indices[k] * 3;
                    let w = tap.weights[k];
                    r += src_row[off] as f32 * w;
                    g += src_row[off + 1] as f32 * w;
                    b += src_row[off + 2] as f32 * w;
                }
                let out = x_i * 3;
                h_row[out] = r;
                h_row[out + 1] = g;
                h_row[out + 2] = b;
            }
        }

        for (local_out_y, tap) in strip_y.iter().enumerate() {
            let band_y = strip_start + local_out_y;
            let row_slot = if bottom_up {
                total_rows - 1 - band_y
            } else {
                band_y
            };
            let dst_row = &mut dst_rows[row_slot * dst_stride..(row_slot + 1) * dst_stride];
            let dst_off_base = (dst_x + x_start) * bpp;
            for x_i in 0..active_w {
                let mut r = 0.0f32;
                let mut g = 0.0f32;
                let mut b = 0.0f32;
                let col = x_i * 3;
                for k in 0..tap.count {
                    let h_off = (tap.indices[k] - sy_min) * horiz_stride + col;
                    let w = tap.weights[k];
                    r += horiz[h_off] * w;
                    g += horiz[h_off + 1] * w;
                    b += horiz[h_off + 2] * w;
                }
                let p = dst_off_base + x_i * bpp;
                dst_row[p] = b.clamp(0.0, 255.0).round() as u8;
                dst_row[p + 1] = g.clamp(0.0, 255.0).round() as u8;
                dst_row[p + 2] = r.clamp(0.0, 255.0).round() as u8;
                if bpp == 4 {
                    dst_row[p + 3] = 255;
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_region(
    src: &SourceImage,
    crop: Crop,
    upscaling: Upscaling,
    out_w: usize,
    out_h: usize,
    dst: &mut [u8],
    dst_stride: usize,
    dst_x: usize,
    dst_y: usize,
    bpp: usize,
    bottom_up: bool,
) {
    let filter = choose_filter(src, crop, upscaling, out_w, out_h);
    let (x_start, x_taps) = build_taps(out_w, src.width as usize, crop.u, crop.w, filter);
    let (y_start, y_taps) = build_taps(out_h, src.height as usize, crop.v, crop.h, filter);
    if x_taps.is_empty() || y_taps.is_empty() {
        return;
    }
    let active_h = y_taps.len();
    let dst_row_start = if bottom_up {
        out_h - (y_start + active_h)
    } else {
        dst_y + y_start
    };
    let active_dst = &mut dst[dst_row_start * dst_stride..(dst_row_start + active_h) * dst_stride];
    let threads = if x_taps.len() * active_h < 128 * 128 {
        1
    } else {
        thread::available_parallelism()
            .map_or(1, usize::from)
            .clamp(1, 12)
            .min(active_h.div_ceil(STRIP_ROWS))
    };
    if threads <= 1 {
        render_band(
            src, x_start, &x_taps, &y_taps, active_dst, dst_stride, dst_x, bpp, bottom_up,
        );
        return;
    }
    let rows_per_thread = active_h.div_ceil(threads);
    thread::scope(|scope| {
        if bottom_up {
            // In bottom-up BMP layout, earlier `y_taps` map to higher row offsets in `active_dst`.
            let mut remaining_dst = active_dst;
            let mut offset = 0usize;
            while offset < active_h {
                let count = rows_per_thread.min(active_h - offset);
                let split_at = remaining_dst.len() - count * dst_stride;
                let (head, chunk) = remaining_dst.split_at_mut(split_at);
                remaining_dst = head;
                let y_slice = &y_taps[offset..offset + count];
                let x_taps = &x_taps;
                scope.spawn(move || {
                    render_band(src, x_start, x_taps, y_slice, chunk, dst_stride, dst_x, bpp, true);
                });
                offset += count;
            }
        } else {
            let mut remaining_dst = active_dst;
            let mut offset = 0usize;
            while offset < active_h {
                let count = rows_per_thread.min(active_h - offset);
                let (chunk, tail) = remaining_dst.split_at_mut(count * dst_stride);
                remaining_dst = tail;
                let y_slice = &y_taps[offset..offset + count];
                let x_taps = &x_taps;
                scope.spawn(move || {
                    render_band(src, x_start, x_taps, y_slice, chunk, dst_stride, dst_x, bpp, false);
                });
                offset += count;
            }
        }
    });
}

pub fn render_part_bgra(
    src: &SourceImage,
    layout: &Layout,
    settings: &Settings,
    displays: &[Display],
    part: &Part,
) -> Result<(u32, u32, Vec<u8>)> {
    let desktop = part.desktop(displays);
    let total_w = desktop.w.round().max(1.0) as usize;
    let total_h = desktop.h.round().max(1.0) as usize;
    let dst_stride = total_w * 4;
    let mut bgra = vec![0u8; dst_stride * total_h];
    for &i in &part.displays {
        let r = displays[i].px;
        let dx = (r.x - desktop.x).round().max(0.0) as usize;
        let dy = (r.y - desktop.y).round().max(0.0) as usize;
        let dw = r.w.round().max(1.0) as usize;
        let dh = r.h.round().max(1.0) as usize;
        if dx + dw > total_w || dy + dh > total_h {
            continue;
        }
        let crop = display_crop(layout, settings, displays, i);
        render_region(
            src,
            crop,
            settings.upscaling,
            dw,
            dh,
            &mut bgra,
            dst_stride,
            dx,
            dy,
            4,
            false,
        );
    }
    Ok((total_w as u32, total_h as u32, bgra))
}

fn write_bmp_header(buf: &mut [u8], width: u32, height: u32, pixel_bytes: u32) {
    let file_size = 54u32 + pixel_bytes;
    buf[0..2].copy_from_slice(b"BM");
    buf[2..6].copy_from_slice(&file_size.to_le_bytes());
    buf[6..10].copy_from_slice(&0u32.to_le_bytes());
    buf[10..14].copy_from_slice(&54u32.to_le_bytes());
    buf[14..18].copy_from_slice(&40u32.to_le_bytes());
    buf[18..22].copy_from_slice(&(width as i32).to_le_bytes());
    buf[22..26].copy_from_slice(&(height as i32).to_le_bytes());
    buf[26..28].copy_from_slice(&1u16.to_le_bytes());
    buf[28..30].copy_from_slice(&24u16.to_le_bytes());
    buf[30..34].copy_from_slice(&0u32.to_le_bytes());
    buf[34..38].copy_from_slice(&pixel_bytes.to_le_bytes());
    buf[38..42].copy_from_slice(&2835i32.to_le_bytes());
    buf[42..46].copy_from_slice(&2835i32.to_le_bytes());
    buf[46..50].copy_from_slice(&0u32.to_le_bytes());
    buf[50..54].copy_from_slice(&0u32.to_le_bytes());
}

pub fn render_display_bmp(
    src: &SourceImage,
    crop: Crop,
    upscaling: Upscaling,
    width: u32,
    height: u32,
) -> Result<Vec<u8>> {
    ensure!(width > 0 && height > 0, "Invalid display dimensions");
    let out_w = width as usize;
    let out_h = height as usize;
    let row_stride = (out_w * 3 + 3) & !3;
    let pixel_bytes = row_stride
        .checked_mul(out_h)
        .and_then(|n| u32::try_from(n).ok())
        .context("Display bitmap is too large")?;
    let mut bmp = vec![0u8; 54 + pixel_bytes as usize];
    write_bmp_header(&mut bmp, width, height, pixel_bytes);
    render_region(
        src,
        crop,
        upscaling,
        out_w,
        out_h,
        &mut bmp[54..],
        row_stride,
        0,
        0,
        3,
        true,
    );
    Ok(bmp)
}

pub fn black_bmp() -> Vec<u8> {
    let mut bmp = vec![0u8; 54 + 4];
    write_bmp_header(&mut bmp, 1, 1, 4);
    bmp
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{Media, Rect};

    #[test]
    fn renders_exact_crop_and_black_margins_in_both_bgra_and_bmp() {
        // 4x2 source: left half red, right half blue.
        let mut rgb = Vec::with_capacity(4 * 2 * 3);
        for _y in 0..2 {
            for x in 0..4 {
                if x < 2 {
                    rgb.extend_from_slice(&[240, 20, 10]);
                } else {
                    rgb.extend_from_slice(&[10, 30, 220]);
                }
            }
        }
        let src = SourceImage {
            width: 4,
            height: 2,
            rgb,
        };
        let crop = Crop {
            u: -0.5,
            v: 0.0,
            w: 2.0,
            h: 1.0,
        };
        // Output 8x2: cols 0..2 black, 2..4 red, 4..6 blue, 6..8 black.
        let bmp = render_display_bmp(&src, crop, Upscaling::None, 8, 2).unwrap();
        let decoded = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp)
            .unwrap()
            .into_rgb8();
        assert_eq!((decoded.width(), decoded.height()), (8, 2));
        for y in 0..2 {
            assert_eq!(decoded.get_pixel(0, y).0, [0, 0, 0]);
            assert_eq!(decoded.get_pixel(1, y).0, [0, 0, 0]);
            assert_eq!(decoded.get_pixel(2, y).0, [240, 20, 10]);
            assert_eq!(decoded.get_pixel(5, y).0, [10, 30, 220]);
            assert_eq!(decoded.get_pixel(6, y).0, [0, 0, 0]);
            assert_eq!(decoded.get_pixel(7, y).0, [0, 0, 0]);
        }
    }

    #[test]
    fn multithreaded_bottom_up_bmp_matches_top_down_part_bgra() {
        let w = 200u32;
        let h = 150u32;
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            for x in 0..w {
                rgb.push((x % 256) as u8);
                rgb.push((y % 256) as u8);
                rgb.push(((x + y) % 256) as u8);
            }
        }
        let src = SourceImage {
            width: w,
            height: h,
            rgb,
        };
        let display = Display {
            id: "a".into(),
            name: "a".into(),
            primary: true,
            px: Rect::new(0., 0., 300., 225.),
            mm: [300., 225.],
            estimated: false,
            mirrors: Vec::new(),
            mirror_ids: Vec::new(),
        };
        let mut settings = Settings::new(Media {
            path: "sample.png".into(),
            width: w,
            height: h,
        });
        settings.upscaling = Upscaling::CatmullRom;
        let layout = Layout::new(std::slice::from_ref(&display), &settings.calibration);
        settings.wallpaper.cover(layout.pitch(&settings), layout.bounds);
        let part = Part {
            media: "sample.png".into(),
            displays: vec![0],
            paused: true,
        };
        let (pw, ph, bgra) =
            render_part_bgra(&src, &layout, &settings, std::slice::from_ref(&display), &part)
                .unwrap();
        assert_eq!((pw, ph), (300, 225));
        let crop = display_crop(&layout, &settings, std::slice::from_ref(&display), 0);
        let bmp = render_display_bmp(&src, crop, settings.upscaling, 300, 225).unwrap();
        let decoded = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp)
            .unwrap()
            .into_rgb8();
        for y in [0, 50, 112, 224] {
            for x in [0, 75, 149, 299] {
                let off = (y as usize * 300 + x as usize) * 4;
                assert_eq!(
                    decoded.get_pixel(x, y).0,
                    [bgra[off + 2], bgra[off + 1], bgra[off]],
                    "pixel mismatch at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn black_bmp_is_valid_1x1_black() {
        let bmp = black_bmp();
        let decoded = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp)
            .unwrap()
            .into_rgb8();
        assert_eq!((decoded.width(), decoded.height()), (1, 1));
        assert_eq!(decoded.get_pixel(0, 0).0, [0, 0, 0]);
    }
}
