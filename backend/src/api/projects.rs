//! `/api/projects/*`: projects, their repos, their Reflex engine and the decision gate.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};

use super::{ApiError, ApiResult, not_found};
use crate::decide::{self, Route};
use crate::hub::Hub;
use crate::model::{DecideResponse, Decision, FitResult, Project, ProjectInfo, Question, ReflexStatus, RepoListing};
use crate::projects::{self, NewProject, validate_fallback, validate_questions, validate_repo_path};
use crate::{reflex, repo};

async fn load(hub: &Hub, slug: &str) -> Result<Project, ApiError> {
    hub.store.project(slug).await?.ok_or_else(|| not_found("project"))
}

pub async fn list(State(hub): State<Arc<Hub>>) -> ApiResult<Vec<ProjectInfo>> {
    Ok(Json(hub.projects().await?))
}

#[derive(Deserialize)]
pub struct CreateBody {
    name: String,
    slug: Option<String>,
    #[serde(default)]
    description: String,
    fallback: Option<String>,
    model: Option<String>,
}

pub async fn create(State(hub): State<Arc<Hub>>, Json(b): Json<CreateBody>) -> Result<(StatusCode, Json<ProjectInfo>), ApiError> {
    let p = projects::create(&hub, NewProject { name: b.name, slug: b.slug, description: b.description, fallback: b.fallback, model: b.model }).await?;
    Ok((StatusCode::CREATED, Json(hub.project_info(p).await?)))
}

pub async fn get(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<ProjectInfo> {
    let p = load(&hub, &slug).await?;
    Ok(Json(hub.project_info(p).await?))
}

/// Tells a missing field (keep) from an explicit `null` (clear).
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
pub struct UpdateBody {
    name: Option<String>,
    description: Option<String>,
    #[serde(default, deserialize_with = "present")]
    threshold: Option<Option<f64>>,
    fallback: Option<String>,
    #[serde(default, deserialize_with = "present")]
    model: Option<Option<String>>,
    questions: Option<Vec<Question>>,
}

pub async fn update(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<UpdateBody>) -> ApiResult<ProjectInfo> {
    let mut p = load(&hub, &slug).await?;
    if let Some(name) = b.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()) {
        p.name = name;
    }
    if let Some(d) = b.description {
        p.description = d.trim().to_string();
    }
    if let Some(t) = b.threshold {
        if t.is_some_and(|t| !t.is_finite() || t < 0.0) {
            return Err(ApiError(StatusCode::BAD_REQUEST, "threshold is a number of at least 0, or null".into()));
        }
        p.threshold = t;
    }
    if let Some(f) = b.fallback {
        validate_fallback(&hub, &f)?;
        p.fallback = f;
    }
    if let Some(m) = b.model {
        p.model = m.filter(|m| !m.trim().is_empty());
    }
    if let Some(q) = b.questions {
        validate_questions(&q)?;
        p.questions = q;
    }
    hub.put_project(&p).await?;
    Ok(Json(hub.project_info(p).await?))
}

pub async fn delete(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<Value> {
    load(&hub, &slug).await?;
    projects::delete(&hub, &slug).await?;
    Ok(Json(json!({ "ok": true })))
}

fn repo_dir(hub: &Hub, slug: &str, name: &str) -> Result<std::path::PathBuf, ApiError> {
    match name {
        "code" => Ok(hub.code_repo(slug)),
        "corpus" => Ok(hub.corpus_repo(slug)),
        _ => Err(not_found("repo")),
    }
}

pub async fn repo_listing(State(hub): State<Arc<Hub>>, UrlPath((slug, name)): UrlPath<(String, String)>) -> ApiResult<RepoListing> {
    load(&hub, &slug).await?;
    let dir = repo_dir(&hub, &slug, &name)?;
    Ok(Json(RepoListing { files: repo::files(&dir).await?, commits: repo::log(&dir, 30).await? }))
}

#[derive(Deserialize)]
pub struct FileQuery {
    path: String,
}

pub async fn read_file(State(hub): State<Arc<Hub>>, UrlPath((slug, name)): UrlPath<(String, String)>, Query(q): Query<FileQuery>) -> ApiResult<Value> {
    load(&hub, &slug).await?;
    validate_repo_path(&q.path)?;
    let content = repo::read(&repo_dir(&hub, &slug, &name)?, &q.path).await.ok_or_else(|| not_found("file"))?;
    Ok(Json(json!({ "path": q.path, "content": content })))
}

#[derive(Deserialize)]
pub struct WriteBody {
    path: String,
    /// `null` deletes the file.
    content: Option<String>,
    #[serde(default)]
    message: String,
}

pub async fn write_file(State(hub): State<Arc<Hub>>, UrlPath((slug, name)): UrlPath<(String, String)>, Json(b): Json<WriteBody>) -> ApiResult<Value> {
    load(&hub, &slug).await?;
    repo_dir(&hub, &slug, &name)?;
    let commit = projects::write_file(&hub, &slug, &name, &b.path, b.content, &b.message).await?;
    Ok(Json(json!({ "commit": commit })))
}

pub async fn restart(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<ReflexStatus> {
    load(&hub, &slug).await?;
    Ok(Json(hub.restart_reflex(&slug).await?))
}

pub async fn reflex_log(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<Value> {
    load(&hub, &slug).await?;
    Ok(Json(json!({ "log": reflex::tail(&hub.reflex_log(&slug), 200).await })))
}

#[derive(Deserialize)]
pub struct DecideBody {
    state: String,
    questions: Vec<Question>,
    #[serde(default)]
    route: Route,
}

pub async fn decide(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<DecideBody>) -> ApiResult<DecideResponse> {
    let p = load(&hub, &slug).await?;
    Ok(Json(decide::decide(&hub, &p, &b.state, &b.questions, b.route).await?))
}

#[derive(Deserialize)]
pub struct FeedbackBody {
    /// The confidence Reflex returned for that answer; never an invented number.
    p: f64,
    outcome: bool,
}

pub async fn feedback(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<FeedbackBody>) -> ApiResult<Value> {
    let p = load(&hub, &slug).await?;
    Ok(Json(hub.reflex.post(p.port, "/feedback", &json!({ "p": b.p, "outcome": b.outcome })).await?))
}

#[derive(Deserialize)]
pub struct FitCase {
    state: String,
    question: Question,
    expected: Value,
}

#[derive(Deserialize)]
pub struct FitBody {
    cases: Vec<FitCase>,
    rho: Option<f64>,
    #[serde(default)]
    apply: bool,
}

pub async fn fit(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<FitBody>) -> ApiResult<FitResult> {
    let mut p = load(&hub, &slug).await?;
    let cases: Vec<(String, Question, Value)> = b.cases.into_iter().map(|c| (c.state, c.question, c.expected)).collect();
    let mut result = decide::fit(&hub, &p, &cases, b.rho.unwrap_or(0.3)).await?;
    if b.apply {
        p.threshold = result.threshold;
        hub.put_project(&p).await?;
        result.applied = true;
    }
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct LimitQuery {
    limit: Option<i64>,
}

pub async fn decisions(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Query(q): Query<LimitQuery>) -> ApiResult<Vec<Decision>> {
    load(&hub, &slug).await?;
    Ok(Json(hub.store.decisions(&slug, q.limit.unwrap_or(200).clamp(1, 1000)).await?))
}
