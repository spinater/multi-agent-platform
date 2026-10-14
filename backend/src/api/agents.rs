//! `/api/agents*`: the Agents screen. `project` is a slug; empty or missing is `workspace.dir`. `scope` is
//! `project` (the default) or `user`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ApiResult, known_project, not_found};
use crate::agents;
use crate::hub::Hub;
use crate::model::{AgentDef, AgentDetail, AgentPlan, AgentScope, AgentsOverview, Level};

#[derive(Deserialize)]
pub struct AgentQuery {
    project: Option<String>,
    #[serde(default)]
    scope: AgentScope,
}

fn place(project: Option<&str>, scope: AgentScope) -> String {
    match scope {
        AgentScope::User => "your user folders".into(),
        AgentScope::Project => project.unwrap_or("the default workspace").into(),
    }
}

pub async fn overview(State(hub): State<Arc<Hub>>, Query(q): Query<AgentQuery>) -> ApiResult<AgentsOverview> {
    let project = known_project(&hub, q.project).await?;
    let (cfg, workspace) = (hub.config.agents.clone(), hub.work_dir(project.as_deref()).await);
    let mut overview = tokio::task::spawn_blocking(move || agents::overview(&cfg, &workspace)).await.map_err(anyhow::Error::from)?;
    // Only CLIs this server has configured.
    overview.readers.retain(|r| hub.config.providers.contains_key(&r.provider));
    overview.project = project;
    Ok(Json(overview))
}

pub async fn detail(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Query(q): Query<AgentQuery>) -> ApiResult<AgentDetail> {
    let project = known_project(&hub, q.project).await?;
    let (cfg, workspace) = (hub.config.agents.clone(), hub.work_dir(project.as_deref()).await);
    let found = tokio::task::spawn_blocking(move || agents::detail(&cfg, &workspace, q.scope, &name)).await.map_err(anyhow::Error::from)?;
    Ok(Json(found.ok_or_else(|| not_found("agent"))?))
}

#[derive(Deserialize)]
pub struct SaveBody {
    project: Option<String>,
    #[serde(default)]
    scope: AgentScope,
    def: AgentDef,
}

#[derive(Deserialize)]
pub struct SaveQuery {
    /// Only say what would be written (for the editor's live preview); validation problems come back as `error`.
    #[serde(default)]
    dry_run: bool,
}

/// Writes the agent's file for each target CLI and removes the files of the others; returns what it did.
pub async fn save(State(hub): State<Arc<Hub>>, Query(q): Query<SaveQuery>, Json(b): Json<SaveBody>) -> ApiResult<AgentPlan> {
    let project = known_project(&hub, b.project).await?;
    let (cfg, workspace) = (hub.config.agents.clone(), hub.work_dir(project.as_deref()).await);
    let (scope, def, dry_run) = (b.scope, b.def, q.dry_run);
    let name = def.name.clone();
    let plan = tokio::task::spawn_blocking(move || if dry_run { Ok(agents::preview(&cfg, &workspace, scope, def)) } else { agents::save(&cfg, &workspace, scope, def) })
        .await
        .map_err(anyhow::Error::from)??;
    if !dry_run {
        let (written, removed) = (plan.changes.iter().filter(|c| c.content.is_some()).count(), plan.changes.iter().filter(|c| c.content.is_none()).count());
        let msg = format!("Agent {name} saved in {}: {written} file(s) written, {removed} removed", place(project.as_deref(), scope));
        hub.event(Level::Info, "agent.saved", None, None, msg).await;
    }
    Ok(Json(plan))
}

pub async fn delete(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Query(q): Query<AgentQuery>) -> ApiResult<Value> {
    let project = known_project(&hub, q.project).await?;
    let (cfg, workspace) = (hub.config.agents.clone(), hub.work_dir(project.as_deref()).await);
    let (scope, agent) = (q.scope, name.clone());
    let removed = tokio::task::spawn_blocking(move || agents::delete(&cfg, &workspace, scope, &agent)).await.map_err(anyhow::Error::from)??.ok_or_else(|| not_found("agent"))?;
    hub.event(Level::Info, "agent.deleted", None, None, format!("Agent {name} deleted from {}: {}", place(project.as_deref(), scope), removed.join(", "))).await;
    Ok(Json(json!({ "ok": true, "removed": removed })))
}
