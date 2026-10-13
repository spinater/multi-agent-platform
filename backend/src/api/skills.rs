//! `/api/skills*`: the Skills screen. `project` is a slug; empty or missing is `workspace.dir`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use super::{ApiResult, known_project};
use crate::hub::Hub;
use crate::model::{Level, SkillFile, SkillsOverview};
use crate::skills;

#[derive(Deserialize)]
pub struct ProjectQuery {
    project: Option<String>,
}

pub async fn overview(State(hub): State<Arc<Hub>>, Query(q): Query<ProjectQuery>) -> ApiResult<SkillsOverview> {
    let project = known_project(&hub, q.project).await?;
    Ok(Json(skills::overview(&hub, project.as_deref()).await?))
}

#[derive(Deserialize)]
pub struct EnabledBody {
    project: Option<String>,
    /// The skill's `id` (its SKILL.md path, as the overview returned it).
    skill: String,
    enabled: bool,
}

/// Writes the workspace's `.claude/settings.local.json`, then returns the fresh overview.
pub async fn set_enabled(State(hub): State<Arc<Hub>>, Json(b): Json<EnabledBody>) -> ApiResult<SkillsOverview> {
    let project = known_project(&hub, b.project).await?;
    let (cfg, workspace) = (hub.config.skills.clone(), hub.work_dir(project.as_deref()).await);
    let id = b.skill.clone();
    let invocation = tokio::task::spawn_blocking(move || skills::toggle(&cfg, &workspace, &id, b.enabled)).await.map_err(anyhow::Error::from)??;
    let word = if b.enabled { "enabled" } else { "disabled" };
    let place = project.as_deref().unwrap_or("the default workspace");
    hub.event(Level::Info, "skill.toggled", None, None, format!("Skill /{invocation} {word} for Claude in {place}")).await;
    Ok(Json(skills::overview(&hub, project.as_deref()).await?))
}

#[derive(Deserialize)]
pub struct FileQuery {
    project: Option<String>,
    /// A skill `id` from the overview; any other path is refused.
    path: String,
}

pub async fn file(State(hub): State<Arc<Hub>>, Query(q): Query<FileQuery>) -> ApiResult<SkillFile> {
    let project = known_project(&hub, q.project).await?;
    let (cfg, workspace) = (hub.config.skills.clone(), hub.work_dir(project.as_deref()).await);
    let file = tokio::task::spawn_blocking(move || skills::read_file(&cfg, &workspace, &q.path)).await.map_err(anyhow::Error::from)??;
    Ok(Json(file))
}
