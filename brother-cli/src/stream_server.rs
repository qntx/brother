//! `WebSocket` stream server for screencast frames and input injection.
//!
//! Starts a `WebSocket` server that:
//! - Pushes `CDP` screencast frames (base64 JPEG/PNG) to connected clients.
//! - Receives mouse/keyboard/touch input events from clients and injects
//!   them into the browser via `CDP`.
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
use tokio::sync::{Mutex, broadcast};
use tokio_tungstenite::tungstenite::Message;

use brother::{CdpKeyEventType, CdpMouseEventType, CdpTouchEventType, RawMouseEvent};

use crate::daemon::state::{DaemonState, get_page};

/// An input event received from a stream client.
#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum InputEvent {
    Mouse {
        #[serde(default = "default_mouse_moved")]
        event_type: CdpMouseEventType,
        #[serde(default)]
        x: f64,
        #[serde(default)]
        y: f64,
        #[serde(default)]
        button: Option<String>,
        #[serde(default)]
        click_count: Option<i64>,
        #[serde(default)]
        delta_x: Option<f64>,
        #[serde(default)]
        delta_y: Option<f64>,
        #[serde(default)]
        modifiers: Option<i64>,
    },
    Keyboard {
        #[serde(default = "default_key_down")]
        event_type: CdpKeyEventType,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        modifiers: Option<i64>,
    },
    Touch {
        #[serde(default = "default_touch_start")]
        event_type: CdpTouchEventType,
        #[serde(default)]
        points: Vec<[f64; 2]>,
        #[serde(default)]
        modifiers: Option<i64>,
    },
}

const fn default_mouse_moved() -> CdpMouseEventType { CdpMouseEventType::MouseMoved }
const fn default_key_down() -> CdpKeyEventType { CdpKeyEventType::KeyDown }
const fn default_touch_start() -> CdpTouchEventType { CdpTouchEventType::TouchStart }

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
    /// Allowed origins for `WebSocket` connections (empty = allow all).
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
) -> std::io::Result<(
    broadcast::Sender<ScreencastFrame>,
    tokio::task::JoinHandle<()>,
)> {
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
                        if let Ok(text) = serde_json::to_string(&msg)
                            && ws_tx.send(Message::Text(text.into())).await.is_err()
                        {
                            break;
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
    let Ok(event) = serde_json::from_str::<InputEvent>(text) else {
        return;
    };
    let Ok(page) = get_page(state).await else {
        return;
    };
    match event {
        InputEvent::Mouse {
            event_type,
            x,
            y,
            button,
            click_count,
            delta_x,
            delta_y,
            modifiers,
        } => {
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
        InputEvent::Keyboard {
            event_type,
            key,
            code,
            text,
            modifiers,
        } => {
            let _ = page
                .inject_key_event(
                    event_type,
                    key.as_deref(),
                    code.as_deref(),
                    text.as_deref(),
                    modifiers,
                )
                .await;
        }
        InputEvent::Touch {
            event_type,
            points,
            modifiers,
        } => {
            let pts: Vec<(f64, f64)> = points.iter().map(|p| (p[0], p[1])).collect();
            let _ = page
                .inject_touch_event(event_type, &pts, modifiers)
                .await;
        }
    }
}

/// `WebSocket` handshake callback that checks the `Origin` header.
struct OriginCheck(Arc<Vec<String>>);

impl tokio_tungstenite::tungstenite::handshake::server::Callback for OriginCheck {
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
            let mut resp = tokio_tungstenite::tungstenite::http::Response::new(Some(
                "origin not allowed".into(),
            ));
            *resp.status_mut() = tokio_tungstenite::tungstenite::http::StatusCode::FORBIDDEN;
            Err(resp)
        }
    }
}
