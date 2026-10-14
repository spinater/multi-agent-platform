//! `/api/templates` and `/api/projects/{slug}/init*`: list templates, plan laying one into a project's folder, and
//! lay it down with the init agent queued as a task (`templates.rs`).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::{ApiError, ApiResult, not_found};
use crate::hub::Hub;
use crate::model::{InitPlan, InitStarted, TemplateInfo};
use crate::templates::{self, InitRequest};

pub async fn list(State(hub): State<Arc<Hub>>) -> ApiResult<Vec<TemplateInfo>> {
    Ok(Json(templates::list(&hub).into_iter().map(|t| t.info).collect()))
}

#[derive(Deserialize)]
pub struct PlanBody {
    template: String,
}

/// What each file would become (create, skip, append); writes nothing.
pub async fn plan(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<PlanBody>) -> ApiResult<InitPlan> {
    let project = hub.store.project(&slug).await?.ok_or_else(|| not_found("project"))?;
    let template = templates::find(&hub, &b.template)?;
    Ok(Json(templates::plan(&template, &hub.workspace_of(&project))?))
}

#[derive(Deserialize)]
pub struct InitBody {
    template: String,
    description: String,
    provider: String,
    model: Option<String>,
    #[serde(default)]
    set_verify: bool,
    /// The init agent also drafts the project's Reflex corpus from the description.
    #[serde(default = "super::projects::yes")]
    draft_corpus: bool,
}

pub async fn init(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<InitBody>) -> Result<(StatusCode, Json<InitStarted>), ApiError> {
    hub.store.project(&slug).await?.ok_or_else(|| not_found("project"))?;
    let req = InitRequest { template: b.template, description: b.description, provider: b.provider, model: b.model, set_verify: b.set_verify, draft_corpus: b.draft_corpus };
    Ok((StatusCode::CREATED, Json(templates::init(&hub, &slug, req).await?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_init_body_drafts_the_corpus_unless_told_not_to() {
        let b: InitBody = serde_json::from_str(r#"{"template": "team-process", "description": "x", "provider": "claude"}"#).unwrap();
        assert!(b.draft_corpus && !b.set_verify);
        let b: InitBody = serde_json::from_str(r#"{"template": "t", "description": "x", "provider": "claude", "draft_corpus": false}"#).unwrap();
        assert!(!b.draft_corpus);
    }
}
