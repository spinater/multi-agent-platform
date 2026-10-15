//! A project's manager: an always-on interactive agent session (`<prefix>mgr-<slug>`) the owner talks to about the
//! project, from the dashboard, its own terminal or Telegram (`/ask <slug> …`). It reads and changes the project's
//! state through MAP's API with `$MAP_MANAGER_TOKEN`, which opens that project's owner-level routes only
//! (`api::manager_scope`).
//!
//! While `settings.manager.enabled`, MAP keeps it running: started at boot and started again when its session
//! disappears, resuming the conversation when the provider can (`resume_args`, Claude's `--resume <id>`). Three
//! failures within ten minutes stop that and raise a warning until the owner starts it again.
//!
//! Persistent lanes are agent sessions MAP opens in a project's folder on request (`POST
//! /api/projects/{slug}/lanes`, from the owner or the manager): stored in the database and watched like every lane,
//! but never relaunched by themselves (a gone session shows as stopped with a Start button).
//!
//! Both are registered as MAP-owned lanes (`lanes.rs`), so rate-limit wake-ups, stall alerts and Telegram notices
//! apply; their sessions run on MAP's own tmux server.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use sha2::{Digest, Sha256};

use crate::config::{LaneConfig, OwnedLane};
use crate::detect::EndDecision;
use crate::hub::{Hub, LaunchRequest};
use crate::model::{
    LaneAskWhen, LaneClearWhen, Level, ManagerInfo, ManagerSettings, OpenLane, OwnedLaneRecord, PlanStatus, PlannerInstructions, Project, ProjectOverview, Role, Session, TaskStatus,
};

/// The manager's instructions shipped with MAP; `templates.manager_agent` in `map.toml` replaces them.
pub const MANAGER_AGENT: &str = include_str!("../templates/manager-agent.md");
/// Failures within [`FAIL_WINDOW_MINS`] that stop the restarts.
pub const MAX_FAILURES: usize = 3;
pub const FAIL_WINDOW_MINS: i64 = 10;
/// How often the supervisor looks.
pub const SUPERVISE_SECS: u64 = 15;

static LANE_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9-]{0,23}$").unwrap());

/// The supervisor's memory of one manager (this run of MAP only).
#[derive(Debug, Clone, Default)]
struct Sup {
    failures: Vec<DateTime<Utc>>,
    /// Gave up: why.
    stopped: Option<String>,
    /// MAP started it (or found it running) in this run, so its disappearance is a failure.
    seen: bool,
    last_error: Option<String>,
}

#[derive(Default)]
pub struct Managers {
    sup: Mutex<HashMap<String, Sup>>,
    /// One start at a time per project (the loop and the API).
    starting: tokio::sync::Mutex<()>,
}

pub fn session_name(hub: &Hub, slug: &str) -> String {
    format!("{}mgr-{slug}", hub.config.workspace.session_prefix)
}

pub fn lane_session(hub: &Hub, slug: &str, name: &str) -> String {
    format!("{}lane-{slug}-{name}", hub.config.workspace.session_prefix)
}

/// The manager's instructions: `templates.manager_agent` when set, else the built-in ones.
pub fn instructions(hub: &Hub) -> Result<PlannerInstructions> {
    match &hub.config.templates.manager_agent {
        Some(path) => Ok(PlannerInstructions {
            source: path.display().to_string(),
            content: std::fs::read_to_string(path).with_context(|| format!("reading templates.manager_agent {}", path.display()))?,
        }),
        None => Ok(PlannerInstructions { source: "built-in".into(), content: MANAGER_AGENT.to_string() }),
    }
}

async fn project(hub: &Hub, slug: &str) -> Result<Project> {
    hub.store.project(slug).await?.with_context(|| format!("unknown project `{slug}`"))
}

/// A random-enough UUID (v4 layout) for a conversation MAP names itself.
fn new_uuid(seed: &str) -> String {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let d = Sha256::digest(format!("{seed}|{nanos}|{}|{n}", std::process::id()).as_bytes());
    let mut b: Vec<u8> = d[..16].to_vec();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

fn conversation_key(session: &str) -> String {
    format!("kept.conversation.{session}")
}

/// Whether the provider's state folder holds conversation `id` (Claude: `projects/<folder>/<id>.jsonl`).
fn conversation_exists(hub: &Hub, provider: &str, id: &str) -> bool {
    let Some(home) = hub.provider_home(provider) else { return false };
    let Ok(dirs) = std::fs::read_dir(home.join("projects")) else { return false };
    dirs.flatten().any(|d| d.path().join(format!("{id}.jsonl")).is_file())
}

/// The extra arguments for a kept session: resume its last conversation when the provider can and it exists, else
/// name a new one. Returns (args, resumed).
async fn conversation_args(hub: &Hub, provider: &str, session: &str) -> Result<(Vec<String>, bool)> {
    let spec = hub.spec(provider)?;
    let key = conversation_key(session);
    if !spec.resume_args.is_empty()
        && let Some(id) = hub.store.runtime_setting(&key).await?
        && conversation_exists(hub, provider, &id)
    {
        return Ok((spec.resume_args.iter().map(|a| a.replace("{session_id}", &id)).collect(), true));
    }
    if spec.session_id_args.is_empty() {
        return Ok((vec![], false));
    }
    let id = new_uuid(session);
    hub.store.set_runtime_setting(&key, Some(&id)).await?;
    Ok((spec.session_id_args.iter().map(|a| a.replace("{session_id}", &id)).collect(), false))
}

/// The provider for a kept session: the one asked for (installed), else the planner's choice for the project.
fn pick_provider(hub: &Hub, project: &Project, asked: Option<&str>) -> Result<String> {
    match asked.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) => {
            hub.spec(p)?;
            if !hub.provider_available(p) {
                bail!("{p} is not found on PATH; pick another provider or install it");
            }
            Ok(p.to_string())
        }
        None => crate::plans::provider_choices(hub, Some(project)).into_iter().next().ok_or_else(|| anyhow!("no provider is installed and signed in")),
    }
}

// ---- the manager's prompt

/// What the manager is told about this run, after its instructions.
pub fn prompt(hub: &Hub, instructions: &str, project: &Project, folder: &Path, ctx: &ManagerContext) -> String {
    let mut p = instructions.trim_end().to_string();
    p.push_str("\n\n## This session\n\n");
    p.push_str(&format!("- Project: {} (`{}`, `$MAP_PROJECT`).\n", project.name, project.slug));
    if !project.description.trim().is_empty() {
        p.push_str(&format!("- Its description: {}\n", project.description.trim()));
    }
    p.push_str(&format!("- Folder (the current directory): `{}`.\n", folder.display()));
    p.push_str(&format!("- The API: `$MAP_API_URL` = `{}`; this project: `{}/projects/{}`.\n", hub.local_api_url(), hub.local_api_url(), project.slug));
    p.push_str(if folder.join("tasks/README.md").is_file() {
        "- Task cards: this folder keeps cards in `tasks/` (see `tasks/README.md`); MAP's tasks are separate from them.\n"
    } else {
        "- Task cards: none in this folder.\n"
    });
    if ctx.providers.is_empty() {
        p.push_str("- Providers: none installed.\n");
    } else {
        let list: Vec<String> = ctx.providers.iter().map(|(n, m)| if m.is_empty() { format!("`{n}`") } else { format!("`{n}` ({})", m.join(", ")) }).collect();
        p.push_str(&format!("- Providers a task or lane may use (with their models): {}.\n", list.join(", ")));
    }
    p.push_str(&format!(
        "- Orchestrator: {}; the project is {}.\n",
        if ctx.orchestrator_running { "running" } else { "paused (approved tasks start once the owner presses Run)" },
        if project.settings.paused { "paused" } else { "not paused" }
    ));
    if ctx.open_tasks.is_empty() {
        p.push_str("- Open tasks: none.\n");
    } else {
        p.push_str(&format!("- Open tasks ({}): {}.\n", ctx.open_tasks.len(), ctx.open_tasks.iter().take(12).cloned().collect::<Vec<_>>().join(" · ")));
    }
    if !ctx.drafts.is_empty() {
        p.push_str(&format!("- Plans waiting for approval: {}.\n", ctx.drafts.join(" · ")));
    }
    if ctx.lanes.is_empty() {
        p.push_str("- Lanes: none.\n");
    } else {
        p.push_str(&format!("- Lanes: {}.\n", ctx.lanes.join(", ")));
    }
    p.push_str("\nGreet the owner in one line and wait for them.\n");
    p
}

/// Facts for [`prompt`].
#[derive(Debug, Default)]
pub struct ManagerContext {
    pub providers: Vec<(String, Vec<String>)>,
    pub orchestrator_running: bool,
    /// `#12 running Title`.
    pub open_tasks: Vec<String>,
    pub drafts: Vec<String>,
    pub lanes: Vec<String>,
}

async fn context(hub: &Hub, slug: &str) -> Result<ManagerContext> {
    let tasks = hub.store.tasks().await?;
    let open_tasks = tasks
        .iter()
        .filter(|t| t.project.as_deref() == Some(slug) && !matches!(t.status, TaskStatus::Done | TaskStatus::Cancelled))
        .map(|t| format!("#{} {} {}", t.id, t.status.as_str(), t.title))
        .collect();
    let drafts = hub.store.plans(Some(slug), 20).await?.into_iter().filter(|p| p.status == PlanStatus::Draft).map(|p| format!("#{} {}", p.id, p.title)).collect();
    let lanes = hub.lanes.configs().into_iter().filter(|c| c.owned.as_ref().is_some_and(|o| o.project == slug && o.kind == "lane")).map(|c| c.name).collect();
    let providers = hub.config.providers.iter().filter(|(n, _)| hub.provider_available(n)).map(|(n, s)| (n.clone(), s.models.clone())).collect();
    Ok(ManagerContext { providers, orchestrator_running: hub.orchestrator_running(), open_tasks, drafts, lanes })
}

/// What a restarted, resumed manager is told (its instructions are already in the conversation).
fn resume_prompt(hub: &Hub, slug: &str) -> String {
    format!(
        "MAP restarted this session; you are still the manager of project `{slug}`. Your instructions are unchanged (a copy is in {}). Do not greet again: wait for the owner.",
        hub.project_dir(slug).join("manager-agent.md").display()
    )
}

// ---- starting and supervising

/// The manager's env beyond what every project session gets.
fn manager_env(hub: &Hub, slug: &str) -> Vec<(String, String)> {
    vec![
        ("MAP_MANAGER_TOKEN".into(), hub.manager_token(slug).unwrap_or_default()),
        ("MAP_MANAGER_URL".into(), format!("{}/projects/{slug}", hub.local_api_url())),
    ]
}

/// Starts the manager of `slug` (or returns it while it runs): interactive, supervisor role, no task, in the
/// project's folder. Resumes its conversation when it can.
pub async fn start(hub: &Arc<Hub>, slug: &str) -> Result<Session> {
    let _one = hub.managers.starting.lock().await;
    let p = project(hub, slug).await?;
    let name = session_name(hub, slug);
    if let Some(existing) = hub.store.session(&name).await?
        && existing.state.is_live()
        && hub.term.exists(&name).await?
    {
        return Ok(existing);
    }
    let settings = p.settings.manager.clone();
    let provider = pick_provider(hub, &p, settings.provider.as_deref())?;
    let spec = hub.spec(&provider)?;
    Hub::resolve_model(spec, settings.model.as_deref())?;
    let (args, resumed) = conversation_args(hub, &provider, &name).await?;
    let folder = hub.work_dir(Some(slug)).await;
    let text = if resumed {
        resume_prompt(hub, slug)
    } else {
        let i = instructions(hub)?;
        let ctx = context(hub, slug).await?;
        let full = prompt(hub, &i.content, &p, &folder, &ctx);
        let dir = hub.project_dir(slug);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("manager-agent.md"), &full);
        full
    };
    let req = LaunchRequest { name: Some(name.clone()), provider: provider.clone(), model: settings.model.clone(), role: Role::Supervisor, task_id: None, project: Some(slug.to_string()), prompt: Some(text) };
    let session = hub.launch_args(req, &manager_env(hub, slug), &args).await?;
    hub.event(
        Level::Info,
        "manager.started",
        Some(&name),
        None,
        format!("Manager for {slug} started on {provider} ({}){}", session.model, if resumed { ", resuming its conversation" } else { "" }),
    )
    .await;
    Ok(session)
}

/// Clears the back-off (the owner pressed Start, or changed the settings).
fn reset(hub: &Hub, slug: &str) {
    hub.managers.sup.lock().unwrap().remove(slug);
}

/// The owner turns the manager on or off, or picks its provider and model. Turning it off closes the session
/// (refused while someone is attached or typing, unless `force`).
pub async fn configure(hub: &Arc<Hub>, slug: &str, settings: ManagerSettings, force: bool) -> Result<ManagerInfo> {
    let mut p = project(hub, slug).await?;
    let provider = settings.provider.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    let model = settings.model.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    if let Some(name) = &provider {
        let spec = hub.spec(name)?;
        Hub::resolve_model(spec, model.as_deref())?;
    }
    let before = p.settings.manager.clone();
    let next = ManagerSettings { enabled: settings.enabled, provider, model };
    let name = session_name(hub, slug);
    if before.enabled && !next.enabled && hub.term.exists(&name).await? && hub.store.session(&name).await?.is_some() {
        match hub.close_session(&name, force, "Manager turned off").await? {
            EndDecision::End => {}
            EndDecision::KeepAttached => bail!("a terminal is attached to the manager; detach it or turn it off with force"),
            EndDecision::KeepInput => bail!("someone has typed on the manager's input line; turn it off with force"),
        }
    }
    let changed_run = before.provider != next.provider || before.model != next.model;
    p.settings.manager = next.clone();
    hub.put_project(&p).await?;
    reset(hub, slug);
    sync_lanes(hub).await;
    if next.enabled {
        // A new provider or model takes effect on the next start: end the running one only when nobody uses it.
        if changed_run && hub.term.exists(&name).await? && hub.store.session(&name).await?.is_some() {
            let _ = hub.close_session(&name, false, "Manager settings changed").await;
        }
        if let Err(err) = start(hub, slug).await {
            hub.event(Level::Error, "manager.failed", Some(&name), None, format!("Manager for {slug} did not start: {err:#}")).await;
        }
    }
    hub.event(Level::Info, "manager.settings", Some(&name), None, format!("Manager for {slug} {}", if next.enabled { "on" } else { "off" })).await;
    info(hub, slug).await
}

/// `POST …/manager/start`: start now, forgetting the back-off.
pub async fn start_now(hub: &Arc<Hub>, slug: &str) -> Result<Session> {
    reset(hub, slug);
    let s = start(hub, slug).await?;
    hub.managers.sup.lock().unwrap().entry(slug.to_string()).or_default().seen = true;
    Ok(s)
}

pub async fn info(hub: &Hub, slug: &str) -> Result<ManagerInfo> {
    let p = project(hub, slug).await?;
    let name = session_name(hub, slug);
    let session = hub.store.session(&name).await?;
    let running = session.as_ref().is_some_and(|s| s.state.is_live()) && hub.term.exists(&name).await.unwrap_or(false);
    let sup = hub.managers.sup.lock().unwrap().get(slug).cloned().unwrap_or_default();
    let settings = p.settings.manager.clone();
    let state = if !settings.enabled {
        if running { "running" } else { "off" }
    } else if sup.stopped.is_some() {
        "stopped"
    } else if running {
        match session.as_ref().map(|s| s.state) {
            Some(crate::model::SessionState::Starting) => "starting",
            _ => "running",
        }
    } else if sup.last_error.is_some() {
        "failed"
    } else {
        "restarting"
    };
    let default_provider = crate::plans::provider_choices(hub, Some(&p)).into_iter().next();
    let provider = settings.provider.clone().or(default_provider.clone());
    let resumes = provider.as_deref().and_then(|n| hub.spec(n).ok()).is_some_and(|s| !s.resume_args.is_empty());
    let instructions_source = hub.config.templates.manager_agent.as_ref().map_or_else(|| "built-in".to_string(), |p| p.display().to_string());
    Ok(ManagerInfo {
        project: slug.into(),
        session_name: name,
        settings,
        default_provider,
        state: state.into(),
        warning: sup.stopped.or(sup.last_error),
        session,
        instructions_source,
        resumes,
    })
}

/// One supervisor round: every enabled manager whose session is gone is started again, unless it failed
/// [`MAX_FAILURES`] times within [`FAIL_WINDOW_MINS`].
pub async fn supervise(hub: &Arc<Hub>, now: DateTime<Utc>) {
    let Ok(projects) = hub.store.projects().await else { return };
    for p in projects.into_iter().filter(|p| p.settings.manager.enabled) {
        let slug = p.slug.clone();
        let name = session_name(hub, &slug);
        let alive = hub.term.exists(&name).await.unwrap_or(false) && hub.store.session(&name).await.ok().flatten().is_some_and(|s| s.state.is_live());
        let gone_after_seen = {
            let mut map = hub.managers.sup.lock().unwrap();
            let sup = map.entry(slug.clone()).or_default();
            if sup.stopped.is_some() {
                continue;
            }
            if alive {
                sup.seen = true;
                sup.last_error = None;
                continue;
            }
            sup.seen
        };
        if gone_after_seen && fail(hub, &slug, now, "its session ended").await {
            continue;
        }
        match start(hub, &slug).await {
            Ok(_) => {
                let mut map = hub.managers.sup.lock().unwrap();
                let sup = map.entry(slug.clone()).or_default();
                sup.seen = true;
                sup.last_error = None;
            }
            Err(err) => {
                let why = format!("{err:#}");
                hub.managers.sup.lock().unwrap().entry(slug.clone()).or_default().last_error = Some(why.clone());
                hub.event(Level::Error, "manager.failed", Some(&name), None, format!("Manager for {slug} did not start: {why}")).await;
                // A start that fails counts; the next round tries again (unless that made three).
                hub.managers.sup.lock().unwrap().entry(slug.clone()).or_default().seen = false;
                fail(hub, &slug, now, "it did not start").await;
            }
        }
    }
}

/// Records a failure; true when that stops the restarts (and the warning went out).
async fn fail(hub: &Hub, slug: &str, now: DateTime<Utc>, why: &str) -> bool {
    let stop = {
        let mut map = hub.managers.sup.lock().unwrap();
        let sup = map.entry(slug.to_string()).or_default();
        sup.failures.retain(|t| now - *t < Duration::minutes(FAIL_WINDOW_MINS));
        sup.failures.push(now);
        if sup.failures.len() >= MAX_FAILURES {
            sup.stopped = Some(format!("{MAX_FAILURES} failures within {FAIL_WINDOW_MINS} min (last: {why}); MAP stopped restarting it — press Start"));
            true
        } else {
            false
        }
    };
    let name = session_name(hub, slug);
    if stop {
        hub.event(Level::Warning, "manager.stopped", Some(&name), None, format!("Manager for {slug}: {MAX_FAILURES} failures within {FAIL_WINDOW_MINS} min ({why}); restarts stopped")).await;
    } else {
        hub.event(Level::Warning, "manager.restarting", Some(&name), None, format!("Manager for {slug}: {why}; starting it again")).await;
    }
    stop
}

/// Boot: register the owned lanes, then supervise for ever.
pub async fn run(hub: Arc<Hub>) {
    sync_lanes(&hub).await;
    loop {
        supervise(&hub, Utc::now()).await;
        tokio::time::sleep(StdDuration::from_secs(SUPERVISE_SECS)).await;
    }
}

// ---- owned lanes

/// The `[[lanes]]` block that watches this folder, if any: its rules are the project's lane rules.
fn rules_for(hub: &Hub, folder: &Path) -> Option<LaneConfig> {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let f = canon(folder);
    hub.lanes.base_configs().into_iter().find(|c| c.path.as_deref().is_some_and(|p| canon(p) == f))
}

fn owned_config(hub: &Hub, name: &str, slug: &str, kind: &str, folder: &Path, provider: &str) -> LaneConfig {
    let base = rules_for(hub, folder);
    let mut c = LaneConfig {
        name: name.to_string(),
        socket: String::new(),
        session_match: vec![name.to_string()],
        path: Some(folder.to_path_buf()),
        status_file: base.as_ref().map_or(".scratch/agent-status.json".into(), |b| b.status_file.clone()),
        tasks_dir: base.as_ref().map_or("tasks".into(), |b| b.tasks_dir.clone()),
        actions: true,
        watch: true,
        status_prompt: base.as_ref().map_or_else(String::new, |b| b.status_prompt.clone()),
        status_prompt_file: None,
        clear_when: base.as_ref().map_or_else(LaneClearWhen::default, |b| b.clear_when.clone()),
        ask_when: base.as_ref().map_or_else(LaneAskWhen::default, |b| b.ask_when.clone()),
        stall_when: base.as_ref().map_or_else(Default::default, |b| b.stall_when.clone()),
        // The provider's own state folder (MAP's, in isolated mode): where its transcripts are.
        claude_dir: Some(hub.provider_home(provider).unwrap_or_else(|| PathBuf::from("/nonexistent"))),
        context_window: base.as_ref().map_or(200_000, |b| b.context_window),
        owned: Some(OwnedLane { project: slug.to_string(), kind: kind.to_string() }),
    };
    if kind == "manager" {
        // The manager talks to the owner: never cleared, never nudged with a status prompt.
        c.clear_when.enabled = false;
        c.ask_when.idle_stale_mins = 0;
        c.status_prompt = String::new();
    }
    c
}

/// Rebuilds the MAP-owned lanes: every enabled manager and every stored persistent lane.
pub async fn sync_lanes(hub: &Hub) {
    let mut out = vec![];
    let projects = hub.store.projects().await.unwrap_or_default();
    for p in &projects {
        let folder = hub.workspace_of(p);
        if p.settings.manager.enabled {
            let provider = p.settings.manager.provider.clone().or_else(|| crate::plans::provider_choices(hub, Some(p)).into_iter().next()).unwrap_or_else(|| "claude".into());
            out.push(owned_config(hub, &session_name(hub, &p.slug), &p.slug, "manager", &folder, &provider));
        }
    }
    for r in hub.store.owned_lanes(None).await.unwrap_or_default() {
        let Some(p) = projects.iter().find(|p| p.slug == r.project) else { continue };
        let mut c = owned_config(hub, &r.lane, &r.project, "lane", &hub.workspace_of(p), &r.provider);
        if let Some(cw) = &r.clear_when {
            c.clear_when = cw.clone();
        }
        if !r.status_prompt.is_empty() {
            c.status_prompt = r.status_prompt.clone();
        }
        out.push(c);
    }
    hub.lanes.set_owned(out);
}

/// `name` given to the API: the short name or the full lane name, of this project.
pub async fn find_lane(hub: &Hub, slug: &str, name: &str) -> Result<Option<OwnedLaneRecord>> {
    let all = hub.store.owned_lanes(Some(slug)).await?;
    Ok(all.into_iter().find(|r| r.name == name || r.lane == name))
}

/// Opens a persistent lane: a MAP-owned interactive session in the project's folder, stored and watched.
pub async fn open_lane(hub: &Arc<Hub>, slug: &str, body: OpenLane, by: &str) -> Result<OwnedLaneRecord> {
    let p = project(hub, slug).await?;
    let short = body.name.trim().to_string();
    if !LANE_NAME.is_match(&short) {
        bail!("a lane name is 1–24 letters, digits or -");
    }
    let lane = lane_session(hub, slug, &short);
    if find_lane(hub, slug, &short).await?.is_some() || hub.lanes.config(&lane).is_some() {
        bail!("lane `{short}` already exists in {slug}");
    }
    let provider = pick_provider(hub, &p, body.provider.as_deref())?;
    let model = body.model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    Hub::resolve_model(hub.spec(&provider)?, model.as_deref())?;
    let rec = OwnedLaneRecord {
        lane: lane.clone(),
        project: slug.into(),
        name: short.clone(),
        provider,
        model,
        prompt: body.prompt.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
        status_prompt: body.status_prompt.map(|s| s.trim().to_string()).unwrap_or_default(),
        clear_when: body.clear_when,
        opened_by: by.into(),
        created_at: Utc::now(),
    };
    let mut session = launch_lane(hub, &rec, false).await?;
    if by == "manager" {
        session.spawned_by = format!("manager:{slug}");
        hub.put_session(&session).await?;
    }
    hub.store.put_owned_lane(&rec).await?;
    sync_lanes(hub).await;
    hub.event(Level::Info, "lane.opened", Some(&lane), None, format!("Lane {short} opened in {slug} on {} (by {by})", rec.provider)).await;
    Ok(rec)
}

async fn launch_lane(hub: &Arc<Hub>, rec: &OwnedLaneRecord, restart: bool) -> Result<Session> {
    let (args, resumed) = conversation_args(hub, &rec.provider, &rec.lane).await?;
    // A resumed conversation has its first prompt already; a fresh restart gets it again.
    let prompt = if resumed { None } else { rec.prompt.clone() };
    let req = LaunchRequest { name: Some(rec.lane.clone()), provider: rec.provider.clone(), model: rec.model.clone(), role: Role::Manual, task_id: None, project: Some(rec.project.clone()), prompt };
    let s = hub.launch_args(req, &[], &args).await?;
    if restart {
        hub.event(Level::Info, "lane.started", Some(&rec.lane), None, format!("Lane {} started again{}", rec.name, if resumed { ", resuming its conversation" } else { "" })).await;
    }
    Ok(s)
}

/// Starts a stopped persistent lane (its session is gone).
pub async fn start_lane(hub: &Arc<Hub>, slug: &str, name: &str) -> Result<Session> {
    let rec = find_lane(hub, slug, name).await?.with_context(|| format!("no lane `{name}` in {slug}"))?;
    if hub.term.exists(&rec.lane).await? {
        return hub.managed(&rec.lane).await;
    }
    launch_lane(hub, &rec, true).await
}

/// Closes a persistent lane: ends its session (refused while someone is attached or typing, unless `force`) and
/// forgets it.
pub async fn close_lane(hub: &Hub, slug: &str, name: &str, force: bool) -> Result<EndDecision> {
    let rec = find_lane(hub, slug, name).await?.with_context(|| format!("no lane `{name}` in {slug}"))?;
    if hub.term.exists(&rec.lane).await? && hub.store.session(&rec.lane).await?.is_some() {
        let d = hub.close_session(&rec.lane, force, "Lane closed").await?;
        if d != EndDecision::End {
            return Ok(d);
        }
    } else if hub.term.exists(&rec.lane).await? {
        hub.term.kill(&rec.lane).await?;
    }
    hub.store.delete_owned_lane(&rec.lane).await?;
    sync_lanes(hub).await;
    hub.event(Level::Info, "lane.closed", Some(&rec.lane), None, format!("Lane {} closed in {slug}", rec.name)).await;
    Ok(EndDecision::End)
}

/// A deleted project's lanes are forgotten (their sessions, if any, stay until closed).
pub async fn forget_project(hub: &Hub, slug: &str) {
    for r in hub.store.owned_lanes(Some(slug)).await.unwrap_or_default() {
        let _ = hub.store.delete_owned_lane(&r.lane).await;
    }
    hub.managers.sup.lock().unwrap().remove(slug);
    sync_lanes(hub).await;
}

/// `GET /api/projects/{slug}/overview`.
pub async fn overview(hub: &Hub, slug: &str) -> Result<ProjectOverview> {
    let p = project(hub, slug).await?;
    let mut tasks: Vec<_> = hub.store.tasks().await?.into_iter().filter(|t| t.project.as_deref() == Some(slug)).collect();
    tasks.sort_by_key(|t| std::cmp::Reverse(t.id));
    let mut counts = std::collections::BTreeMap::new();
    for t in &tasks {
        *counts.entry(t.status.as_str().to_string()).or_insert(0u32) += 1;
    }
    let open_tasks = tasks.iter().filter(|t| !matches!(t.status, TaskStatus::Done | TaskStatus::Cancelled)).take(50).cloned().collect();
    let recent_done = tasks.iter().filter(|t| t.status == TaskStatus::Done).take(10).cloned().collect();
    let draft_plans = hub.store.plans(Some(slug), 50).await?.into_iter().filter(|p| p.status == PlanStatus::Draft).collect();
    let sessions = hub.store.sessions().await?.into_iter().filter(|s| s.project.as_deref() == Some(slug)).collect();
    let lanes = project_lanes(hub, slug).await?;
    Ok(ProjectOverview {
        project: hub.project_info(p).await?,
        orchestrator: hub.orchestrator_status().await,
        counts,
        open_tasks,
        recent_done,
        draft_plans,
        sessions,
        lanes,
        manager: info(hub, slug).await?,
    })
}

/// The project's lanes: MAP-owned ones and `[[lanes]]` blocks watching its folder.
pub async fn project_lanes(hub: &Hub, slug: &str) -> Result<Vec<crate::model::LaneInfo>> {
    let p = project(hub, slug).await?;
    let folder = std::fs::canonicalize(hub.workspace_of(&p)).unwrap_or_else(|_| hub.workspace_of(&p));
    let now = Utc::now();
    let mut out = vec![];
    for cfg in hub.lanes.configs() {
        let mine = match &cfg.owned {
            Some(o) => o.project == slug,
            None => cfg.path.as_deref().is_some_and(|x| std::fs::canonicalize(x).unwrap_or_else(|_| x.to_path_buf()) == folder),
        };
        if mine {
            out.push(hub.lanes.info(hub, &cfg, now).await?);
        }
    }
    Ok(out)
}

/// A `[[lanes]]` block (someone else's session) of this project, by name.
pub async fn is_project_lane(hub: &Hub, slug: &str, name: &str) -> bool {
    project_lanes(hub, slug).await.is_ok_and(|l| l.iter().any(|i| i.name == name))
}

#[cfg(test)]
mod tests;
