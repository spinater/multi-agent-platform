//! A project's manager (`manager.rs`) and the project-scoped routes its `$MAP_MANAGER_TOKEN` opens (see
//! `manager_scope`): the project's overview, tasks, plan decisions, pause/resume, orchestrator status and lanes. The
//! owner's token opens them too. The manager's own settings (`PUT …/manager`) are the owner's only.

use std::sync::Arc;

use axum::extract::{Extension, Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{ApiError, ApiResult, Caller, not_found};
use crate::detect::EndDecision;
use crate::hub::Hub;
use crate::manager;
use crate::model::{Event, LaneInfo, LaneScreen, LaneTell, Level, ManagerInfo, ManagerSettings, OpenLane, OwnedLaneRecord, Plan, PlanApproved, ProjectOverview, Session, Task, TaskStatus, TellVerb};
use crate::store::NewTask;

type CallerExt = Option<Extension<Caller>>;

fn by(caller: &CallerExt) -> &'static str {
    match caller.as_ref().map(|c| &c.0) {
        Some(Caller::Manager(_)) => "manager",
        _ => "owner",
    }
}

fn is_manager(caller: &CallerExt) -> bool {
    matches!(caller.as_ref().map(|c| &c.0), Some(Caller::Manager(_)))
}

async fn known(hub: &Hub, slug: &str) -> Result<(), ApiError> {
    hub.store.project(slug).await?.ok_or_else(|| not_found("project"))?;
    Ok(())
}

// ---- the manager

pub async fn get(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<ManagerInfo> {
    known(&hub, &slug).await?;
    Ok(Json(manager::info(&hub, &slug).await?))
}

#[derive(Deserialize)]
pub struct PutBody {
    #[serde(flatten)]
    settings: ManagerSettings,
    #[serde(default)]
    force: bool,
}

/// Owner only: on/off, provider and model. Turning it off ends the session (409 while someone uses it, unless
/// `force`).
pub async fn put(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<PutBody>) -> ApiResult<ManagerInfo> {
    known(&hub, &slug).await?;
    manager::configure(&hub, &slug, b.settings, b.force).await.map(Json).map_err(|e| {
        let msg = format!("{e:#}");
        ApiError(if msg.contains("attached") || msg.contains("typed") { StatusCode::CONFLICT } else { StatusCode::BAD_REQUEST }, msg)
    })
}

/// Owner only: start it now (also after MAP stopped restarting it).
pub async fn start(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(_): Json<Value>) -> ApiResult<Session> {
    known(&hub, &slug).await?;
    Ok(Json(manager::start_now(&hub, &slug).await?))
}

pub async fn instructions(State(hub): State<Arc<Hub>>) -> ApiResult<crate::model::PlannerInstructions> {
    Ok(Json(manager::instructions(&hub)?))
}

// ---- project-scoped routes (owner or manager token)

pub async fn overview(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<ProjectOverview> {
    known(&hub, &slug).await?;
    Ok(Json(manager::overview(&hub, &slug).await?))
}

pub async fn orchestrator(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<Value> {
    known(&hub, &slug).await?;
    Ok(Json(json!(hub.orchestrator_status().await)))
}

async fn set_paused(hub: &Hub, slug: &str, paused: bool, who: &str) -> Result<crate::model::ProjectInfo, ApiError> {
    let mut p = hub.store.project(slug).await?.ok_or_else(|| not_found("project"))?;
    p.settings.paused = paused;
    hub.put_project(&p).await?;
    hub.event(Level::Info, "project.paused", None, None, format!("Project {slug} {} (by {who})", if paused { "paused" } else { "resumed" })).await;
    Ok(hub.project_info(p).await?)
}

pub async fn pause(State(hub): State<Arc<Hub>>, caller: CallerExt, UrlPath(slug): UrlPath<String>, Json(_): Json<Value>) -> ApiResult<crate::model::ProjectInfo> {
    Ok(Json(set_paused(&hub, &slug, true, by(&caller)).await?))
}

pub async fn resume(State(hub): State<Arc<Hub>>, caller: CallerExt, UrlPath(slug): UrlPath<String>, Json(_): Json<Value>) -> ApiResult<crate::model::ProjectInfo> {
    Ok(Json(set_paused(&hub, &slug, false, by(&caller)).await?))
}

#[derive(Deserialize)]
pub struct TasksQuery {
    status: Option<String>,
    limit: Option<usize>,
}

/// The project's tasks, newest first; `status` filters (comma-separated).
pub async fn tasks(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Query(q): Query<TasksQuery>) -> ApiResult<Vec<Task>> {
    known(&hub, &slug).await?;
    let wanted: Vec<String> = q.status.unwrap_or_default().split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    let mut out: Vec<Task> = hub.store.tasks().await?.into_iter().filter(|t| t.project.as_deref() == Some(slug.as_str())).filter(|t| wanted.is_empty() || wanted.iter().any(|w| w == t.status.as_str())).collect();
    out.sort_by_key(|t| std::cmp::Reverse(t.id));
    out.truncate(q.limit.unwrap_or(200).clamp(1, 1000));
    Ok(Json(out))
}

/// The task, when it is this project's.
async fn task_of(hub: &Hub, slug: &str, id: i64) -> Result<Task, ApiError> {
    hub.store.task(id).await?.filter(|t| t.project.as_deref() == Some(slug)).ok_or_else(|| not_found("task"))
}

pub async fn task(State(hub): State<Arc<Hub>>, UrlPath((slug, id)): UrlPath<(String, i64)>) -> ApiResult<Task> {
    Ok(Json(task_of(&hub, &slug, id).await?))
}

pub async fn task_trace(State(hub): State<Arc<Hub>>, UrlPath((slug, id)): UrlPath<(String, i64)>) -> ApiResult<Vec<Event>> {
    task_of(&hub, &slug, id).await?;
    Ok(Json(hub.store.decision_events(id, 200).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateBody {
    title: String,
    prompt: String,
    #[serde(default)]
    depends_on: Vec<i64>,
    provider: Option<String>,
    model: Option<String>,
    /// A workflow of the project (or the machine); missing: a workflow trigger may choose one.
    #[serde(default)]
    workflow: Option<String>,
}

pub async fn create_task(State(hub): State<Arc<Hub>>, caller: CallerExt, UrlPath(slug): UrlPath<String>, Json(b): Json<CreateBody>) -> ApiResult<Task> {
    known(&hub, &slug).await?;
    if b.title.trim().is_empty() || b.prompt.trim().is_empty() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "a task needs a title and a prompt".into()));
    }
    let provider = b.provider.filter(|p| !p.trim().is_empty());
    let model = b.model.filter(|m| !m.trim().is_empty());
    if let Some(p) = &provider {
        let spec = hub.spec(p)?;
        if model.is_some() {
            Hub::resolve_model(&spec, model.as_deref())?;
        }
    }
    for dep in &b.depends_on {
        task_of(&hub, &slug, *dep).await.map_err(|_| ApiError(StatusCode::BAD_REQUEST, format!("dependency #{dep} is not a task of {slug}")))?;
    }
    let workflow = super::known_workflow(&hub, Some(&slug), b.workflow).await?;
    let mut task = hub.store.create_task(NewTask { title: b.title.trim().into(), prompt: b.prompt, depends_on: b.depends_on, provider, model, project: Some(slug.clone()), workflow }).await?;
    // Who made it, for the activity graph: the manager (from its terminal, the dashboard or Telegram), or the owner.
    hub.store.set_task_created_by(task.id, by(&caller)).await?;
    task.created_by = by(&caller).into();
    hub.announce_task(&task);
    hub.event(Level::Info, "task.created", None, Some(task.id), format!("Task #{} created: {} (by {})", task.id, task.title, by(&caller))).await;
    Ok(Json(task))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchBody {
    action: Option<String>,
    note: Option<String>,
    /// Edit a pending task.
    title: Option<String>,
    prompt: Option<String>,
}

/// An action (as `PATCH /api/tasks/{id}`) or an edit of a pending task's title and prompt. The manager may cancel,
/// retry, start_anyway and clarify; merging, pull requests, send-back and discard stay the owner's.
pub async fn patch_task(State(hub): State<Arc<Hub>>, caller: CallerExt, UrlPath((slug, id)): UrlPath<(String, i64)>, Json(b): Json<PatchBody>) -> ApiResult<Task> {
    let mut task = task_of(&hub, &slug, id).await?;
    if let Some(action) = b.action.as_deref() {
        let allowed = ["cancel", "retry", "start_anyway", "clarify"];
        if is_manager(&caller) && !allowed.contains(&action) {
            return Err(ApiError(StatusCode::FORBIDDEN, format!("the manager may only {} a task; `{action}` is the owner's", allowed.join(", "))));
        }
        let body = json!({ "action": action, "note": b.note });
        return super::update_task(State(hub), UrlPath(id), Json(serde_json::from_value(body).map_err(|e| ApiError(StatusCode::BAD_REQUEST, format!("{e}")))?)).await;
    }
    if b.title.is_none() && b.prompt.is_none() {
        return Err(ApiError(StatusCode::BAD_REQUEST, "send an action, or a title and/or prompt to edit".into()));
    }
    if task.status != TaskStatus::Pending {
        return Err(ApiError(StatusCode::CONFLICT, format!("task #{id} is {}; only a pending task can be edited", task.status.as_str())));
    }
    if let Some(t) = b.title.map(|t| t.trim().to_string()) {
        if t.is_empty() {
            return Err(ApiError(StatusCode::BAD_REQUEST, "a task needs a title".into()));
        }
        task.title = t;
    }
    if let Some(p) = b.prompt {
        if p.trim().is_empty() {
            return Err(ApiError(StatusCode::BAD_REQUEST, "a task needs a prompt".into()));
        }
        task.prompt = p;
    }
    hub.put_task(&mut task).await?;
    hub.event(Level::Info, "task.edited", None, Some(id), format!("Task #{id} edited (by {})", by(&caller))).await;
    Ok(Json(task))
}

pub async fn delete_task(State(hub): State<Arc<Hub>>, UrlPath((slug, id)): UrlPath<(String, i64)>) -> ApiResult<Value> {
    task_of(&hub, &slug, id).await?;
    super::delete_task(State(hub), UrlPath(id)).await
}

/// The plan, when it is this project's.
async fn plan_of(hub: &Hub, slug: &str, id: i64) -> Result<Plan, ApiError> {
    hub.store.plan(id).await?.filter(|p| p.project.as_deref() == Some(slug)).ok_or_else(|| not_found("plan"))
}

pub async fn approve_plan(State(hub): State<Arc<Hub>>, caller: CallerExt, UrlPath((slug, id)): UrlPath<(String, i64)>, Json(_): Json<Value>) -> ApiResult<PlanApproved> {
    plan_of(&hub, &slug, id).await?;
    let out = crate::plans::approve(&hub, id).await?;
    if is_manager(&caller) {
        hub.event(Level::Info, "plan.approved_by", None, None, format!("Plan #{id} approved by the manager of {slug}")).await;
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct RejectBody {
    #[serde(default)]
    reason: Option<String>,
}

pub async fn reject_plan(State(hub): State<Arc<Hub>>, UrlPath((slug, id)): UrlPath<(String, i64)>, Json(b): Json<RejectBody>) -> ApiResult<Plan> {
    plan_of(&hub, &slug, id).await?;
    Ok(Json(crate::plans::reject(&hub, id, b.reason).await?))
}

// ---- lanes of the project

pub async fn lanes(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<Vec<LaneInfo>> {
    known(&hub, &slug).await?;
    Ok(Json(manager::project_lanes(&hub, &slug).await?))
}

pub async fn open_lane(State(hub): State<Arc<Hub>>, caller: CallerExt, UrlPath(slug): UrlPath<String>, Json(b): Json<OpenLane>) -> Result<(StatusCode, Json<OwnedLaneRecord>), ApiError> {
    known(&hub, &slug).await?;
    Ok((StatusCode::CREATED, Json(manager::open_lane(&hub, &slug, b, by(&caller)).await?)))
}

/// The lane's full name: a persistent lane by its short or full name, or a `[[lanes]]` block watching the folder.
async fn lane_name(hub: &Hub, slug: &str, name: &str) -> Result<String, ApiError> {
    if let Some(r) = manager::find_lane(hub, slug, name).await? {
        return Ok(r.lane);
    }
    if name == manager::session_name(hub, slug) || manager::is_project_lane(hub, slug, name).await {
        return Ok(name.to_string());
    }
    Err(not_found("lane"))
}

#[derive(Deserialize)]
pub struct CloseQuery {
    #[serde(default)]
    force: bool,
}

/// Closes a persistent lane; refused while someone is attached or typing. `force` is the owner's only.
pub async fn close_lane(State(hub): State<Arc<Hub>>, caller: CallerExt, UrlPath((slug, name)): UrlPath<(String, String)>, Query(q): Query<CloseQuery>) -> ApiResult<Value> {
    if q.force && is_manager(&caller) {
        return Err(ApiError(StatusCode::FORBIDDEN, "only the owner may force a lane closed".into()));
    }
    if manager::find_lane(&hub, &slug, &name).await?.is_none() {
        return Err(not_found("persistent lane"));
    }
    match manager::close_lane(&hub, &slug, &name, q.force).await? {
        EndDecision::End => Ok(Json(json!({ "ok": true }))),
        EndDecision::KeepAttached => Err(ApiError(StatusCode::CONFLICT, "a terminal is attached to this lane".into())),
        EndDecision::KeepInput => Err(ApiError(StatusCode::CONFLICT, "someone has typed on this lane's input line".into())),
    }
}

pub async fn start_lane(State(hub): State<Arc<Hub>>, UrlPath((slug, name)): UrlPath<(String, String)>, Json(_): Json<Value>) -> ApiResult<Session> {
    if manager::find_lane(&hub, &slug, &name).await?.is_none() {
        return Err(not_found("persistent lane"));
    }
    Ok(Json(manager::start_lane(&hub, &slug, &name).await?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TellBody {
    verb: TellVerb,
    note: Option<String>,
    mins: Option<u32>,
    session: Option<String>,
}

pub async fn tell_lane(State(hub): State<Arc<Hub>>, UrlPath((slug, name)): UrlPath<(String, String)>, Json(b): Json<TellBody>) -> ApiResult<LaneTell> {
    let lane = lane_name(&hub, &slug, &name).await?;
    Ok(Json(crate::lanes::tell(&hub, &lane, b.verb, b.note.as_deref(), b.mins, b.session.as_deref(), Utc::now()).await?))
}

#[derive(Deserialize)]
pub struct ScreenQuery {
    lines: Option<usize>,
}

pub async fn lane_screen(State(hub): State<Arc<Hub>>, UrlPath((slug, name)): UrlPath<(String, String)>, Query(q): Query<ScreenQuery>) -> ApiResult<LaneScreen> {
    let lane = lane_name(&hub, &slug, &name).await?;
    Ok(Json(hub.lanes.screen(&lane, None, q.lines.unwrap_or(60))))
}
