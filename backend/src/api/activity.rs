//! The activity graph (trace.rs): a project's task flow and what each agent used, and one session's raw trace.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use chrono::{DateTime, Utc};
use serde::Deserialize;

use super::{ApiResult, not_found};
use crate::hub::Hub;
use crate::model::{ActivityGraph, SessionTrace};

#[derive(Deserialize)]
pub struct ActivityQuery {
    /// Tasks changed since then, plus every open one; default: the last 24 hours.
    since: Option<DateTime<Utc>>,
    /// `true`: every task of the project, however old.
    #[serde(default)]
    all: bool,
    /// One task and its dependencies and dependents.
    task: Option<i64>,
}

pub async fn project(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Query(q): Query<ActivityQuery>) -> ApiResult<ActivityGraph> {
    hub.store.project(&slug).await?.ok_or_else(|| not_found("project"))?;
    let since = if q.all { None } else { Some(q.since.unwrap_or_else(|| Utc::now() - chrono::Duration::hours(24))) };
    Ok(Json(crate::trace::activity(&hub, &slug, since, q.task).await?))
}

pub async fn session(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>) -> ApiResult<SessionTrace> {
    let s = hub.store.session(&name).await?.ok_or_else(|| not_found("session"))?;
    Ok(Json(crate::trace::refresh(&hub, &s).await))
}
