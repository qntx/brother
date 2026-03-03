//! WebSocket stream server for screencast frames and input injection.
//!
//! Starts a WebSocket server that:
//! - Pushes CDP screencast frames (base64 JPEG/PNG) to connected clients.
//! - Receives mouse/keyboard/touch input events from clients and injects
//!   them into the browser via CDP.
//!
//! Message protocol (JSON):
//!
//! **Server → Client:**
//! ```json
//! { "type": "frame", "data": "<base64>", "timestamp": 1234567890 }
//! ```
//!
//! **Client → Server:**
//! ```json
//! { "type": "mouse", "eventType": "mousePressed", "x": 100, "y": 200, "button": "left" }
//! { "type": "keyboard", "eventType": "keyDown", "key": "Enter" }
//! { "type": "touch", "eventType": "touchStart", "points": [[100, 200]] }
//! ```

use std::net::SocketAddr;
use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, Mutex};
use tokio_tungstenite::tungstenite::Message;

use brother::RawMouseEvent;

use crate::daemon::state::{DaemonState, get_page};

/// A screencast frame ready for broadcast.
#[derive(Debug, Clone)]
pub struct ScreencastFrame {
    /// Base64-encoded image data.
    pub data: String,
    /// Timestamp in milliseconds.
    pub timestamp: u64,
}

/// Configuration for the stream server.
#[derive(Debug)]
pub struct StreamServerConfig {
    /// Bind address (e.g. `127.0.0.1:9223`).
    pub addr: SocketAddr,
    /// Allowed origins for WebSocket connections (empty = allow all).
    pub allowed_origins: Vec<String>,
}

impl Default for StreamServerConfig {
    fn default() -> Self {
        Self {
            addr: SocketAddr::from(([127, 0, 0, 1], 9223)),
            allowed_origins: Vec::new(),
        }
    }
}

/// Start the stream server in a background task.
///
/// Returns a broadcast sender for pushing frames and a join handle.
pub async fn start(
    config: StreamServerConfig,
    state: Arc<Mutex<DaemonState>>,
) -> std::io::Result<(broadcast::Sender<ScreencastFrame>, tokio::task::JoinHandle<()>)> {
    let listener = TcpListener::bind(config.addr).await?;
    let (tx, _) = broadcast::channel::<ScreencastFrame>(16);
    let tx_clone = tx.clone();
    let allowed_origins = Arc::new(config.allowed_origins);

    tracing::info!("stream server listening on ws://{}", config.addr);

    let handle = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    tracing::debug!("stream client connected: {peer}");
                    let rx = tx_clone.subscribe();
                    let state = Arc::clone(&state);
                    let origins = Arc::clone(&allowed_origins);
                    tokio::spawn(handle_connection(stream, rx, state, origins));
                }
                Err(e) => {
                    tracing::warn!("stream accept error: {e}");
                }
            }
        }
    });

    Ok((tx, handle))
}

async fn handle_connection(
    stream: TcpStream,
    mut frame_rx: broadcast::Receiver<ScreencastFrame>,
    state: Arc<Mutex<DaemonState>>,
    allowed_origins: Arc<Vec<String>>,
) {
    let ws = match tokio_tungstenite::accept_hdr_async(
        stream,
        OriginCheck(Arc::clone(&allowed_origins)),
    )
    .await
    {
        Ok(ws) => ws,
        Err(e) => {
            tracing::debug!("websocket handshake failed: {e}");
            return;
        }
    };

    let (mut ws_tx, mut ws_rx) = ws.split();

    loop {
        tokio::select! {
            frame = frame_rx.recv() => {
                match frame {
                    Ok(f) => {
                        let msg = serde_json::json!({
                            "type": "frame",
                            "data": f.data,
                            "timestamp": f.timestamp,
                        });
                        if let Ok(text) = serde_json::to_string(&msg) {
                            if ws_tx.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        tracing::debug!("stream client lagged {n} frames");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            msg = ws_rx.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        handle_input_message(&text, &state).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    _ => {}
                }
            }
        }
    }
}

async fn handle_input_message(text: &str, state: &Arc<Mutex<DaemonState>>) {
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(text) else {
        return;
    };
    let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(_) => return,
    };

    match msg_type {
        "mouse" => {
            let event_type = msg
                .get("eventType")
                .and_then(|v| v.as_str())
                .unwrap_or("mouseMoved");
            let x = msg.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = msg.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let button = msg.get("button").and_then(|v| v.as_str());
            let click_count = msg.get("clickCount").and_then(|v| v.as_i64());
            let delta_x = msg.get("deltaX").and_then(|v| v.as_f64());
            let delta_y = msg.get("deltaY").and_then(|v| v.as_f64());
            let modifiers = msg.get("modifiers").and_then(|v| v.as_i64());
            let _ = page
                .inject_mouse_event(RawMouseEvent {
                    event_type,
                    x,
                    y,
                    button,
                    click_count,
                    delta_x,
                    delta_y,
                    modifiers,
                })
                .await;
        }
        "keyboard" => {
            let event_type = msg
                .get("eventType")
                .and_then(|v| v.as_str())
                .unwrap_or("keyDown");
            let key = msg.get("key").and_then(|v| v.as_str());
            let code = msg.get("code").and_then(|v| v.as_str());
            let text = msg.get("text").and_then(|v| v.as_str());
            let modifiers = msg.get("modifiers").and_then(|v| v.as_i64());
            let _ = page
                .inject_key_event(event_type, key, code, text, modifiers)
                .await;
        }
        "touch" => {
            let event_type = msg
                .get("eventType")
                .and_then(|v| v.as_str())
                .unwrap_or("touchStart");
            let points: Vec<(f64, f64)> = msg
                .get("points")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|p| {
                            let a = p.as_array()?;
                            Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
                        })
                        .collect()
                })
                .unwrap_or_default();
            let modifiers = msg.get("modifiers").and_then(|v| v.as_i64());
            let _ = page
                .inject_touch_event(event_type, &points, modifiers)
                .await;
        }
        _ => {}
    }
}

/// WebSocket handshake callback that checks the Origin header.
struct OriginCheck(Arc<Vec<String>>);

impl tokio_tungstenite::tungstenite::handshake::server::Callback
    for OriginCheck
{
    fn on_request(
        self,
        request: &tokio_tungstenite::tungstenite::http::Request<()>,
        response: tokio_tungstenite::tungstenite::http::Response<()>,
    ) -> Result<
        tokio_tungstenite::tungstenite::http::Response<()>,
        tokio_tungstenite::tungstenite::http::Response<Option<String>>,
    > {
        if self.0.is_empty() {
            return Ok(response);
        }
        let origin = request
            .headers()
            .get("origin")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if self.0.iter().any(|allowed| allowed == origin) {
            Ok(response)
        } else {
            let mut resp =
                tokio_tungstenite::tungstenite::http::Response::new(Some("origin not allowed".into()));
            *resp.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
            Err(resp)
        }
    }
}
