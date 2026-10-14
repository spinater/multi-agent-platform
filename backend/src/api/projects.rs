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
use crate::model::{
    DecideResponse, Decision, FitResult, Project, ProjectCreated, ProjectDefaults, ProjectInfo, ProjectSettings, Question, ReflexStatus, RepoListing, SourceInspection, SourceKind,
};
use crate::projects::{self, NewProject, validate_fallback, validate_questions, validate_repo_path};
use crate::{project_settings, reflex, repo};
// Reflex screen.
use crate::model::{ArchiveResult, DecisionFeedback, DecisionSummary, ReflexCase};
use crate::reflex_screen;
// Reflex savings.
use crate::model::SavingsReport;
use crate::savings;

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
    /// Source, providers, verify, audit, paused; missing fields use the defaults.
    #[serde(default)]
    settings: ProjectSettings,
}

pub async fn create(State(hub): State<Arc<Hub>>, Json(b): Json<CreateBody>) -> Result<(StatusCode, Json<ProjectCreated>), ApiError> {
    let new = NewProject { name: b.name, slug: b.slug, description: b.description, fallback: b.fallback, model: b.model };
    let (p, checks) = projects::create_with(&hub, new, b.settings).await?;
    Ok((StatusCode::CREATED, Json(ProjectCreated { project: hub.project_info(p).await?, checks })))
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
    /// Replaces providers, verify, audit and paused; the source is kept (it records how the repo was made).
    settings: Option<ProjectSettings>,
    /// Pauses or resumes the project without sending the rest of its settings.
    paused: Option<bool>,
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
    if let Some(s) = b.settings {
        let s = ProjectSettings { source: p.settings.source.clone(), ..project_settings::normalize(s) };
        project_settings::validate(&hub, &s)?;
        p.settings = s;
    }
    if let Some(paused) = b.paused {
        p.settings.paused = paused;
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

// ---- project settings: defaults, source inspection, provider sign-in

pub async fn defaults(State(hub): State<Arc<Hub>>) -> ApiResult<ProjectDefaults> {
    Ok(Json(project_settings::defaults(&hub)))
}

#[derive(Deserialize)]
pub struct InspectBody {
    kind: SourceKind,
    location: String,
}

/// Looks at a project's folder before the project is made. Problems come back as `ok: false` with `error`, so the
/// wizard can show them next to the field.
pub async fn inspect(Json(b): Json<InspectBody>) -> ApiResult<SourceInspection> {
    let failed = |err: anyhow::Error| SourceInspection { error: Some(format!("{err:#}")), ..Default::default() };
    if let Err(err) = project_settings::validate_location(b.kind, Some(&b.location)) {
        return Ok(Json(failed(err)));
    }
    let path = crate::login::expand_home(b.location.trim());
    Ok(Json(match b.kind {
        SourceKind::Local if path.is_dir() => repo::inspect_folder(&path).await,
        SourceKind::Local => failed(anyhow::anyhow!("{} is not a folder on this machine", path.display())),
        SourceKind::Empty if !path.exists() => SourceInspection { ok: true, ..Default::default() },
        SourceKind::Empty if path.is_dir() && std::fs::read_dir(&path).is_ok_and(|mut d| d.next().is_none()) => SourceInspection { ok: true, ..Default::default() },
        SourceKind::Empty => failed(anyhow::anyhow!("{} already exists and is not an empty folder", path.display())),
        SourceKind::Git => failed(anyhow::anyhow!("the platform never clones code: clone the repo yourself, then add its folder")),
    }))
}

pub async fn login_start(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>) -> ApiResult<crate::model::LoginState> {
    crate::login::start(&hub, &name).await?;
    Ok(Json(crate::login::state(&hub, &name).await?))
}

pub async fn login_state(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>) -> ApiResult<crate::model::LoginState> {
    Ok(Json(crate::login::state(&hub, &name).await?))
}

// ---- reflex screen: calibration cases, decision feedback and summary, corpus archive

pub async fn cases(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<Vec<ReflexCase>> {
    load(&hub, &slug).await?;
    Ok(Json(hub.store.cases(&slug).await?))
}

#[derive(Deserialize)]
pub struct CaseBody {
    question_id: String,
    text: String,
    expected: Value,
}

#[derive(Deserialize)]
pub struct AddCasesBody {
    cases: Vec<CaseBody>,
}

pub async fn add_cases(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<AddCasesBody>) -> Result<(StatusCode, Json<Vec<ReflexCase>>), ApiError> {
    let p = load(&hub, &slug).await?;
    let new = b.cases.into_iter().map(|c| reflex_screen::NewCase { question_id: c.question_id, text: c.text, expected: c.expected }).collect();
    let added = reflex_screen::add_cases(&hub, &p, new).await?;
    hub.announce_project(&slug).await;
    Ok((StatusCode::CREATED, Json(added)))
}

pub async fn delete_case(State(hub): State<Arc<Hub>>, UrlPath((slug, id)): UrlPath<(String, i64)>) -> ApiResult<Value> {
    load(&hub, &slug).await?;
    if !hub.store.delete_case(&slug, id).await? {
        return Err(not_found("case"));
    }
    hub.announce_project(&slug).await;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct FitCasesBody {
    rho: Option<f64>,
    #[serde(default)]
    apply: bool,
}

pub async fn fit_cases(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<FitCasesBody>) -> ApiResult<FitResult> {
    let mut p = load(&hub, &slug).await?;
    let result = reflex_screen::fit(&hub, &mut p, b.rho.unwrap_or(0.3), b.apply).await?;
    hub.announce_project(&slug).await;
    Ok(Json(result))
}

#[derive(Deserialize)]
pub struct DecisionFeedbackBody {
    correct: bool,
    /// The right answer, required when `correct` is false.
    expected: Option<Value>,
    #[serde(default = "yes")]
    add_case: bool,
}

fn yes() -> bool {
    true
}

pub async fn decision_feedback(
    State(hub): State<Arc<Hub>>,
    UrlPath((slug, id)): UrlPath<(String, i64)>,
    Json(b): Json<DecisionFeedbackBody>,
) -> ApiResult<DecisionFeedback> {
    let p = load(&hub, &slug).await?;
    Ok(Json(reflex_screen::feedback(&hub, &p, id, b.correct, b.expected, b.add_case).await?))
}

#[derive(Deserialize)]
pub struct SummaryQuery {
    hours: Option<i64>,
}

pub async fn decision_summary(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Query(q): Query<SummaryQuery>) -> ApiResult<DecisionSummary> {
    load(&hub, &slug).await?;
    Ok(Json(hub.store.decision_summary(&slug, q.hours.unwrap_or(24), chrono::Utc::now()).await?))
}

#[derive(Deserialize)]
pub struct ArchiveBody {
    /// Domains to move; omitted: the smallest until 8 are left.
    domains: Option<Vec<String>>,
    #[serde(default)]
    restore: bool,
}

pub async fn archive(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<ArchiveBody>) -> ApiResult<ArchiveResult> {
    load(&hub, &slug).await?;
    Ok(Json(reflex_screen::archive(&hub, &slug, b.domains, b.restore).await?))
}

// ---- reflex savings: tokens Reflex saved against what the fallbacks spent (savings.rs)

#[derive(Deserialize)]
pub struct SavingsQuery {
    hours: Option<i64>,
    /// A project's slug; empty or absent: every project.
    project: Option<String>,
}

pub async fn savings(State(hub): State<Arc<Hub>>, Query(q): Query<SavingsQuery>) -> ApiResult<SavingsReport> {
    let project = q.project.filter(|p| !p.is_empty());
    if let Some(slug) = &project {
        load(&hub, slug).await?;
    }
    Ok(Json(savings::load(&hub, project.as_deref(), q.hours.unwrap_or(24), chrono::Utc::now()).await?))
}
