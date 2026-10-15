//! `/api/rules*` and `/api/hooks*`: the Configure screen's Rules and Hooks tabs. `project` is a slug; empty or
//! missing is the machine scope.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use super::{ApiResult, known_project};
use crate::hub::Hub;
use crate::model::{HookRun, HookTest, HooksOverview, HooksWrite, RuleDelete, RuleReaders, RuleWrite, RulesOverview};
use crate::{hooks, rules};

#[derive(Deserialize)]
pub struct ProjectQuery {
    project: Option<String>,
}

pub async fn list_rules(State(hub): State<Arc<Hub>>, Query(q): Query<ProjectQuery>) -> ApiResult<RulesOverview> {
    let project = known_project(&hub, q.project).await?;
    Ok(Json(rules::overview(&hub, project.as_deref()).await?))
}

pub async fn put_rule(State(hub): State<Arc<Hub>>, Json(b): Json<RuleWrite>) -> ApiResult<RulesOverview> {
    let project = known_project(&hub, b.project).await?;
    Ok(Json(rules::put(&hub, project.as_deref(), b.path, b.body).await?))
}

pub async fn delete_rule(State(hub): State<Arc<Hub>>, Json(b): Json<RuleDelete>) -> ApiResult<RulesOverview> {
    let project = known_project(&hub, b.project).await?;
    Ok(Json(rules::remove(&hub, project.as_deref(), b.path).await?))
}

pub async fn put_readers(State(hub): State<Arc<Hub>>, Json(b): Json<RuleReaders>) -> ApiResult<RulesOverview> {
    let project = known_project(&hub, b.project).await?;
    Ok(Json(rules::readers(&hub, project.as_deref(), b.path, b.readers).await?))
}

pub async fn list_hooks(State(hub): State<Arc<Hub>>, Query(q): Query<ProjectQuery>) -> ApiResult<HooksOverview> {
    let project = known_project(&hub, q.project).await?;
    Ok(Json(hooks::overview(&hub, project.as_deref()).await?))
}

pub async fn put_hooks(State(hub): State<Arc<Hub>>, Json(b): Json<HooksWrite>) -> ApiResult<HooksOverview> {
    let project = known_project(&hub, b.project).await?;
    Ok(Json(hooks::save(&hub, project.as_deref(), b.hooks).await?))
}

pub async fn test_hook(State(hub): State<Arc<Hub>>, Json(b): Json<HookTest>) -> ApiResult<HookRun> {
    let project = known_project(&hub, b.project).await?;
    Ok(Json(hooks::test(&hub, project.as_deref(), &b.command, b.timeout_secs).await?))
}

#[cfg(test)]
mod tests;
