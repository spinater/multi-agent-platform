//! HTTP API under `/api` and the dashboard's static files.

mod git;
mod projects;
mod ws;

use std::sync::Arc;

use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::extract::DefaultBodyLimit;
use axum::routing::{any, get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::services::{ServeDir, ServeFile};

use crate::detect::{EndDecision, strip_ansi};
use crate::hub::{Hub, LaunchRequest};
use crate::model::{Event, Level, Overview, Role, Session, Task};
use crate::orchestrator::Orchestrator;
use crate::store::NewTask;

pub struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({ "error": self.1 }))).into_response()
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(err: anyhow::Error) -> Self {
        ApiError(StatusCode::BAD_REQUEST, format!("{err:#}"))
    }
}

type ApiResult<T> = Result<Json<T>, ApiError>;

fn not_found(what: &str) -> ApiError {
    ApiError(StatusCode::NOT_FOUND, format!("{what} not found"))
}

pub fn router(hub: Arc<Hub>) -> Router {
    let api = Router::new()
        .route("/health", get(|| async { Json(json!({ "ok": true })) }))
        .route("/overview", get(overview))
        .route("/events", get(events))
        .route("/providers", get(providers))
        .route("/sessions", get(sessions).post(create_session))
        .route("/sessions/{name}", axum::routing::delete(delete_session))
        .route("/sessions/{name}/pane", get(pane))
        .route("/sessions/{name}/prompt", post(prompt))
        .route("/sessions/{name}/key", post(key))
        .route("/tasks", get(tasks).post(create_task))
        .route("/tasks/{id}", axum::routing::patch(update_task).delete(delete_task))
        .route("/orchestrator", get(orchestrator).post(set_orchestrator))
        .route("/projects", get(projects::list).post(projects::create))
        .route("/projects/{slug}", get(projects::get).patch(projects::update).delete(projects::delete))
        .route("/projects/{slug}/repos/{repo}", get(projects::repo_listing))
        .route("/projects/{slug}/repos/{repo}/file", get(projects::read_file).put(projects::write_file))
        .route("/projects/{slug}/reflex/restart", post(projects::restart))
        .route("/projects/{slug}/reflex/log", get(projects::reflex_log))
        .route("/projects/{slug}/decide", post(projects::decide))
        .route("/projects/{slug}/feedback", post(projects::feedback))
        .route("/projects/{slug}/fit", post(projects::fit))
        .route("/projects/{slug}/decisions", get(projects::decisions))
        .route("/ws", get(ws::handler))
        .layer(middleware::from_fn_with_state(hub.clone(), require_token))
        .with_state(hub.clone());

    // Pushes carry whole packfiles, far above axum's default 2 MB body limit.
    let git = Router::new().route("/git/{slug}/{*rest}", any(git::handler)).layer(DefaultBodyLimit::max(1 << 30)).with_state(hub.clone());
    let mut app = Router::new().nest("/api", api).merge(git);
    let static_dir = &hub.config.server.static_dir;
    if static_dir.join("index.html").exists() {
        app = app.fallback_service(ServeDir::new(static_dir).fallback(ServeFile::new(static_dir.join("index.html"))));
    } else {
        tracing::warn!("no dashboard at {}; serving the API only (build it with `bun run build` in frontend/)", static_dir.display());
    }
    app.layer(middleware::from_fn_with_state(hub, require_known_host)).layer(tower_http::trace::TraceLayer::new_for_http())
}

/// The host part of a `Host` header or origin authority: `[::1]:4400` → `::1`, `localhost:3000` → `localhost`.
pub(crate) fn host_name(authority: &str) -> &str {
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    authority.rsplit_once(':').map_or(authority, |(h, _)| h)
}

/// Rejects requests whose `Host` is not loopback, the bound address or a configured name, so a hostile page that
/// rebinds its domain to 127.0.0.1 cannot drive the API from the browser.
async fn require_known_host(State(hub): State<Arc<Hub>>, headers: HeaderMap, request: Request, next: Next) -> Response {
    let host = headers.get("host").and_then(|v| v.to_str().ok()).map(host_name).unwrap_or("");
    let bound = host_name(&hub.config.server.bind);
    let known = matches!(host, "localhost" | "127.0.0.1" | "::1") || host == bound || hub.config.server.allowed_hosts.iter().any(|h| h == host);
    if known {
        next.run(request).await
    } else {
        ApiError(StatusCode::MISDIRECTED_REQUEST, format!("unknown host `{host}`; add it to server.allowed_hosts")).into_response()
    }
}

/// Length-independent comparison so the token cannot be guessed byte by byte from response times.
pub(crate) fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn require_token(State(hub): State<Arc<Hub>>, headers: HeaderMap, request: Request, next: Next) -> Response {
    let Some(expected) = hub.config.server.api_token.as_deref() else {
        return next.run(request).await;
    };
    let bearer = headers.get("authorization").and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer "));
    // Browsers cannot set headers on a WebSocket, so it may carry the token in the query string instead.
    let query = request
        .uri()
        .query()
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("token=")))
        .map(String::from);
    if bearer.is_some_and(|t| same(t, expected)) || query.is_some_and(|t| same(&t, expected)) {
        return next.run(request).await;
    }
    ApiError(StatusCode::UNAUTHORIZED, "missing or wrong API token".into()).into_response()
}

async fn overview(State(hub): State<Arc<Hub>>) -> ApiResult<Overview> {
    Ok(Json(Overview {
        sessions: hub.store.sessions().await?,
        tasks: hub.store.tasks().await?,
        providers: hub.providers(),
        orchestrator: hub.orchestrator_status().await,
        projects: hub.projects().await?,
        server: hub.server_info(),
    }))
}

#[derive(Deserialize)]
struct EventsQuery {
    limit: Option<i64>,
    session: Option<String>,
}

async fn events(State(hub): State<Arc<Hub>>, Query(q): Query<EventsQuery>) -> ApiResult<Vec<Event>> {
    Ok(Json(hub.store.events(q.limit.unwrap_or(100).clamp(1, 1000), q.session.as_deref()).await?))
}

async fn providers(State(hub): State<Arc<Hub>>) -> ApiResult<Value> {
    Ok(Json(json!(hub.providers())))
}

async fn sessions(State(hub): State<Arc<Hub>>) -> ApiResult<Vec<Session>> {
    Ok(Json(hub.store.sessions().await?))
}

#[derive(Deserialize)]
struct CreateSession {
    provider: String,
    model: Option<String>,
    name: Option<String>,
    prompt: Option<String>,
    #[serde(default)]
    supervisor: bool,
    project: Option<String>,
}

async fn create_session(State(hub): State<Arc<Hub>>, Json(body): Json<CreateSession>) -> ApiResult<Session> {
    let role = if body.supervisor { Role::Supervisor } else { Role::Manual };
    let prompt = body.prompt.filter(|p| !p.trim().is_empty());
    let project = known_project(&hub, body.project).await?;
    let session = hub.launch(LaunchRequest { name: body.name, provider: body.provider, model: body.model, role, task_id: None, project, prompt }).await?;
    Ok(Json(session))
}

#[derive(Deserialize)]
struct DeleteQuery {
    #[serde(default)]
    force: bool,
}

/// Live session: end it (refused while a person is using it unless `force`). Exited session: forget it.
async fn delete_session(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Query(q): Query<DeleteQuery>) -> ApiResult<Value> {
    let session = hub.store.session(&name).await?.ok_or_else(|| not_found("session"))?;
    if !session.state.is_live() {
        hub.forget_session(&name).await?;
        return Ok(Json(json!({ "ok": true, "message": format!("Removed {name}") })));
    }
    match hub.close_session(&name, q.force, "Closed from the dashboard").await? {
        EndDecision::End => Ok(Json(json!({ "ok": true, "message": format!("Closed {name}") }))),
        EndDecision::KeepAttached => Err(ApiError(StatusCode::CONFLICT, "A terminal is attached to this session; close it anyway with force".into())),
        EndDecision::KeepInput => Err(ApiError(StatusCode::CONFLICT, "Someone has typed on the input line; close it anyway with force".into())),
    }
}

#[derive(Deserialize)]
struct PaneQuery {
    lines: Option<usize>,
}

async fn pane(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Query(q): Query<PaneQuery>) -> ApiResult<Value> {
    let session = hub.store.session(&name).await?.ok_or_else(|| not_found("session"))?;
    let text = if session.state.is_live() && hub.term.exists(&name).await? {
        strip_ansi(&hub.term.capture(&name, q.lines.unwrap_or(300).clamp(10, 5000), false).await?)
    } else {
        std::fs::read_to_string(hub.run_dir(&name).join("final-pane.txt")).unwrap_or_default()
    };
    Ok(Json(json!({ "name": name, "text": text })))
}

#[derive(Deserialize)]
struct PromptBody {
    text: String,
}

async fn prompt(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Json(body): Json<PromptBody>) -> ApiResult<Value> {
    if body.text.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "empty prompt".into()));
    }
    hub.send_prompt(&name, &body.text).await?;
    let preview: String = body.text.chars().take(60).collect();
    hub.event(Level::Info, "session.prompt", Some(&name), None, format!("Sent: {preview}")).await;
    Ok(Json(json!({ "ok": true })))
}

/// Only keys an operator needs to steer a TUI; anything else would let the API run arbitrary key sequences.
const KEYS: &[&str] = &["C-c", "Escape", "Enter", "Up", "Down", "Tab", "BTab", "y", "n"];

#[derive(Deserialize)]
struct KeyBody {
    key: String,
}

async fn key(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Json(body): Json<KeyBody>) -> ApiResult<Value> {
    if !KEYS.contains(&body.key.as_str()) {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("key must be one of {}", KEYS.join(", "))));
    }
    hub.managed(&name).await?;
    hub.term.send_key(&name, &body.key).await?;
    Ok(Json(json!({ "ok": true })))
}

async fn tasks(State(hub): State<Arc<Hub>>) -> ApiResult<Vec<Task>> {
    Ok(Json(hub.store.tasks().await?))
}

#[derive(Deserialize)]
struct CreateTask {
    title: String,
    prompt: String,
    #[serde(default)]
    depends_on: Vec<i64>,
    provider: Option<String>,
    model: Option<String>,
    project: Option<String>,
}

/// An empty project means `workspace.dir`; a named one must exist.
async fn known_project(hub: &Hub, project: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(slug) = project.filter(|p| !p.is_empty()) else { return Ok(None) };
    hub.store.project(&slug).await?.ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, format!("project `{slug}` does not exist")))?;
    Ok(Some(slug))
}

async fn create_task(State(hub): State<Arc<Hub>>, Json(body): Json<CreateTask>) -> ApiResult<Task> {
    if body.title.trim().is_empty() || body.prompt.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "a task needs a title and a prompt".into()));
    }
    let provider = body.provider.filter(|p| !p.is_empty());
    if let Some(p) = &provider {
        hub.spec(p)?;
    }
    let project = known_project(&hub, body.project).await?;
    for dep in &body.depends_on {
        hub.store.task(*dep).await?.ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, format!("dependency #{dep} does not exist")))?;
    }
    let task = hub
        .store
        .create_task(NewTask { title: body.title.trim().into(), prompt: body.prompt, depends_on: body.depends_on, provider, model: body.model.filter(|m| !m.is_empty()), project })
        .await?;
    hub.announce_task(&task);
    hub.event(Level::Info, "task.created", None, Some(task.id), format!("Task #{} created: {}", task.id, task.title)).await;
    Ok(Json(task))
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum TaskAction {
    Retry,
    Cancel,
}

#[derive(Deserialize)]
struct UpdateTask {
    action: TaskAction,
}

async fn update_task(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<i64>, Json(body): Json<UpdateTask>) -> ApiResult<Task> {
    let task = match body.action {
        TaskAction::Retry => Orchestrator::retry(&hub, id).await?,
        TaskAction::Cancel => Orchestrator::cancel(&hub, id).await?,
    };
    Ok(Json(task))
}

async fn delete_task(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<i64>) -> ApiResult<Value> {
    let task = hub.store.task(id).await?.ok_or_else(|| not_found("task"))?;
    if !task.status.is_terminal() && task.status != crate::model::TaskStatus::Pending {
        return Err(ApiError(StatusCode::CONFLICT, "cancel a running task before deleting it".into()));
    }
    hub.store.delete_task(id).await?;
    hub.announce_task_removed(id);
    Ok(Json(json!({ "ok": true })))
}

async fn orchestrator(State(hub): State<Arc<Hub>>) -> ApiResult<Value> {
    Ok(Json(json!(hub.orchestrator_status().await)))
}

#[derive(Deserialize)]
struct SetOrchestrator {
    running: bool,
}

async fn set_orchestrator(State(hub): State<Arc<Hub>>, Json(body): Json<SetOrchestrator>) -> ApiResult<Value> {
    hub.set_orchestrator_running(body.running).await;
    let word = if body.running { "started" } else { "paused" };
    hub.event(Level::Info, "orchestrator", None, None, format!("Orchestrator {word}")).await;
    Ok(Json(json!(hub.orchestrator_status().await)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_name_strips_port_and_brackets() {
        assert_eq!(host_name("localhost:3000"), "localhost");
        assert_eq!(host_name("127.0.0.1"), "127.0.0.1");
        assert_eq!(host_name("[::1]:4400"), "::1");
        assert_eq!(host_name("agents.example.com"), "agents.example.com");
    }

    #[test]
    fn token_comparison() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }
}
