//! A fake Bot API on 127.0.0.1 for tests: records every call, answers from a script, else `{"ok": true}`
//! (getUpdates: an empty list after a short pause, like a long-poll that timed out).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

#[derive(Default)]
struct Inner {
    /// (token, method, body)
    calls: Vec<(String, String, Value)>,
    replies: HashMap<String, VecDeque<(u16, Value)>>,
}

#[derive(Clone)]
pub struct FakeTelegram {
    pub base: String,
    inner: Arc<Mutex<Inner>>,
}

async fn handle(State(inner): State<Arc<Mutex<Inner>>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    let body: Bytes = axum::body::to_bytes(req.into_body(), 1 << 20).await.unwrap_or_default();
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let rest = path.trim_start_matches('/');
    let (token, method) = rest.split_once('/').unwrap_or((rest, ""));
    let token = token.strip_prefix("bot").unwrap_or(token).to_string();
    let scripted = {
        let mut g = inner.lock().unwrap();
        g.calls.push((token, method.to_string(), body));
        g.replies.get_mut(method).and_then(VecDeque::pop_front)
    };
    let (status, reply) = match scripted {
        Some(r) => r,
        None if method == "getUpdates" => {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            (200, json!({ "ok": true, "result": [] }))
        }
        None => (200, json!({ "ok": true, "result": { "message_id": 1 } })),
    };
    (StatusCode::from_u16(status).unwrap(), axum::Json(reply)).into_response()
}

impl FakeTelegram {
    pub async fn start() -> Self {
        let inner = Arc::new(Mutex::new(Inner::default()));
        let app = Router::new().fallback(handle).with_state(inner.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { base, inner }
    }

    /// The next call of `method` gets this answer (queued in order).
    pub fn reply(&self, method: &str, status: u16, body: Value) {
        self.inner.lock().unwrap().replies.entry(method.into()).or_default().push_back((status, body));
    }

    /// Bodies of every `method` call so far.
    pub fn calls(&self, method: &str) -> Vec<Value> {
        self.inner.lock().unwrap().calls.iter().filter(|c| c.1 == method).map(|c| c.2.clone()).collect()
    }

    /// The tokens calls were made with.
    pub fn tokens(&self) -> Vec<String> {
        self.inner.lock().unwrap().calls.iter().map(|c| c.0.clone()).collect()
    }

    /// Texts of every sendMessage so far.
    pub fn texts(&self) -> Vec<String> {
        self.calls("sendMessage").iter().filter_map(|b| b.get("text").and_then(Value::as_str).map(String::from)).collect()
    }
}
