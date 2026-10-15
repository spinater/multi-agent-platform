//! `/api/lanes` (lanes.rs) and `/api/telegram` (telegram.rs). Owner token only, like every route outside a project
//! scope. The Telegram token is never returned.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ApiError, ApiResult, not_found};
use crate::hub::Hub;
use crate::lanes::{self, LanePatch};
use crate::model::{LaneImport, LaneInfo, LaneScreen, LaneTell, TelegramInfo, TellVerb};

pub async fn list(State(hub): State<Arc<Hub>>) -> ApiResult<Vec<LaneInfo>> {
    Ok(Json(hub.lanes.infos(&hub).await?))
}

pub async fn update(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Json(patch): Json<LanePatch>) -> ApiResult<LaneInfo> {
    if hub.lanes.config(&name).is_none() {
        return Err(not_found("lane"));
    }
    let cfg = hub.lanes.update(&hub, &name, patch).await?;
    Ok(Json(hub.lanes.info(&hub, &cfg, Utc::now()).await?))
}

#[derive(Deserialize)]
pub struct ScreenQuery {
    session: Option<String>,
    lines: Option<usize>,
}

pub async fn screen(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Query(q): Query<ScreenQuery>) -> ApiResult<LaneScreen> {
    if hub.lanes.config(&name).is_none() {
        return Err(not_found("lane"));
    }
    Ok(Json(hub.lanes.screen(&name, q.session.as_deref().filter(|s| !s.is_empty()), q.lines.unwrap_or(60))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TellBody {
    verb: TellVerb,
    note: Option<String>,
    mins: Option<u32>,
    session: Option<String>,
}

/// `{name}` is a lane, or a tmux session a lane watches (what `deploy/map-tell` sends from inside the session).
pub async fn tell(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Json(b): Json<TellBody>) -> ApiResult<LaneTell> {
    if hub.lanes.resolve(&name).is_none() {
        return Err(ApiError(StatusCode::NOT_FOUND, format!("no lane watches `{name}`")));
    }
    Ok(Json(lanes::tell(&hub, &name, b.verb, b.note.as_deref(), b.mins, b.session.as_deref(), Utc::now()).await?))
}

fn ttl() -> chrono::Duration {
    chrono::Duration::minutes(lanes::TELL_TTL_MINS)
}

pub async fn tells(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>) -> ApiResult<Vec<LaneTell>> {
    let (cfg, session) = hub.lanes.resolve(&name).ok_or_else(|| not_found("lane"))?;
    let now = Utc::now();
    let all = hub.store.lane_tells(Some(&cfg.name), now - ttl(), ttl()).await?;
    Ok(Json(all.into_iter().filter(|t| session.as_ref().is_none_or(|s| t.session.is_empty() || &t.session == s)).collect()))
}

pub async fn all_tells(State(hub): State<Arc<Hub>>) -> ApiResult<Vec<LaneTell>> {
    let now = Utc::now();
    Ok(Json(hub.store.lane_tells(None, now - ttl(), ttl()).await?))
}

#[derive(Deserialize)]
pub struct CancelQuery {
    session: Option<String>,
}

pub async fn cancel_tell(State(hub): State<Arc<Hub>>, UrlPath((name, verb)): UrlPath<(String, String)>, Query(q): Query<CancelQuery>) -> ApiResult<Value> {
    let (cfg, from_name) = hub.lanes.resolve(&name).ok_or_else(|| not_found("lane"))?;
    let verb: TellVerb = serde_json::from_value(Value::String(verb.clone())).map_err(|_| ApiError(StatusCode::BAD_REQUEST, format!("unknown verb `{verb}`")))?;
    let session = q.session.or(from_name).unwrap_or_default();
    let ok = hub.store.delete_lane_tell(&cfg.name, &session, verb, None).await?;
    Ok(Json(json!({ "ok": ok })))
}

#[derive(Deserialize)]
pub struct ImportBody {
    path: String,
}

pub async fn import_watcher(State(hub): State<Arc<Hub>>, Json(b): Json<ImportBody>) -> ApiResult<LaneImport> {
    Ok(Json(lanes::import_watcher(&hub, &b.path).await?))
}

pub async fn telegram(State(hub): State<Arc<Hub>>) -> ApiResult<TelegramInfo> {
    Ok(Json(hub.telegram.info()))
}

pub async fn telegram_test(State(hub): State<Arc<Hub>>) -> ApiResult<Value> {
    let text = format!("🧪 MAP test message · {}", chrono::Local::now().format("%d/%m/%Y %H:%M"));
    Ok(Json(match hub.telegram.send(&text).await {
        Ok(()) => json!({ "ok": true, "message": format!("Sent to chat {}", hub.telegram.chat_id()) }),
        Err(e) => json!({ "ok": false, "message": e }),
    }))
}
