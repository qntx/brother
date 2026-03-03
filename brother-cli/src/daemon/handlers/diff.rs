//! Diff handlers: snapshot diff, screenshot diff, URL diff, PNG utilities.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData};

use super::super::{get_page, DaemonState};

/// Compare current snapshot against baseline text.
pub(in crate::daemon) async fn cmd_diff_snapshot(
    state: &Arc<Mutex<DaemonState>>,
    baseline: &str,
    options: brother::SnapshotOptions,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let snap = match page.snapshot_with(options).await {
        Ok(s) => s,
        Err(e) => return Response::error(format!("snapshot failed: {e}")),
    };

    let current = snap.tree();
    let result = brother::diff_snapshots(baseline, current);
    let summary = result.summary();

    Response::ok_data(ResponseData::DiffSnapshot {
        added: result.added,
        removed: result.removed,
        unchanged: result.unchanged,
        diff: result.diff,
        summary,
    })
}

/// Compare two URLs: navigate to each, take snapshot, optionally diff screenshots.
pub(in crate::daemon) async fn cmd_diff_url(
    state: &Arc<Mutex<DaemonState>>,
    url_a: &str,
    url_b: &str,
    screenshot: bool,
    threshold: u8,
    options: brother::SnapshotOptions,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Navigate to URL A and take snapshot (+ optional screenshot)
    if let Err(e) = page.goto(url_a).await {
        return Response::error(format!("navigate to URL A: {e}"));
    }
    let snap_a = match page.snapshot_with(options.clone()).await {
        Ok(s) => s.tree().to_owned(),
        Err(e) => return Response::error(format!("snapshot A: {e}")),
    };
    let screenshot_a = if screenshot {
        match page.screenshot(false, None, "png", Some(80)).await {
            Ok(b) => Some(b),
            Err(e) => return Response::error(format!("screenshot A: {e}")),
        }
    } else {
        None
    };

    // Navigate to URL B and take snapshot (+ optional screenshot)
    if let Err(e) = page.goto(url_b).await {
        return Response::error(format!("navigate to URL B: {e}"));
    }
    let snap_b = match page.snapshot_with(options).await {
        Ok(s) => s.tree().to_owned(),
        Err(e) => return Response::error(format!("snapshot B: {e}")),
    };
    let screenshot_b = if screenshot {
        match page.screenshot(false, None, "png", Some(80)).await {
            Ok(b) => Some(b),
            Err(e) => return Response::error(format!("screenshot B: {e}")),
        }
    } else {
        None
    };

    // Diff snapshots
    let snap_result = brother::diff_snapshots(&snap_a, &snap_b);
    let mut output = snap_result.summary();

    // Optionally diff screenshots
    if let (Some(bytes_a), Some(bytes_b)) = (screenshot_a, screenshot_b) {
        let rgba_a = match decode_png_to_rgba(&bytes_a) {
            Ok(r) => r,
            Err(e) => return Response::error(format!("decode screenshot A: {e}")),
        };
        let rgba_b = match decode_png_to_rgba(&bytes_b) {
            Ok(r) => r,
            Err(e) => return Response::error(format!("decode screenshot B: {e}")),
        };
        let img_result = brother::diff_rgba(
            &rgba_a.pixels, rgba_a.width, rgba_a.height,
            &rgba_b.pixels, rgba_b.width, rgba_b.height,
            threshold,
        );

        if let Ok(diff_path) = generate_diff_image(&rgba_a, &rgba_b, threshold).await {
            output = format!("{output}\nscreenshot: {} | diff image: {diff_path}", img_result.summary());
        } else {
            output = format!("{output}\nscreenshot: {}", img_result.summary());
        }
    }

    // Return snapshot diff + extra screenshot info
    Response::ok_data(ResponseData::DiffSnapshot {
        diff: snap_result.diff,
        added: snap_result.added,
        removed: snap_result.removed,
        unchanged: snap_result.unchanged,
        summary: output,
    })
}

/// Compare current screenshot against baseline (base64-encoded PNG).
///
/// Decodes PNGs in Rust (no browser round-trip), generates a diff image
/// with red-highlighted pixels, and saves it to `~/.brother/tmp/diffs/`.
pub(in crate::daemon) async fn cmd_diff_screenshot(
    state: &Arc<Mutex<DaemonState>>,
    baseline_b64: &str,
    threshold: u8,
    full_page: bool,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Take current screenshot
    let current_bytes = match page.screenshot(full_page, None, "png", Some(80)).await {
        Ok(b) => b,
        Err(e) => return Response::error(format!("screenshot failed: {e}")),
    };

    // Decode baseline from base64
    let baseline_bytes = match base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        baseline_b64,
    ) {
        Ok(b) => b,
        Err(e) => return Response::error(format!("invalid baseline base64: {e}")),
    };

    // Decode both PNGs to RGBA using the png crate (no browser round-trip)
    let baseline_rgba = match decode_png_to_rgba(&baseline_bytes) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("baseline decode: {e}")),
    };
    let current_rgba = match decode_png_to_rgba(&current_bytes) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("current decode: {e}")),
    };

    let result = brother::diff_rgba(
        &baseline_rgba.pixels,
        baseline_rgba.width,
        baseline_rgba.height,
        &current_rgba.pixels,
        current_rgba.width,
        current_rgba.height,
        threshold,
    );

    // Generate diff image and save to disk
    let diff_path = match generate_diff_image(
        &baseline_rgba,
        &current_rgba,
        threshold,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => return Response::error(format!("diff image: {e}")),
    };

    Response::ok_data(ResponseData::DiffScreenshot {
        diff_path,
        total_pixels: result.total_pixels,
        diff_pixels: result.diff_pixels,
        diff_percentage: result.diff_percentage,
        size_mismatch: result.size_mismatch,
        summary: result.summary(),
    })
}

/// Decoded RGBA image data.
struct RgbaImage {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

/// Decode a PNG buffer to RGBA pixel data using the `png` crate.
fn decode_png_to_rgba(data: &[u8]) -> Result<RgbaImage, String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(data));
    let mut reader = decoder.read_info().map_err(|e| format!("png header: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("png frame: {e}"))?;
    buf.truncate(info.buffer_size());

    let width = info.width;
    let height = info.height;

    // Convert to RGBA if needed
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for chunk in buf.chunks_exact(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        png::ColorType::GrayscaleAlpha => {
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for chunk in buf.chunks_exact(2) {
                let g = chunk[0];
                rgba.extend_from_slice(&[g, g, g, chunk[1]]);
            }
            rgba
        }
        png::ColorType::Grayscale => {
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for &g in &buf {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
            rgba
        }
        other @ png::ColorType::Indexed => return Err(format!("unsupported color type: {other:?}")),
    };

    Ok(RgbaImage {
        pixels,
        width,
        height,
    })
}

/// Generate a diff PNG: different pixels in red, same pixels dimmed.
/// Saves to `~/.brother/tmp/diffs/diff-<timestamp>.png`.
async fn generate_diff_image(
    baseline: &RgbaImage,
    current: &RgbaImage,
    threshold: u8,
) -> Result<String, String> {
    let w = baseline.width.max(current.width);
    let h = baseline.height.max(current.height);
    let mut diff_pixels = vec![0u8; (w * h * 4) as usize];

    let thresh_sq = i32::from(threshold) * i32::from(threshold) * 3;

    for y in 0..h {
        for x in 0..w {
            let di = ((y * w + x) * 4) as usize;
            let get_pixel = |img: &RgbaImage, px: u32, py: u32| -> [u8; 4] {
                if px < img.width && py < img.height {
                    let i = ((py * img.width + px) * 4) as usize;
                    [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2], img.pixels[i + 3]]
                } else {
                    [0, 0, 0, 0]
                }
            };

            let a = get_pixel(baseline, x, y);
            let b = get_pixel(current, x, y);

            let dr = i32::from(a[0]) - i32::from(b[0]);
            let dg = i32::from(a[1]) - i32::from(b[1]);
            let db = i32::from(a[2]) - i32::from(b[2]);
            let dist_sq = dr * dr + dg * dg + db * db;

            if dist_sq > thresh_sq {
                // Different: red
                diff_pixels[di] = 255;
                diff_pixels[di + 1] = 0;
                diff_pixels[di + 2] = 0;
                diff_pixels[di + 3] = 255;
            } else {
                // Same: dimmed baseline
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    diff_pixels[di] = (f64::from(a[0]) * 0.3) as u8;
                    diff_pixels[di + 1] = (f64::from(a[1]) * 0.3) as u8;
                    diff_pixels[di + 2] = (f64::from(a[2]) * 0.3) as u8;
                    diff_pixels[di + 3] = 255;
                }
            }
        }
    }

    // Encode diff image as PNG
    let diff_dir = crate::protocol::runtime_dir()
        .ok_or_else(|| "cannot determine runtime dir".to_owned())?
        .join("tmp")
        .join("diffs");
    tokio::fs::create_dir_all(&diff_dir)
        .await
        .map_err(|e| format!("mkdir: {e}"))?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = diff_dir.join(format!("diff-{ts}.png"));

    let file = std::fs::File::create(&path).map_err(|e| format!("create file: {e}"))?;
    let buf_writer = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(buf_writer, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| format!("png header: {e}"))?;
    writer
        .write_image_data(&diff_pixels)
        .map_err(|e| format!("png write: {e}"))?;

    Ok(path.to_string_lossy().into_owned())
}
