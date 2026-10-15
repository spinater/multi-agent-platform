//! `/api/env`: the Configure screen's Env & secrets (`env_vars.rs`). A secret's value is never returned.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use super::{ApiResult, not_found};
use crate::env_vars;
use crate::hub::Hub;
use crate::model::{EnvDelete, EnvList, EnvUpdate};

#[derive(Deserialize)]
pub struct EnvQuery {
    project: Option<String>,
}

fn scope(project: Option<String>) -> Option<String> {
    project.map(|p| p.trim().to_string()).filter(|p| !p.is_empty())
}

/// The machine's variables, merged with `project`'s when given (each with its source).
pub async fn list(State(hub): State<Arc<Hub>>, Query(q): Query<EnvQuery>) -> ApiResult<EnvList> {
    Ok(Json(env_vars::list(&hub, scope(q.project).as_deref()).await?))
}

/// Replaces one scope's list; returns the merged view of that scope.
pub async fn put(State(hub): State<Arc<Hub>>, Json(b): Json<EnvUpdate>) -> ApiResult<EnvList> {
    Ok(Json(env_vars::replace(&hub, scope(b.project).as_deref(), b.vars).await?))
}

/// Removes one variable from a scope (404 when the scope has none by that name).
pub async fn delete(State(hub): State<Arc<Hub>>, Json(b): Json<EnvDelete>) -> ApiResult<EnvList> {
    match env_vars::remove(&hub, scope(b.project).as_deref(), b.key.trim()).await? {
        Some(list) => Ok(Json(list)),
        None => Err(not_found("variable")),
    }
}
