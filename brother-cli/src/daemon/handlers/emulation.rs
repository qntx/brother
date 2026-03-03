//! Emulation handlers: device, device_list, screencast.

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
                    "user_agent": p.user_agent,
                })
            })
        })
        .collect();
    Response::ok_data(ResponseData::Eval {
        value: serde_json::Value::Array(descriptions),
    })
}

pub(in crate::daemon) async fn cmd_device(
    state: &Arc<Mutex<DaemonState>>,
    name: &str,
) -> Response {
    let Some(preset) = brother::DevicePreset::lookup(name) else {
        let names = brother::DevicePreset::list_names().join(", ");
        return Response::error(format!("unknown device '{name}'. Available: {names}"));
    };
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(e) = page.set_viewport(preset.width, preset.height).await {
        return Response::error(e.to_string());
    }
    if let Err(e) = page.set_user_agent(preset.user_agent).await {
        return Response::error(e.to_string());
    }
    Response::ok_data(ResponseData::Text {
        text: format!(
            "emulating {} ({}x{}, {})",
            preset.name, preset.width, preset.height, preset.user_agent
        ),
    })
}

pub(in crate::daemon) async fn cmd_screencast_start(
    state: &Arc<Mutex<DaemonState>>,
    format: &str,
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
    let fmt = if format == "png" {
        StartScreencastFormat::Png
    } else {
        StartScreencastFormat::Jpeg
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
