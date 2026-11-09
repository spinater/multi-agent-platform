//! HTTP API under `/api` and the dashboard's static files.

mod activity;
mod agents;
mod env;
mod configure;
mod rules_hooks;
mod workflows;
mod fs;
mod git;
mod git_access;
mod lanes;
mod manager;
mod plans;
mod projects;
mod skills;
mod templates;
mod ws;

use std::sync::Arc;

use axum::extract::{Path as UrlPath, Query, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::extract::DefaultBodyLimit;
use axum::routing::{any, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use tower_http::services::{ServeDir, ServeFile};

use crate::detect::{EndDecision, strip_ansi};
use crate::hub::{Hub, LaunchRequest};
use crate::model::{Event, Level, Overview, PipelineInfo, Role, Session, Task};
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
        .route("/sessions/{name}/trust", post(trust_session))
        .route("/tasks", get(tasks).post(create_task))
        .route("/tasks/page", get(task_page))
        .route("/tasks/{id}", axum::routing::patch(update_task).delete(delete_task))
        .route("/orchestrator", get(orchestrator).post(set_orchestrator))
        .route("/gates", get(gates_info).post(set_gates))
        .route("/gates/restart", post(restart_gates))
        .route("/gates/file", get(gates_file).put(put_gates_file))
        // ---- command centre
        .route("/pipeline", get(pipeline))
        .route("/projects", get(projects::list).post(projects::create))
        .route("/projects/{slug}", get(projects::get).patch(projects::update).delete(projects::delete))
        .route("/projects/{slug}/repos/{repo}", get(projects::repo_listing))
        .route("/projects/{slug}/repos/{repo}/file", get(projects::read_file).put(projects::write_file))
        .route("/projects/{slug}/reflex/restart", post(projects::restart))
        .route("/projects/{slug}/reflex/log", get(projects::reflex_log))
        .route("/projects/{slug}/decide", post(projects::decide))
        .route("/projects/{slug}/thai", get(projects::thai))
        .route("/projects/{slug}/thai/preview", post(projects::thai_preview))
        .route("/projects/{slug}/thai/words", put(projects::thai_words))
        .route("/projects/{slug}/feedback", post(projects::feedback))
        .route("/projects/{slug}/fit", post(projects::fit))
        .route("/projects/{slug}/decisions", get(projects::decisions))
        // ---- orchestrator decisions (decisions.rs; not Reflex decisions)
        .route("/tasks/{id}/trace", get(task_trace))
        .route("/decisions/model", get(decision_model))
        .route("/decisions/edges", get(decision_edges))
        .route("/decisions/latest", get(decision_latest))
        .route("/orchestrator/step", post(orchestrator_step))
        // ----
        // ---- skills: context files, skills and their use per workspace; enabling writes .claude/settings.local.json
        // ---- Configure: rules and hooks
        .route("/rules", get(rules_hooks::list_rules).put(rules_hooks::put_rule).delete(rules_hooks::delete_rule))
        .route("/rules/readers", put(rules_hooks::put_readers))
        .route("/hooks", get(rules_hooks::list_hooks).put(rules_hooks::put_hooks))
        .route("/hooks/test", post(rules_hooks::test_hook))
        .route("/skills", get(skills::overview))
        .route("/skills/enabled", axum::routing::patch(skills::set_enabled))
        .route("/skills/file", get(skills::file))
        // ---- agents: custom sub-agents for Claude, Codex and agy, per workspace and user-wide (agents.rs)
        .route("/agents", get(agents::overview).put(agents::save))
        .route("/agents/{name}", get(agents::detail).delete(agents::delete))
        // ---- Configure: tool permissions per CLI and MCP servers, in the CLIs' own files (permissions.rs, mcp.rs)
        .route("/permissions", get(configure::permissions).put(configure::set_permissions))
        .route("/mcp", get(configure::mcp).put(configure::save_mcp).delete(configure::delete_mcp))
        .route("/mcp/test", post(configure::test_mcp))
        .route("/workflows", get(workflows::list).put(workflows::save).delete(workflows::delete))
        .route("/workflows/{id}/stats", get(workflows::stats))
        .route("/configure/ask", post(workflows::ask))
        // ---- project settings and provider sign-in
        .route("/projects/defaults", get(projects::defaults))
        .route("/projects/inspect", post(projects::inspect))
        // ---- the folder picker: the project folder and its sub-folders (roots.rs); owner token only
        .route("/fs/roots", get(fs::list_roots))
        .route("/fs/browse", get(fs::browse))
        .route("/projects/{slug}/trust", get(projects::trust_state).post(projects::trust))
        .route("/projects/{slug}/worktree-estimate", post(projects::worktree_estimate))
        .route("/projects/{slug}/parallel/analyze", post(projects::parallel_analyze))
        .route("/projects/{slug}/scopes", post(projects::scopes))
        .route("/projects/{slug}/knowledge", get(projects::knowledge))
        .route("/projects/{slug}/knowledge/reindex", post(projects::knowledge_reindex))
        .route("/projects/{slug}/knowledge/accept", post(projects::knowledge_accept))
        .route("/projects/{slug}/knowledge/dismiss", post(projects::knowledge_dismiss))
        .route("/projects/{slug}/knowledge/search", get(projects::knowledge_search))
        .route("/projects/{slug}/knowledge/doc", get(projects::knowledge_doc))
        .route("/projects/{slug}/knowledge/graph", get(projects::knowledge_graph))
        .route("/providers/{name}/login", get(projects::login_state).post(projects::login_start))
        .route("/providers/{name}/login/code", post(projects::login_code))
        // ---- usage: plan limits per provider (usage.rs)
        .route("/usage", get(usage))
        .route("/usage/refresh", post(refresh_usage))
        // ---- updates: installed vs latest version of each provider CLI (updates.rs)
        .route("/updates", get(updates))
        .route("/updates/refresh", post(refresh_updates))
        .route("/providers/{name}/update", post(start_update))
        // Reflex screen: calibration cases, decision feedback and summary, corpus archive.
        .route("/projects/{slug}/cases", get(projects::cases).post(projects::add_cases))
        .route("/projects/{slug}/cases/fit", post(projects::fit_cases))
        .route("/projects/{slug}/cases/{id}", axum::routing::delete(projects::delete_case))
        .route("/projects/{slug}/decisions/summary", get(projects::decision_summary))
        .route("/projects/{slug}/decisions/{id}/feedback", post(projects::decision_feedback))
        .route("/projects/{slug}/corpus/archive", post(projects::archive))
        .route("/projects/{slug}/corpus/lessons", get(crate::corpus_learn::list_api)).route("/projects/{slug}/corpus/lessons/{id}", axum::routing::delete(crate::corpus_learn::remove_api))
        .route("/reflex/savings", get(projects::savings))
        // ---- templates: lay a development process into a project's folder, then queue the init agent
        // ---- git access: MAP's SSH key, identity and host tests; a project folder's git status and sync
        .route("/git/access", get(git_access::access))
        .route("/git/key", post(git_access::create_key))
        .route("/git/test", post(git_access::test))
        .route("/git/identity", axum::routing::put(git_access::set_identity))
        .route("/git/settings", axum::routing::put(git_access::set_settings))
        .route("/git/known-hosts/{host}", axum::routing::delete(git_access::remove_known_host))
        // ---- the Settings screen: read-only server facts, agent binaries, gh/glab and whether their tokens are set
        .route("/settings", get(git_access::settings))
        // ---- Configure: Env & secrets (env_vars.rs)
        .route("/env", get(env::list).put(env::put).delete(env::delete))
        .route("/projects/{slug}/git", get(git_access::status))
        .route("/projects/{slug}/git/fetch", post(git_access::fetch))
        .route("/projects/{slug}/git/pull", post(git_access::pull))
        .route("/projects/{slug}/git/push", post(git_access::push))
        .route("/projects/{slug}/git/remote", post(git_access::remote))
        .route("/templates", get(templates::list))
        .route("/projects/{slug}/init/plan", post(templates::plan))
        .route("/projects/{slug}/init", post(templates::init))
        // ---- requirements → plan → tasks: the planner's drafts, the owner's approval (plans.rs)
        .route("/projects/{slug}/plans", get(plans::project_list).post(plans::project_submit))
        .route("/workspace/plans", get(plans::workspace_list).post(plans::workspace_submit))
        .route("/plans", get(plans::list))
        .route("/plans/{id}", get(plans::get).patch(plans::edit))
        .route("/plans/{id}/approve", post(plans::approve))
        .route("/plans/{id}/reject", post(plans::reject))
        .route("/projects/{slug}/planner", get(plans::project_planner).post(plans::project_start))
        .route("/workspace/planner", get(plans::workspace_planner).post(plans::workspace_start))
        .route("/planner/instructions", get(plans::instructions))
        // ---- lanes: tmux sessions someone else owns that MAP watches (lanes.rs), and Telegram (telegram.rs)
        .route("/lanes", get(lanes::list))
        .route("/lanes/tells", get(lanes::all_tells))
        .route("/lanes/import-watcher", post(lanes::import_watcher))
        .route("/lanes/{name}", axum::routing::patch(lanes::update))
        .route("/lanes/{name}/screen", get(lanes::screen))
        .route("/lanes/{name}/tell", post(lanes::tell))
        .route("/lanes/{name}/tells", get(lanes::tells)).route("/lanes/{name}/chat", get(crate::lane_chat::get_api).post(crate::lane_chat::post_api))
        .route("/lanes/{name}/tells/{verb}", axum::routing::delete(lanes::cancel_tell))
        // ---- a project's manager (manager.rs) and the project-scoped routes its token opens (manager_scope)
        .route("/projects/{slug}/manager", get(manager::get).put(manager::put))
        .route("/projects/{slug}/manager/start", post(manager::start)).route("/projects/{slug}/manager/compact", get(crate::manager_compact::get_api).post(crate::manager_compact::post_api))
        .route("/manager/instructions", get(manager::instructions))
        .route("/projects/{slug}/overview", get(manager::overview))
        // ---- the activity graph (trace.rs)
        .route("/projects/{slug}/activity", get(activity::project))
        .route("/sessions/{name}/trace", get(activity::session))
        .route("/projects/{slug}/orchestrator", get(manager::orchestrator))
        .route("/projects/{slug}/pause", post(manager::pause))
        .route("/projects/{slug}/resume", post(manager::resume))
        .route("/projects/{slug}/tasks", get(manager::tasks).post(manager::create_task))
        .route("/projects/{slug}/tasks/{id}", get(manager::task).patch(manager::patch_task).delete(manager::delete_task))
        .route("/projects/{slug}/tasks/{id}/trace", get(manager::task_trace))
        .route("/projects/{slug}/plans/{id}/approve", post(manager::approve_plan))
        .route("/projects/{slug}/plans/{id}/reject", post(manager::reject_plan))
        .route("/projects/{slug}/lanes", get(manager::lanes).post(manager::open_lane))
        .route("/projects/{slug}/lanes/watch", post(lanes::add_watch))
        .route("/projects/{slug}/lanes/watch/{name}", axum::routing::delete(lanes::remove_watch))
        .route("/tmux/sessions", get(lanes::tmux_sessions))
        .route("/projects/{slug}/lanes/{name}", axum::routing::delete(manager::close_lane))
        .route("/projects/{slug}/lanes/{name}/start", post(manager::start_lane))
        .route("/projects/{slug}/lanes/{name}/tell", post(manager::tell_lane))
        .route("/projects/{slug}/lanes/{name}/screen", get(manager::lane_screen))
        .route("/telegram", get(lanes::telegram))
        .route("/telegram/test", post(lanes::telegram_test))
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

/// The project whose token may call `method path`: asking and correcting its decisions, reading its decision log,
/// reading or writing its corpus, and submitting and listing its plans (never approving them). Everything else
/// needs the API token.
pub(crate) fn project_scope<'a>(method: &axum::http::Method, path: &'a str) -> Option<&'a str> {
    use axum::http::Method;
    let rest = path.strip_prefix("/api").unwrap_or(path).strip_prefix("/projects/")?;
    let (slug, tail) = rest.split_once('/')?;
    let ok = matches!(
        (method, tail),
        (&Method::POST, "decide" | "feedback" | "plans" | "scopes")
            | (&Method::GET, "decisions" | "decisions/summary" | "repos/corpus" | "repos/corpus/file" | "plans" | "knowledge/search" | "knowledge/doc")
            | (&Method::PUT, "repos/corpus/file")
    );
    (ok && !slug.is_empty()).then_some(slug)
}

/// What a project's manager token (`$MAP_MANAGER_TOKEN`) may call, and for which project: everything its project
/// token may, plus that project's owner-level routes (overview, tasks, plan decisions, pause and resume, the
/// orchestrator's status, knowledge, decisions, lanes, git status, fetch and push). Never MAP's global settings,
/// sign-ins, Telegram, other projects, provider installs, the manager's own settings or deleting the project.
pub(crate) fn manager_scope<'a>(method: &axum::http::Method, path: &'a str) -> Option<&'a str> {
    use axum::http::Method;
    if let Some(slug) = project_scope(method, path) {
        return Some(slug);
    }
    let rest = path.strip_prefix("/api").unwrap_or(path).strip_prefix("/projects/")?;
    let (slug, tail) = rest.split_once('/').unwrap_or((rest, ""));
    if slug.is_empty() || matches!(slug, "defaults" | "inspect") {
        return None;
    }
    let seg: Vec<&str> = if tail.is_empty() { vec![] } else { tail.split('/').collect() };
    let id = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let name = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    let ok = match (method, seg.as_slice()) {
        (&Method::GET, [] | ["overview" | "orchestrator" | "manager" | "tasks" | "plans" | "lanes" | "git" | "knowledge" | "decisions" | "thai" | "activity"]) => true,
        (&Method::GET, ["decisions", "summary"] | ["knowledge", "search" | "doc" | "graph"]) => true,
        (&Method::GET, ["tasks", t] | ["tasks", t, "trace"]) => id(t),
        (&Method::GET, ["lanes", n, "screen"]) => name(n),
        (&Method::POST, ["pause" | "resume" | "tasks" | "lanes" | "plans"]) => true,
        (&Method::POST, ["knowledge", "accept" | "dismiss" | "reindex"] | ["git", "fetch" | "push"]) => true,
        (&Method::POST, ["plans", p, "approve" | "reject"]) => id(p),
        (&Method::POST, ["lanes", n, "tell" | "start"]) => name(n),
        (&Method::PATCH | &Method::DELETE, ["tasks", t]) => id(t),
        (&Method::DELETE, ["lanes", n]) => name(n),
        _ => false,
    };
    ok.then_some(slug)
}

/// Who called: the owner (the API token, or an open API), an agent's project token, a project's manager, or the
/// default workspace's planner. Handlers that treat the manager differently read it from the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    Owner,
    Project(String),
    Manager(String),
    Workspace,
}

/// What the default workspace's planner token may call: submitting and listing the default workspace's plans.
pub(crate) fn workspace_scope(method: &axum::http::Method, path: &str) -> bool {
    use axum::http::Method;
    path.strip_prefix("/api").unwrap_or(path) == "/workspace/plans" && matches!(*method, Method::GET | Method::POST)
}

async fn require_token(State(hub): State<Arc<Hub>>, headers: HeaderMap, mut request: Request, next: Next) -> Response {
    let Some(expected) = hub.config.server.api_token.as_deref() else {
        request.extensions_mut().insert(Caller::Owner);
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
        request.extensions_mut().insert(Caller::Owner);
        return next.run(request).await;
    }
    // An agent's project token opens only its own project's decisions and corpus.
    let path = request.extensions().get::<axum::extract::OriginalUri>().map_or_else(|| request.uri().path().to_string(), |u| u.path().to_string());
    if let (Some(token), Some(slug)) = (bearer, project_scope(request.method(), &path))
        && hub.project_token(slug).is_some_and(|t| same(token, &t))
    {
        let caller = Caller::Project(slug.to_string());
        request.extensions_mut().insert(caller);
        return next.run(request).await;
    }
    // A manager's token opens its own project's owner-level routes.
    if let (Some(token), Some(slug)) = (bearer, manager_scope(request.method(), &path))
        && hub.manager_token(slug).is_some_and(|t| same(token, &t))
    {
        let caller = Caller::Manager(slug.to_string());
        request.extensions_mut().insert(caller);
        return next.run(request).await;
    }
    if let Some(token) = bearer
        && workspace_scope(request.method(), &path)
        && hub.workspace_token().is_some_and(|t| same(token, &t))
    {
        request.extensions_mut().insert(Caller::Workspace);
        return next.run(request).await;
    }
    // A real project or manager token used outside its scope (another project, a global route) is forbidden, not
    // unauthenticated: the agent learns it may not, rather than that its token is wrong.
    if let Some(token) = bearer
        && let Ok(projects) = hub.store.projects().await
        && projects.iter().any(|p| hub.manager_token(&p.slug).is_some_and(|t| same(token, &t)) || hub.project_token(&p.slug).is_some_and(|t| same(token, &t)))
    {
        return ApiError(StatusCode::FORBIDDEN, "this token may not call this route (it opens its own project only; the rest needs the owner's API token)".into()).into_response();
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

/// "Trust this folder": only while the pane shows the CLI's folder-trust question and nobody is attached or typing,
/// records the folder as trusted and picks the CLI's trust option (409 otherwise). Takes `{}`.
async fn trust_session(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Json(_): Json<Value>) -> ApiResult<crate::model::TrustAnswer> {
    match crate::trust::answer(&hub, &name).await {
        Ok(answer) => Ok(Json(answer)),
        Err(crate::trust::AnswerError::NotFound(why)) => Err(ApiError(StatusCode::NOT_FOUND, why)),
        Err(crate::trust::AnswerError::Conflict(why)) => Err(ApiError(StatusCode::CONFLICT, why)),
        Err(crate::trust::AnswerError::Failed(err)) => Err(err.into()),
    }
}

/// `GET /api/tasks` and `/api/tasks/page` filters. `project` absent: every project; empty: the default workspace.
#[derive(Deserialize, Default)]
struct TasksQuery {
    status: Option<crate::model::TaskStatus>,
    q: Option<String>,
    project: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

impl TasksQuery {
    fn any(&self) -> bool {
        self.status.is_some() || self.q.is_some() || self.project.is_some() || self.limit.is_some() || self.offset.is_some()
    }

    fn store(self, default_limit: i64) -> crate::store::TaskQuery {
        crate::store::TaskQuery { status: self.status, project: self.project, q: self.q, limit: self.limit.unwrap_or(default_limit), offset: self.offset.unwrap_or(0) }
    }
}

/// Without parameters every task, oldest first (as always). With any filter: the matching tasks, newest change first,
/// at most `limit` (default 500) from `offset`.
async fn tasks(State(hub): State<Arc<Hub>>, Query(q): Query<TasksQuery>) -> ApiResult<Vec<Task>> {
    if !q.any() {
        return Ok(Json(hub.store.tasks().await?));
    }
    Ok(Json(hub.store.task_page(&q.store(500)).await?.items.into_iter().map(|i| i.task).collect()))
}

/// One page of tasks with the total (the Done tasks screen: `?status=done&project=<slug>&q=<title or #id>`), newest
/// change first; `limit` 1–500 (default 50).
async fn task_page(State(hub): State<Arc<Hub>>, Query(q): Query<TasksQuery>) -> ApiResult<crate::model::TaskPage> {
    Ok(Json(hub.store.task_page(&q.store(50)).await?))
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
    /// A workflow id of the task's scope; empty or missing: a workflow trigger may choose one.
    workflow: Option<String>,
}

/// A workflow the task may name: one the project (or the machine) has.
pub(crate) async fn known_workflow(hub: &Hub, project: Option<&str>, workflow: Option<String>) -> Result<Option<String>, ApiError> {
    let Some(id) = workflow.map(|w| w.trim().to_string()).filter(|w| !w.is_empty()) else { return Ok(None) };
    if crate::workflows::find(hub, project, &id).await?.is_none() {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("workflow `{id}` does not exist for {}", project.unwrap_or("the machine"))));
    }
    Ok(Some(id))
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
    let workflow = known_workflow(&hub, project.as_deref(), body.workflow).await?;
    for dep in &body.depends_on {
        hub.store.task(*dep).await?.ok_or_else(|| ApiError(StatusCode::BAD_REQUEST, format!("dependency #{dep} does not exist")))?;
    }
    let task = hub
        .store
        .create_task(NewTask {
            title: body.title.trim().into(),
            prompt: body.prompt,
            depends_on: body.depends_on,
            provider,
            model: body.model.filter(|m| !m.is_empty()),
            project,
            workflow,
        })
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
    /// From `review`: rebase and fast-forward into the base.
    Merge,
    /// From `review`: push the branch and open a pull request.
    PullRequest,
    /// From `review`: back to an agent, with the owner's note.
    SendBack,
    /// Removes the task's worktree and branch; a task in review or failed becomes cancelled.
    Discard,
    /// A task the intake gate holds (unclear, plan first, duplicate): start it as it is.
    StartAnyway,
    /// A pending task: the note is added to its prompt and the intake gate looks again.
    Clarify,
}

#[derive(Deserialize)]
struct UpdateTask {
    action: TaskAction,
    #[serde(default)]
    note: Option<String>,
}

async fn update_task(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<i64>, Json(body): Json<UpdateTask>) -> ApiResult<Task> {
    let task = match body.action {
        TaskAction::Retry => Orchestrator::retry(&hub, id).await?,
        TaskAction::Cancel => Orchestrator::cancel(&hub, id).await?,
        TaskAction::Merge => Orchestrator::land_now(&hub, id, crate::model::LandMode::Merge).await.map_err(conflict)?,
        TaskAction::PullRequest => Orchestrator::land_now(&hub, id, crate::model::LandMode::PullRequest).await.map_err(conflict)?,
        TaskAction::SendBack => Orchestrator::send_back(&hub, id, body.note.as_deref()).await.map_err(conflict)?,
        TaskAction::Discard => Orchestrator::discard(&hub, id).await.map_err(conflict)?,
        TaskAction::StartAnyway => crate::intake::start_anyway(&hub, id).await.map_err(conflict)?,
        TaskAction::Clarify => crate::intake::clarify(&hub, id, body.note.as_deref().unwrap_or_default()).await.map_err(conflict)?,
    };
    Ok(Json(task))
}

async fn delete_task(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<i64>) -> ApiResult<Value> {
    let task = hub.store.task(id).await?.ok_or_else(|| not_found("task"))?;
    if !task.status.is_terminal() && task.status != crate::model::TaskStatus::Pending {
        return Err(ApiError(StatusCode::CONFLICT, "cancel a running task before deleting it".into()));
    }
    let mut task = task;
    crate::worktrees::discard(&hub, &mut task).await;
    hub.store.delete_task(id).await?;
    hub.announce_task_removed(id);
    Ok(Json(json!({ "ok": true })))
}

async fn gates_info_of(hub: &Hub) -> Result<crate::model::GatesInfo, ApiError> {
    let t = hub.gates_target();
    let files = match hub.gates_corpus().await {
        Ok(repo) => crate::repo::files(&repo).await.unwrap_or_default(),
        Err(_) => vec![],
    };
    let projects = hub.store.projects().await?.into_iter().filter(|p| p.settings.reflex_gates.any()).map(|p| p.slug).collect();
    Ok(crate::model::GatesInfo {
        status: hub.reflex.status(crate::gates::KEY),
        port: t.port,
        threshold: t.threshold,
        config_threshold: hub.config.reflex.gates_threshold,
        files,
        projects,
    })
}

async fn gates_info(State(hub): State<Arc<Hub>>) -> ApiResult<crate::model::GatesInfo> {
    Ok(Json(gates_info_of(&hub).await?))
}

#[derive(Deserialize)]
struct SetGates {
    /// The gates' threshold; `null` goes back to map.toml's (unset there: shadow).
    threshold: Option<f64>,
}

async fn set_gates(State(hub): State<Arc<Hub>>, Json(b): Json<SetGates>) -> ApiResult<crate::model::GatesInfo> {
    hub.set_gates_threshold(b.threshold).await.map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    hub.event(Level::Info, "reflex.gates", None, None, format!("gates threshold {}", hub.gates_target().threshold.map_or("none (shadow)".into(), crate::decide::fmt_conf))).await;
    Ok(Json(gates_info_of(&hub).await?))
}

async fn restart_gates(State(hub): State<Arc<Hub>>) -> ApiResult<crate::model::GatesInfo> {
    hub.restart_gates().await?;
    Ok(Json(gates_info_of(&hub).await?))
}

#[derive(Deserialize)]
struct GatesPath {
    path: String,
}

async fn gates_file(State(hub): State<Arc<Hub>>, Query(q): Query<GatesPath>) -> ApiResult<Value> {
    crate::projects::validate_corpus_path(&q.path).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    let text = crate::repo::read(&hub.gates_corpus().await?, &q.path).await.ok_or_else(|| not_found("file"))?;
    Ok(Json(json!({ "path": q.path, "content": text })))
}

#[derive(Deserialize)]
struct GatesFileBody {
    path: String,
    content: String,
}

/// Saves one file of the gates corpus (a domain's prose) and restarts the gates Reflex. The domains stay the answers'.
async fn put_gates_file(State(hub): State<Arc<Hub>>, Json(b): Json<GatesFileBody>) -> ApiResult<crate::model::GatesInfo> {
    crate::projects::validate_corpus_path(&b.path).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{e:#}")))?;
    let domain = b.path.split('/').next().unwrap_or_default();
    if !crate::gates::SEED.iter().any(|(p, _)| p.split('/').next() == Some(domain)) {
        return Err(ApiError(StatusCode::BAD_REQUEST, format!("`{domain}` is not one of the gates' answers")));
    }
    if b.content.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "a domain needs its prose".into()));
    }
    let repo = hub.gates_corpus().await?;
    crate::repo::commit(&repo, &[(b.path.clone(), Some(b.content))], &format!("Update {}", b.path)).await?;
    hub.restart_gates().await?;
    Ok(Json(gates_info_of(&hub).await?))
}

fn conflict(err: anyhow::Error) -> ApiError {
    ApiError(StatusCode::CONFLICT, format!("{err:#}"))
}

async fn orchestrator(State(hub): State<Arc<Hub>>) -> ApiResult<Value> {
    Ok(Json(json!(hub.orchestrator_status().await)))
}

#[derive(Deserialize)]
struct SetOrchestrator {
    running: Option<bool>,
    /// Tasks at once across all projects, applied at the next tick; `0` goes back to map.toml's value.
    max_parallel: Option<usize>,
}

async fn set_orchestrator(State(hub): State<Arc<Hub>>, Json(body): Json<SetOrchestrator>) -> ApiResult<Value> {
    if let Some(running) = body.running {
        hub.set_orchestrator_running(running).await;
        let word = if running { "started" } else { "paused" };
        hub.event(Level::Info, "orchestrator", None, None, format!("Orchestrator {word}")).await;
    }
    if let Some(n) = body.max_parallel {
        hub.set_max_parallel((n > 0).then_some(n)).await?;
        hub.event(Level::Info, "orchestrator", None, None, format!("Up to {} tasks at once", hub.max_parallel())).await;
    }
    Ok(Json(json!(hub.orchestrator_status().await)))
}

// ---- orchestrator decisions (decisions.rs): per-task traces, the decision graph, edge counts, single steps

#[derive(Deserialize)]
struct TraceQuery {
    limit: Option<i64>,
}

/// The task's `decision.*` events, newest first.
async fn task_trace(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<i64>, Query(q): Query<TraceQuery>) -> ApiResult<Vec<Event>> {
    hub.store.task(id).await?.ok_or_else(|| not_found("task"))?;
    Ok(Json(hub.store.decision_events(id, q.limit.unwrap_or(200).clamp(1, 2000)).await?))
}

async fn decision_model(State(hub): State<Arc<Hub>>) -> ApiResult<crate::model::DecisionModel> {
    Ok(Json(crate::decisions::model(&hub, crate::orchestrator::TICK.as_secs())))
}

/// `project` absent: every task; empty: the default workspace; else that project's tasks.
#[derive(Deserialize)]
struct DecisionQuery {
    since: Option<chrono::DateTime<chrono::Utc>>,
    project: Option<String>,
}

/// How often each edge was taken since `since` (default: the last 24 hours).
async fn decision_edges(State(hub): State<Arc<Hub>>, Query(q): Query<DecisionQuery>) -> ApiResult<crate::model::DecisionCounts> {
    let since = q.since.unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::hours(24));
    Ok(Json(crate::decisions::counts(&hub, since, q.project.as_deref()).await?))
}

/// The newest decision event of each task.
async fn decision_latest(State(hub): State<Arc<Hub>>, Query(q): Query<DecisionQuery>) -> ApiResult<Vec<Event>> {
    Ok(Json(hub.store.latest_decisions(q.project.as_deref()).await?))
}

/// Runs exactly one orchestrator tick while it is paused and returns the decisions that tick made. Verify, audit and
/// post_done run in the background after a tick, so their decisions arrive later as events.
async fn orchestrator_step(State(hub): State<Arc<Hub>>) -> ApiResult<crate::model::StepResult> {
    if hub.orchestrator_running() {
        return Err(ApiError(StatusCode::CONFLICT, "the orchestrator is running; pause it to step".into()));
    }
    let before = hub.store.last_event_id().await?;
    if !hub.step.step(std::time::Duration::from_secs(60)).await {
        return Err(ApiError(StatusCode::GATEWAY_TIMEOUT, "the orchestrator did not finish a tick within 60s".into()));
    }
    let decisions = hub.store.decision_events_after(before).await?;
    hub.event(Level::Info, "orchestrator.step", None, None, format!("Stepped one tick: {} decisions", decisions.len())).await;
    Ok(Json(crate::model::StepResult { decisions }))
}

// ---- command centre

async fn pipeline(State(hub): State<Arc<Hub>>) -> ApiResult<PipelineInfo> {
    let o = &hub.config.orchestrator;
    Ok(Json(PipelineInfo {
        provider_order: o.provider_order.clone(),
        max_attempts: o.max_attempts,
        task_timeout_minutes: o.task_timeout_minutes,
        verify: o.verify.clone(),
        audit_enabled: o.audit.enabled,
        audit_required: o.audit.required,
        audit_chain: o.audit.chain.clone(),
        post_done: o.post_done.clone(),
    }))
}

// ---- usage

/// The latest reading (polled every `usage.interval_secs`).
async fn usage(State(hub): State<Arc<Hub>>) -> Json<crate::model::UsageReport> {
    Json(hub.usage.report())
}

/// Reads every provider now (a read in the last few seconds is returned as is). Takes a JSON body (`{}`) like the
/// other state-changing routes, so a cross-site form cannot trigger it.
async fn refresh_usage(State(hub): State<Arc<Hub>>, Json(_): Json<Value>) -> Json<crate::model::UsageReport> {
    Json(crate::usage::refresh(&hub).await)
}

// ---- updates

/// The latest check (every `updates.interval_secs`).
async fn updates(State(hub): State<Arc<Hub>>) -> Json<crate::model::UpdatesReport> {
    Json(hub.updates.report())
}

/// Checks every provider now (a check in the last few seconds is returned as is). Takes `{}` like the other
/// state-changing routes.
async fn refresh_updates(State(hub): State<Arc<Hub>>, Json(_): Json<Value>) -> Json<crate::model::UpdatesReport> {
    Json(crate::updates::refresh(&hub, crate::updates::DEBOUNCE).await)
}

/// Runs the provider's `update` command in the helper session `<prefix>update-<name>` (409 while one runs, or when
/// the CLI is installed read-only). The version is checked again when the session ends. Takes `{}`.
async fn start_update(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Json(_): Json<Value>) -> ApiResult<Session> {
    match crate::reflex_update::start(&hub, &name).await {
        Ok(session) => Ok(Json(session)),
        Err(crate::updates::StartError::Conflict(why)) => Err(ApiError(StatusCode::CONFLICT, why)),
        Err(crate::updates::StartError::Invalid(err)) => Err(err.into()),
    }
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
    fn project_tokens_open_only_their_projects_decisions_and_corpus() {
        use axum::http::Method;
        assert_eq!(project_scope(&Method::POST, "/api/projects/support/decide"), Some("support"));
        assert_eq!(project_scope(&Method::POST, "/projects/support/feedback"), Some("support"));
        assert_eq!(project_scope(&Method::GET, "/projects/support/repos/corpus/file"), Some("support"));
        assert_eq!(project_scope(&Method::PUT, "/projects/support/repos/corpus/file"), Some("support"));
        assert_eq!(project_scope(&Method::GET, "/projects/support/decisions"), Some("support"));
        assert_eq!(project_scope(&Method::POST, "/api/projects/support/plans"), Some("support"));
        assert_eq!(project_scope(&Method::GET, "/api/projects/support/plans"), Some("support"));
        assert_eq!(project_scope(&Method::GET, "/api/projects/support/knowledge/search"), Some("support"));
        assert_eq!(project_scope(&Method::GET, "/api/projects/support/knowledge/doc"), Some("support"));
        assert_eq!(project_scope(&Method::POST, "/api/projects/support/scopes"), Some("support"));
        assert_eq!(project_scope(&Method::POST, "/api/projects/support/parallel/analyze"), None);
        for (method, path) in [
            (Method::GET, "/api/projects/support/knowledge"),
            (Method::POST, "/api/projects/support/knowledge/reindex"),
            (Method::POST, "/api/projects/support/planner"),
            (Method::PATCH, "/api/projects/support/plans"),
            (Method::POST, "/api/plans/1/approve"),
            (Method::GET, "/api/plans"),
        ] {
            assert_eq!(project_scope(&method, path), None, "{method} {path}");
        }
        assert!(workspace_scope(&Method::POST, "/api/workspace/plans") && workspace_scope(&Method::GET, "/api/workspace/plans"));
        for (method, path) in [(Method::DELETE, "/api/workspace/plans"), (Method::POST, "/api/workspace/planner"), (Method::GET, "/api/plans"), (Method::POST, "/api/projects/support/plans")] {
            assert!(!workspace_scope(&method, path), "{method} {path}");
        }
        for (method, path) in [
            (Method::GET, "/projects/support"),
            (Method::PATCH, "/projects/support"),
            (Method::DELETE, "/projects/support"),
            (Method::PUT, "/projects/support/repos/code/file"),
            (Method::POST, "/projects/support/init"),
            (Method::POST, "/projects/support/reflex/restart"),
            (Method::GET, "/tasks"),
            (Method::POST, "/sessions"),
            (Method::POST, "/projects//decide"),
            (Method::GET, "/api/fs/roots"),
            (Method::GET, "/api/fs/browse"),
        ] {
            assert_eq!(project_scope(&method, path), None, "{method} {path}");
        }
    }

    #[tokio::test]
    async fn project_tokens_differ_per_project_and_need_the_api_token() {
        let dir = tempfile::tempdir().unwrap();
        let hub = |token: &str| {
            let text = format!("[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n{token}", dir.path().display());
            crate::config::Config::parse(&text, std::path::Path::new("/")).unwrap()
        };
        let open = Hub::new(hub(""), crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        assert_eq!(open.project_token("a"), None);
        let locked = Hub::new(hub("api_token = \"secret\""), crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        let (a, b) = (locked.project_token("a").unwrap(), locked.project_token("b").unwrap());
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        assert!(!a.contains("secret"));
    }

    #[test]
    fn token_comparison() {
        assert!(same("abc", "abc"));
        assert!(!same("abc", "abd"));
        assert!(!same("abc", "abcd"));
    }
}
