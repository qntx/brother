//! Emulation handlers: device, `device_list`, screencast.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData};

use crate::daemon::state::{DaemonState, get_page};

pub(in crate::daemon) fn cmd_device_list() -> Response {
    let names = brother::DevicePreset::list_names();
    let descriptions: Vec<serde_json::Value> = names
        .iter()
        .filter_map(|n| {
            brother::DevicePreset::lookup(n).map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "width": p.width,
                    "height": p.height,
                    "device_scale_factor": p.device_scale_factor,
                    "user_agent": p.user_agent,
                })
            })
        })
        .collect();
    Response::ok_data(ResponseData::Eval {
        value: serde_json::Value::Array(descriptions),
    })
}

pub(in crate::daemon) async fn cmd_device(state: &Arc<Mutex<DaemonState>>, name: &str) -> Response {
    let Some(preset) = brother::DevicePreset::lookup(name) else {
        let names = brother::DevicePreset::list_names().join(", ");
        return Response::error(format!("unknown device '{name}'. Available: {names}"));
    };
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(e) = page
        .set_viewport_scaled(preset.width, preset.height, preset.device_scale_factor)
        .await
    {
        return Response::error(e.to_string());
    }
    if let Err(e) = page.set_user_agent(preset.user_agent).await {
        return Response::error(e.to_string());
    }
    Response::ok_data(ResponseData::Text {
        text: format!(
            "emulating {} ({}x{} @{:.1}x, {})",
            preset.name, preset.width, preset.height, preset.device_scale_factor, preset.user_agent
        ),
    })
}

pub(in crate::daemon) async fn cmd_screencast_start(
    state: &Arc<Mutex<DaemonState>>,
    format: brother::ImageFormat,
    quality: u32,
    max_width: Option<u32>,
    max_height: Option<u32>,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::page::{
        StartScreencastFormat, StartScreencastParams,
    };
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let fmt = match format {
        brother::ImageFormat::Png => StartScreencastFormat::Png,
        brother::ImageFormat::Jpeg => StartScreencastFormat::Jpeg,
    };
    let params = StartScreencastParams::builder()
        .format(fmt)
        .quality(i64::from(quality))
        .max_width(i64::from(max_width.unwrap_or(1280)))
        .max_height(i64::from(max_height.unwrap_or(720)))
        .build();
    match page.inner().execute(params).await {
        Ok(_) => Response::ok_data(ResponseData::Text {
            text: format!("screencast started ({format}, quality={quality})"),
        }),
        Err(e) => Response::error(format!("screencast start failed: {e}")),
    }
}

/// Start video recording: enables CDP screencast and saves frames to disk.
pub(in crate::daemon) async fn cmd_record_start(
    state: &Arc<Mutex<DaemonState>>,
    path: Option<String>,
    quality: u32,
) -> Response {
    use base64::Engine;
    use chromiumoxide::cdp::browser_protocol::page::{
        ScreencastFrameAckParams, StartScreencastFormat, StartScreencastParams,
    };
    use futures::StreamExt;
    use std::sync::atomic::Ordering;

    {
        let guard = state.lock().await;
        if guard.recording_dir.is_some() {
            return Response::error("recording already in progress");
        }
    }

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Determine output directory
    let out_dir = path.unwrap_or_else(|| {
        let base = dirs::home_dir()
            .unwrap_or_else(|| std::path::PathBuf::from("."))
            .join(".brother")
            .join("recordings");
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        base.join(format!("rec_{ts}"))
            .to_string_lossy()
            .into_owned()
    });

    if let Err(e) = std::fs::create_dir_all(&out_dir) {
        return Response::error(format!("cannot create recording dir: {e}"));
    }

    // Reset frame counter
    let frame_count = {
        let guard = state.lock().await;
        guard.recording_frame_count.store(0, Ordering::Relaxed);
        Arc::clone(&guard.recording_frame_count)
    };

    // Start CDP screencast
    let params = StartScreencastParams::builder()
        .format(StartScreencastFormat::Jpeg)
        .quality(i64::from(quality))
        .max_width(1280_i64)
        .max_height(720_i64)
        .build();
    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("screencast start failed: {e}"));
    }

    // Set up event listener for frames
    let mut events = match page
        .inner()
        .event_listener::<chromiumoxide::cdp::browser_protocol::page::EventScreencastFrame>()
        .await
    {
        Ok(e) => e,
        Err(e) => return Response::error(format!("event listener failed: {e}")),
    };

    let (cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    {
        let mut guard = state.lock().await;
        guard.recording_dir = Some(out_dir.clone());
        guard.recording_cancel = Some(cancel_tx);
    }

    let page_inner = page.inner().clone();
    let dir = out_dir.clone();

    tokio::spawn(async move {
        loop {
            tokio::select! {
                Some(event) = events.next() => {
                    let n = frame_count.fetch_add(1, Ordering::Relaxed);
                    let filename = format!("frame_{n:05}.jpeg");
                    let filepath = std::path::Path::new(&dir).join(&filename);

                    // Decode base64 frame data and write to file
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD
                        .decode(&event.data)
                    {
                        let _ = std::fs::write(&filepath, &bytes);
                    }

                    // Acknowledge the frame to receive the next one
                    let ack = ScreencastFrameAckParams::new(event.session_id);
                    let _ = page_inner.execute(ack).await;
                }
                _ = cancel_rx.changed() => {
                    break;
                }
            }
        }
    });

    Response::ok_data(ResponseData::Text {
        text: format!("recording started → {out_dir}"),
    })
}

/// Stop video recording and return frame count.
pub(in crate::daemon) async fn cmd_record_stop(state: &Arc<Mutex<DaemonState>>) -> Response {
    use chromiumoxide::cdp::browser_protocol::page::StopScreencastParams;
    use std::sync::atomic::Ordering;

    let (dir, count) = {
        let mut guard = state.lock().await;
        let Some(dir) = guard.recording_dir.take() else {
            return Response::error("no recording in progress");
        };
        // Cancel listener
        if let Some(tx) = guard.recording_cancel.take() {
            let _ = tx.send(true);
        }
        let count = guard.recording_frame_count.load(Ordering::Relaxed);
        (dir, count)
    };

    // Stop CDP screencast
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let _ = page.inner().execute(StopScreencastParams::default()).await;

    Response::ok_data(ResponseData::Text {
        text: format!("recording stopped: {count} frames saved to {dir}"),
    })
}

pub(in crate::daemon) async fn cmd_screencast_stop(state: &Arc<Mutex<DaemonState>>) -> Response {
    use chromiumoxide::cdp::browser_protocol::page::StopScreencastParams;
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.inner().execute(StopScreencastParams::default()).await {
        Ok(_) => Response::ok_data(ResponseData::Text {
            text: "screencast stopped".to_owned(),
        }),
        Err(e) => Response::error(format!("screencast stop failed: {e}")),
    }
}
