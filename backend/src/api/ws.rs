//! `/api/ws`: pushes every [`ServerMessage`] to the dashboard. Clients load `/api/overview` once, then apply these.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast::error::RecvError;

use crate::hub::Hub;
use crate::model::ServerMessage;

/// WebSockets are not covered by CORS, so any page could open one; only the dashboard's origin and the configured
/// ones may. Clients without an `Origin` (CLI tools) are not browsers and pass.
fn origin_allowed(hub: &Hub, headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get("origin").and_then(|v| v.to_str().ok()) else { return true };
    let host = headers.get("host").and_then(|v| v.to_str().ok()).unwrap_or("");
    let authority = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    authority == host || hub.config.server.allowed_origins.iter().any(|o| o == origin)
}

pub async fn handler(State(hub): State<Arc<Hub>>, headers: HeaderMap, upgrade: WebSocketUpgrade) -> Response {
    if !origin_allowed(&hub, &headers) {
        return (StatusCode::FORBIDDEN, "origin not allowed; add it to server.allowed_origins").into_response();
    }
    upgrade.on_upgrade(move |socket| serve(hub, socket))
}

async fn serve(hub: Arc<Hub>, mut socket: WebSocket) {
    let mut rx = hub.subscribe();
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                Ok(msg) => {
                    let Ok(text) = serde_json::to_string(&msg) else { continue };
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                // A slow client missed messages: tell it to reload the overview instead of drifting.
                Err(RecvError::Lagged(_)) => {
                    let text = serde_json::to_string(&ServerMessage::Resync).unwrap_or_default();
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(RecvError::Closed) => break,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                _ => {}
            },
        }
    }
}
