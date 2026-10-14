//! Requirements → plan → tasks. Every project (and the default workspace) has a built-in planner agent the owner
//! talks to in an interactive session (`<prefix>plan-<slug>`). It turns a requirement into a draft plan of tasks and
//! submits it here; nothing runs until the owner approves the draft, which creates the real tasks in dependency
//! order for the orchestrator. Drafts can be edited, rejected, or replaced by a new revision.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result, anyhow, bail};
use regex::Regex;

use crate::hub::{Hub, LaunchRequest};
use crate::model::{Level, Plan, PlanApproved, PlanEdit, PlanStatus, PlanSubmit, PlanTask, PlanTaskInput, PlannerInfo, PlannerInstructions, Project, Role, Session};
use crate::store::{NewPlan, NewTask};

/// The planner's instructions shipped with MAP; `templates.planner_agent` in `map.toml` replaces them.
pub const PLANNER_AGENT: &str = include_str!("../templates/planner-agent.md");
/// Most tasks in one plan.
pub const MAX_TASKS: usize = 50;
const MAX_TITLE: usize = 200;
const MAX_PROMPT: usize = 20_000;
/// Lines that wrap the part of the instructions about `tasks/` cards, kept only for projects that use them.
const CARDS_START: &str = "<!-- cards -->";
const CARDS_END: &str = "<!-- /cards -->";

static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,32}$").unwrap());

/// Why a plan request failed, as the API answers it.
#[derive(Debug)]
pub enum PlanError {
    NotFound(String),
    /// The plan is no longer a draft (approved twice, edited after a decision…).
    Conflict(String),
    Invalid(anyhow::Error),
}

impl From<anyhow::Error> for PlanError {
    fn from(err: anyhow::Error) -> Self {
        PlanError::Invalid(err)
    }
}

type PlanResult<T> = std::result::Result<T, PlanError>;

fn clean(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Checks a plan's tasks and returns them trimmed: keys unique and well-formed, a title and a prompt each,
/// dependencies on other keys of the plan without cycles, providers and models MAP knows, at most [`MAX_TASKS`].
pub fn validate_tasks(hub: &Hub, tasks: Vec<PlanTaskInput>) -> Result<Vec<PlanTask>> {
    if tasks.is_empty() {
        bail!("a plan needs at least one task");
    }
    if tasks.len() > MAX_TASKS {
        bail!("a plan has at most {MAX_TASKS} tasks ({} sent); split the requirement into several plans", tasks.len());
    }
    let mut out: Vec<PlanTask> = Vec::with_capacity(tasks.len());
    let mut keys = HashSet::new();
    for t in tasks {
        let key = t.key.trim().to_string();
        if !KEY.is_match(&key) {
            bail!("task key `{key}` must be 1–32 letters, digits, _ or -");
        }
        if !keys.insert(key.clone()) {
            bail!("task key `{key}` is used twice");
        }
        let title = t.title.trim().to_string();
        if title.is_empty() {
            bail!("task {key} needs a title");
        }
        if title.chars().count() > MAX_TITLE {
            bail!("task {key}: the title is longer than {MAX_TITLE} characters");
        }
        if t.prompt.trim().is_empty() {
            bail!("task {key} needs a prompt");
        }
        if t.prompt.chars().count() > MAX_PROMPT {
            bail!("task {key}: the prompt is longer than {MAX_PROMPT} characters");
        }
        let provider = clean(t.provider);
        let model = clean(t.model);
        if let Some(p) = &provider {
            let spec = hub.spec(p).with_context(|| format!("task {key}"))?;
            if model.is_some() {
                Hub::resolve_model(spec, model.as_deref()).with_context(|| format!("task {key}"))?;
            }
        }
        let mut depends_on: Vec<String> = vec![];
        for d in t.depends_on.unwrap_or_default() {
            let d = d.trim().to_string();
            if !depends_on.contains(&d) {
                depends_on.push(d);
            }
        }
        out.push(PlanTask { key, title, prompt: t.prompt.trim().to_string(), depends_on, provider, model, task_id: None });
    }
    for t in &out {
        for d in &t.depends_on {
            if *d == t.key {
                bail!("task {} depends on itself", t.key);
            }
            if !keys.contains(d) {
                bail!("task {} depends on `{d}`, which is not a task of this plan", t.key);
            }
        }
    }
    order(&out)?;
    Ok(out)
}

/// The tasks' indexes in an order where every task comes after its dependencies (ties: as submitted), or an error
/// naming the tasks caught in a cycle.
pub fn order(tasks: &[PlanTask]) -> Result<Vec<usize>> {
    let index: HashMap<&str, usize> = tasks.iter().enumerate().map(|(i, t)| (t.key.as_str(), i)).collect();
    let mut placed = vec![false; tasks.len()];
    let mut out = Vec::with_capacity(tasks.len());
    while out.len() < tasks.len() {
        let next = (0..tasks.len()).find(|&i| !placed[i] && tasks[i].depends_on.iter().all(|d| index.get(d.as_str()).is_some_and(|&j| placed[j])));
        let Some(i) = next else {
            let stuck: Vec<&str> = (0..tasks.len()).filter(|&i| !placed[i]).map(|i| tasks[i].key.as_str()).collect();
            bail!("these tasks depend on each other in a cycle (or on one that does): {}", stuck.join(", "));
        };
        placed[i] = true;
        out.push(i);
    }
    Ok(out)
}

fn validate_title(title: &str) -> Result<String> {
    let title = title.trim();
    if title.is_empty() {
        bail!("a plan needs a title");
    }
    if title.chars().count() > MAX_TITLE {
        bail!("the plan's title is longer than {MAX_TITLE} characters");
    }
    Ok(title.to_string())
}

async fn known_project(hub: &Hub, project: Option<&str>) -> PlanResult<Option<Project>> {
    match project {
        None => Ok(None),
        Some(slug) => Ok(Some(hub.store.project(slug).await?.ok_or_else(|| PlanError::NotFound(format!("project `{slug}`")))?)),
    }
}

fn where_(project: Option<&str>) -> String {
    project.map_or_else(|| "the default workspace".to_string(), String::from)
}

/// Stores a new draft for `project` (`None`: the default workspace). With `replaces`, a draft of the same project
/// becomes `superseded` and the new plan is its next revision.
pub async fn submit(hub: &Hub, project: Option<&str>, body: PlanSubmit) -> PlanResult<Plan> {
    known_project(hub, project).await?;
    let title = validate_title(&body.title)?;
    let tasks = validate_tasks(hub, body.tasks)?;
    let mut revision = 1;
    let mut old = None;
    if let Some(id) = body.replaces {
        let prev = hub.store.plan(id).await?.filter(|p| p.project.as_deref() == project).ok_or_else(|| PlanError::NotFound(format!("plan #{id} of {}", where_(project))))?;
        if prev.status == PlanStatus::Approved {
            return Err(PlanError::Conflict(format!("plan #{id} is already approved and its tasks exist; submit a new plan without `replaces`")));
        }
        revision = prev.revision + 1;
        old = Some(prev);
    }
    // The submitting session, if it is one of this project's; else the planner's own.
    let mut session = None;
    for name in [clean(body.session), Some(session_name(hub, project))].into_iter().flatten() {
        if hub.store.session(&name).await?.is_some_and(|s| s.project.as_deref() == project) {
            session = Some(name);
            break;
        }
    }
    let new = NewPlan { project: project.map(String::from), title, summary: body.summary.unwrap_or_default().trim().to_string(), revision, replaces: body.replaces, session, tasks };
    let id = hub.store.insert_plan(&new).await?;
    if let Some(prev) = old
        && prev.status == PlanStatus::Draft
        && hub.store.set_plan_status(prev.id, PlanStatus::Draft, PlanStatus::Superseded, None).await?
        && let Some(p) = hub.store.plan(prev.id).await?
    {
        hub.announce_plan(&p);
        hub.event(Level::Info, "plan.superseded", p.session.as_deref(), None, format!("Plan #{} superseded by #{id} (revision {revision})", p.id)).await;
    }
    let plan = hub.store.plan(id).await?.context("plan vanished after insert")?;
    hub.announce_plan(&plan);
    let rev = if revision > 1 { format!(" · revision {revision}") } else { String::new() };
    hub.event(
        Level::Info,
        "plan.created",
        plan.session.as_deref(),
        None,
        format!("Plan #{id} for {}: {} · {} tasks{rev} · waiting for approval", where_(project), plan.title, plan.tasks.len()),
    )
    .await;
    Ok(plan)
}

async fn draft(hub: &Hub, id: i64) -> PlanResult<Plan> {
    let plan = hub.store.plan(id).await?.ok_or_else(|| PlanError::NotFound(format!("plan #{id}")))?;
    if plan.status != PlanStatus::Draft {
        return Err(PlanError::Conflict(format!("plan #{id} is {}; only a draft can change", plan.status.as_str())));
    }
    Ok(plan)
}

/// The owner changes a draft before approving it: title, summary, or the whole task list.
pub async fn edit(hub: &Hub, id: i64, body: PlanEdit) -> PlanResult<Plan> {
    let plan = draft(hub, id).await?;
    let title = match body.title {
        Some(t) => validate_title(&t)?,
        None => plan.title.clone(),
    };
    let summary = body.summary.map_or(plan.summary.clone(), |s| s.trim().to_string());
    let tasks = match body.tasks {
        Some(tasks) => validate_tasks(hub, tasks)?,
        None => plan.tasks.clone(),
    };
    if !hub.store.edit_plan(id, &title, &summary, &tasks).await? {
        return Err(PlanError::Conflict(format!("plan #{id} was decided meanwhile")));
    }
    let plan = hub.store.plan(id).await?.context("plan vanished")?;
    hub.announce_plan(&plan);
    hub.event(Level::Info, "plan.updated", plan.session.as_deref(), None, format!("Plan #{id} edited: {} · {} tasks", plan.title, plan.tasks.len())).await;
    Ok(plan)
}

/// Creates the draft's tasks in dependency order (each `depends_on` key mapped to the new task's id), in the plan's
/// project, and marks it approved. A second approval is a conflict and creates nothing.
pub async fn approve(hub: &Hub, id: i64) -> PlanResult<PlanApproved> {
    let plan = draft(hub, id).await?;
    known_project(hub, plan.project.as_deref()).await.map_err(|e| match e {
        PlanError::NotFound(what) => PlanError::Invalid(anyhow!("{what} no longer exists")),
        other => other,
    })?;
    // Providers may have left map.toml since the draft was stored.
    let inputs = plan.tasks.iter().map(|t| PlanTaskInput { key: t.key.clone(), title: t.title.clone(), prompt: t.prompt.clone(), depends_on: Some(t.depends_on.clone()), provider: t.provider.clone(), model: t.model.clone() });
    let tasks = validate_tasks(hub, inputs.collect())?;
    let sequence = order(&tasks)?;
    if !hub.store.set_plan_status(id, PlanStatus::Draft, PlanStatus::Approved, None).await? {
        return Err(PlanError::Conflict(format!("plan #{id} was decided meanwhile")));
    }
    let mut ids: HashMap<String, i64> = HashMap::new();
    let mut created = vec![];
    for i in sequence {
        let t = &tasks[i];
        let depends_on = t.depends_on.iter().map(|d| ids.get(d).copied().context("dependency created out of order")).collect::<Result<Vec<_>>>()?;
        let task = hub
            .store
            .create_task(NewTask { title: t.title.clone(), prompt: t.prompt.clone(), depends_on, provider: t.provider.clone(), model: t.model.clone(), project: plan.project.clone() })
            .await?;
        hub.store.set_plan_task_id(id, &t.key, task.id).await?;
        ids.insert(t.key.clone(), task.id);
        hub.announce_task(&task);
        hub.event(Level::Info, "task.created", None, Some(task.id), format!("Task #{} created: {} (plan #{id} · {})", task.id, task.title, t.key)).await;
        created.push(task);
    }
    let plan = hub.store.plan(id).await?.context("plan vanished")?;
    hub.announce_plan(&plan);
    let range = match (created.first(), created.last()) {
        (Some(a), Some(b)) if a.id != b.id => format!("#{}–#{}", a.id, b.id),
        (Some(a), _) => format!("#{}", a.id),
        _ => String::new(),
    };
    hub.event(Level::Success, "plan.approved", plan.session.as_deref(), None, format!("Plan #{id} approved: {} · {} tasks created {range}", plan.title, created.len())).await;
    Ok(PlanApproved { plan, tasks: created })
}

pub async fn reject(hub: &Hub, id: i64, reason: Option<String>) -> PlanResult<Plan> {
    draft(hub, id).await?;
    let reason = clean(reason);
    if !hub.store.set_plan_status(id, PlanStatus::Draft, PlanStatus::Rejected, reason.as_deref()).await? {
        return Err(PlanError::Conflict(format!("plan #{id} was decided meanwhile")));
    }
    let plan = hub.store.plan(id).await?.context("plan vanished")?;
    hub.announce_plan(&plan);
    let why = reason.map(|r| format!(": {r}")).unwrap_or_default();
    hub.event(Level::Warning, "plan.rejected", plan.session.as_deref(), None, format!("Plan #{id} rejected{why}")).await;
    Ok(plan)
}

// ---- the planner session

/// `<prefix>plan-<slug>`, or `<prefix>plan` for the default workspace.
pub fn session_name(hub: &Hub, project: Option<&str>) -> String {
    let prefix = &hub.config.workspace.session_prefix;
    match project {
        Some(slug) => format!("{prefix}plan-{slug}"),
        None => format!("{prefix}plan"),
    }
}

/// Where the planner submits and lists its plans.
pub fn plans_url(hub: &Hub, project: Option<&str>) -> String {
    match project {
        Some(slug) => format!("{}/projects/{slug}/plans", hub.local_api_url()),
        None => format!("{}/workspace/plans", hub.local_api_url()),
    }
}

/// The planner's instructions: `templates.planner_agent` when set, else the built-in ones.
pub fn instructions(hub: &Hub) -> Result<PlannerInstructions> {
    match &hub.config.templates.planner_agent {
        Some(path) => Ok(PlannerInstructions {
            source: path.display().to_string(),
            content: std::fs::read_to_string(path).with_context(|| format!("reading templates.planner_agent {}", path.display()))?,
        }),
        None => Ok(PlannerInstructions { source: "built-in".into(), content: PLANNER_AGENT.to_string() }),
    }
}

/// The instructions with the parts between `start` and `end` lines kept (markers dropped) or left out.
fn marked_parts(text: &str, keep: bool) -> String {
    let mut out = vec![];
    let mut inside = false;
    for line in text.lines() {
        match line.trim() {
            CARDS_START => inside = true,
            CARDS_END => inside = false,
            _ if inside && !keep => {}
            _ => out.push(line),
        }
    }
    out.join("\n")
}

/// The providers the planner starts on when none is picked, best first: the project's list, else `provider_order`,
/// then any other configured provider; installed ones only, signed in before sign-in unknown, never signed out.
fn provider_choices(hub: &Hub, project: Option<&Project>) -> Vec<String> {
    let mut names: Vec<String> = match project {
        Some(p) if !p.settings.providers.is_empty() => p.settings.providers.iter().map(|x| x.name.clone()).collect(),
        _ => hub.config.orchestrator.provider_order.clone(),
    };
    for name in hub.config.providers.keys() {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    let usable: Vec<(String, Option<bool>)> = names
        .into_iter()
        .filter_map(|n| {
            let spec = hub.config.providers.get(&n)?;
            hub.provider_available(&n).then(|| (n.clone(), crate::login::signed_in(hub, spec)))
        })
        .filter(|(_, signed)| *signed != Some(false))
        .collect();
    let mut out: Vec<String> = usable.iter().filter(|(_, s)| *s == Some(true)).map(|(n, _)| n.clone()).collect();
    out.extend(usable.into_iter().filter(|(_, s)| s.is_none()).map(|(n, _)| n));
    out
}

pub async fn info(hub: &Hub, project: Option<&str>) -> PlanResult<PlannerInfo> {
    let p = known_project(hub, project).await?;
    let name = session_name(hub, project);
    let instructions_source = match &hub.config.templates.planner_agent {
        Some(path) => path.display().to_string(),
        None => "built-in".into(),
    };
    Ok(PlannerInfo { session: hub.store.session(&name).await?, session_name: name, instructions_source, default_provider: provider_choices(hub, p.as_ref()).into_iter().next() })
}

/// The highest `NNN` card number under `tasks/` (todo, todo-human, done), when the folder uses task cards.
fn highest_card(folder: &Path) -> Option<u32> {
    let mut best = None;
    for sub in ["todo", "todo-human", "done"] {
        let Ok(entries) = std::fs::read_dir(folder.join("tasks").join(sub)) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let digits: String = name.chars().take_while(|c| c.is_ascii_digit()).collect();
            if digits.len() >= 3
                && let Ok(n) = digits.parse::<u32>()
            {
                best = Some(best.map_or(n, |b: u32| b.max(n)));
            }
        }
    }
    best
}

/// What the planner is told about this run, after its instructions.
pub struct PlannerContext<'a> {
    pub project: Option<&'a Project>,
    pub folder: &'a Path,
    pub session: &'a str,
    pub plans_url: &'a str,
    /// `tasks/README.md` exists: the project keeps task cards (team-process).
    pub cards: bool,
    pub highest_card: Option<u32>,
    /// (provider, models) of the installed providers.
    pub providers: Vec<(String, Vec<String>)>,
    /// Open drafts and recent decisions of this project, newest first.
    pub recent: &'a [Plan],
    pub orchestrator_running: bool,
}

pub fn prompt(instructions: &str, ctx: &PlannerContext) -> String {
    let mut p = marked_parts(instructions, ctx.cards).trim_end().to_string();
    p.push_str("\n\n## This session\n\n");
    match ctx.project {
        Some(project) => {
            p.push_str(&format!("- Project: {} (`{}`).\n", project.name, project.slug));
            if !project.description.trim().is_empty() {
                p.push_str(&format!("- Its description: {}\n", project.description.trim()));
            }
            if project.settings.paused {
                p.push_str("- The project is paused: approved tasks wait until the owner resumes it.\n");
            }
        }
        None => p.push_str("- Project: none, the default workspace (`workspace.dir` in map.toml). It has no Reflex, so there is no `$MAP_DECIDE_URL`.\n"),
    }
    p.push_str(&format!("- Folder (the current directory): `{}`.\n", ctx.folder.display()));
    p.push_str(&format!("- Your session: `{}` (`$MAP_SESSION`). Plans go to `$MAP_PLANS_URL` = `{}`.\n", ctx.session, ctx.plans_url));
    if ctx.cards {
        let next = ctx.highest_card.map_or(1, |n| n + 1);
        p.push_str(&format!("- Task cards: this folder has `tasks/README.md`; the next card number is {next:03} (check again before writing).\n"));
    } else {
        p.push_str("- Task cards: this folder has no `tasks/README.md`, so do not write cards; the plan's prompts carry everything.\n");
    }
    if ctx.providers.is_empty() {
        p.push_str("- Providers: none installed; leave `provider` out.\n");
    } else {
        let list: Vec<String> = ctx.providers.iter().map(|(n, m)| if m.is_empty() { format!("`{n}`") } else { format!("`{n}` ({})", m.join(", ")) }).collect();
        p.push_str(&format!("- Providers you may pin a task to (with their models): {}.\n", list.join(", ")));
    }
    if !ctx.orchestrator_running {
        p.push_str("- The orchestrator is paused right now: approved tasks start once the owner presses Run.\n");
    }
    if ctx.recent.is_empty() {
        p.push_str("- Plans so far: none.\n");
    } else {
        p.push_str("- Plans so far (newest first):\n");
        for plan in ctx.recent {
            let why = plan.reason.as_deref().map(|r| format!(" · rejected: {r}")).unwrap_or_default();
            p.push_str(&format!("  - #{} rev {} {} · {} · {} tasks{why}\n", plan.id, plan.revision, plan.status.as_str(), plan.title, plan.tasks.len()));
        }
    }
    p.push_str("\nRead this project now, then greet the owner and ask what they want.\n");
    p
}

/// Starts the planner session of `project` (or returns it while it runs): interactive, in the project's folder, with
/// the instructions as its first prompt, not attached to any task, so the orchestrator never treats it as a worker.
pub async fn start(hub: &Arc<Hub>, project: Option<&str>, provider: Option<String>, model: Option<String>) -> PlanResult<Session> {
    let p = known_project(hub, project).await?;
    let name = session_name(hub, project);
    if let Some(existing) = hub.store.session(&name).await?
        && existing.state.is_live()
        && hub.term.exists(&name).await?
    {
        return Ok(existing);
    }
    let provider = match clean(provider) {
        Some(provider) => {
            hub.spec(&provider)?;
            if !hub.provider_available(&provider) {
                return Err(PlanError::Invalid(anyhow!("{provider} is not found on PATH; pick another provider or install it")));
            }
            provider
        }
        None => provider_choices(hub, p.as_ref())
            .into_iter()
            .next()
            .ok_or_else(|| PlanError::Invalid(anyhow!("no provider is installed and signed in; sign one in, then talk to the planner")))?,
    };
    let model = clean(model);
    let spec = hub.spec(&provider)?;
    Hub::resolve_model(spec, model.as_deref())?;

    let instructions = instructions(hub)?;
    let folder = hub.work_dir(project).await;
    let cards = folder.join("tasks/README.md").is_file();
    let recent = hub.store.plans(Some(project.unwrap_or("")), 10).await?;
    let providers = hub.config.providers.iter().filter(|(n, _)| hub.provider_available(n)).map(|(n, s)| (n.clone(), s.models.clone())).collect();
    let url = plans_url(hub, project);
    let ctx = PlannerContext {
        project: p.as_ref(),
        folder: &folder,
        session: &name,
        plans_url: &url,
        cards,
        highest_card: if cards { highest_card(&folder) } else { None },
        providers,
        recent: &recent,
        orchestrator_running: hub.orchestrator_running(),
    };
    let text = prompt(&instructions.content, &ctx);

    let mut env = vec![("MAP_PLANS_URL".to_string(), url)];
    if project.is_none() {
        // Project sessions get these from hub.launch; the default workspace's token opens only its plans.
        env.push(("MAP_API_URL".into(), hub.local_api_url()));
        env.push(("MAP_PROJECT_TOKEN".into(), hub.workspace_token().unwrap_or_default()));
    }
    let req = LaunchRequest { name: Some(name.clone()), provider: provider.clone(), model, role: Role::Supervisor, task_id: None, project: project.map(String::from), prompt: Some(text) };
    let session = hub.launch_env(req, &env).await?;
    hub.event(Level::Info, "planner.started", Some(&name), None, format!("Planner for {} started on {provider} ({})", where_(project), session.model)).await;
    Ok(session)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use crate::config::Config;
    use crate::model::{ProjectSettings, ProjectSource, SourceKind, TaskStatus};
    use crate::projects::{NewProject, create_with};
    use crate::store::Store;
    use crate::terminal::fake::FakeTerminal;

    /// `claude` counts as installed (`/bin/sh`), `codex` and `gemini` do not; `api_token = "secret"`; `extra` goes in
    /// `[templates]`.
    pub(crate) async fn hub(extra: &str) -> (Arc<Hub>, Arc<FakeTerminal>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\napi_token = \"secret\"\n[orchestrator]\nprovider_order = [\"codex\", \"claude\"]\n[reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"/bin/sh\"\ncredentials = []\nmodels = [\"opus\", \"sonnet\"]\ndefault_model = \"sonnet\"\n[providers.codex]\nbinary = \"/nonexistent/codex\"\n[providers.gemini]\nbinary = \"/nonexistent/agy\"\n[templates]\ndirs = []\n{extra}\n",
            dir.path().display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        let term = Arc::new(FakeTerminal::default());
        (Hub::new(config, Store::memory().await.unwrap(), term.clone()), term, dir)
    }

    pub(crate) async fn project(hub: &Hub, folder: &Path, name: &str) -> String {
        std::fs::create_dir_all(folder).unwrap();
        let settings = ProjectSettings { source: ProjectSource { kind: SourceKind::Local, location: Some(folder.display().to_string()), ..Default::default() }, ..Default::default() };
        let new = NewProject { name: name.into(), slug: None, description: "A booking app.".into(), fallback: Some("none".into()), model: None };
        create_with(hub, new, settings).await.unwrap().0.slug
    }

    fn task(key: &str, deps: &[&str]) -> PlanTaskInput {
        PlanTaskInput { key: key.into(), title: format!("Do {key}"), prompt: format!("Goal: {key}."), depends_on: Some(deps.iter().map(|d| d.to_string()).collect()), provider: None, model: None }
    }

    fn submit_body(tasks: Vec<PlanTaskInput>) -> PlanSubmit {
        PlanSubmit { title: "Bookings".into(), summary: Some("Restated.".into()), tasks, replaces: None, session: None }
    }

    fn invalid(r: PlanResult<Plan>) -> String {
        match r {
            Err(PlanError::Invalid(e)) => format!("{e:#}"),
            Err(other) => panic!("expected invalid, got {other:?}"),
            Ok(p) => panic!("expected invalid, got plan #{}", p.id),
        }
    }

    #[tokio::test]
    async fn validation_refuses_bad_plans() {
        let (hub, _, _dir) = hub("").await;
        let cycle = invalid(submit(&hub, None, submit_body(vec![task("t1", &["t3"]), task("t2", &["t1"]), task("t3", &["t2"]), task("t4", &[])])).await);
        assert!(cycle.contains("cycle") && cycle.contains("t1, t2, t3") && !cycle.contains("t4"), "{cycle}");
        assert!(invalid(submit(&hub, None, submit_body(vec![task("t1", &["t1"])])).await).contains("depends on itself"));
        assert!(invalid(submit(&hub, None, submit_body(vec![task("t1", &["t9"])])).await).contains("`t9`, which is not a task of this plan"));
        assert!(invalid(submit(&hub, None, submit_body(vec![task("t1", &[]), task("t1", &[])])).await).contains("used twice"));
        assert!(invalid(submit(&hub, None, submit_body(vec![task("bad key", &[])])).await).contains("must be 1–32"));
        let mut empty = task("t1", &[]);
        empty.prompt = "  \n ".into();
        assert!(invalid(submit(&hub, None, submit_body(vec![empty])).await).contains("needs a prompt"));
        let mut untitled = task("t1", &[]);
        untitled.title = " ".into();
        assert!(invalid(submit(&hub, None, submit_body(vec![untitled])).await).contains("needs a title"));
        let many: Vec<PlanTaskInput> = (0..=MAX_TASKS).map(|i| task(&format!("t{i}"), &[])).collect();
        assert!(invalid(submit(&hub, None, submit_body(many)).await).contains("at most 50"));
        assert!(invalid(submit(&hub, None, submit_body(vec![])).await).contains("at least one"));
        let mut unknown = task("t1", &[]);
        unknown.provider = Some("nope".into());
        assert!(invalid(submit(&hub, None, submit_body(vec![unknown])).await).contains("unknown provider"));
        let mut bad_model = task("t1", &[]);
        (bad_model.provider, bad_model.model) = (Some("claude".into()), Some("gpt-9".into()));
        assert!(invalid(submit(&hub, None, submit_body(vec![bad_model])).await).contains("not in this provider's list"));
        let mut no_title = submit_body(vec![task("t1", &[])]);
        no_title.title = "".into();
        assert!(invalid(submit(&hub, None, no_title).await).contains("needs a title"));
        assert!(hub.store.plans(None, 10).await.unwrap().is_empty(), "nothing invalid is stored");
        // 50 tasks is fine; duplicate dependencies are folded.
        let fifty: Vec<PlanTaskInput> = (0..MAX_TASKS).map(|i| task(&format!("t{i}"), if i > 0 { &["t0", "t0"] } else { &[] })).collect();
        let plan = submit(&hub, None, submit_body(fifty)).await.unwrap();
        assert_eq!((plan.tasks.len(), plan.tasks[1].depends_on.clone()), (50, vec!["t0".to_string()]));
    }

    #[tokio::test]
    async fn approve_creates_tasks_in_dependency_order_once() {
        let (hub, _, dir) = hub("").await;
        let slug = project(&hub, &dir.path().join("app"), "App").await;
        let mut pinned = task("ui", &["api", "db"]);
        (pinned.provider, pinned.model) = (Some("claude".into()), Some("opus".into()));
        // Submitted out of order: the screen first, then what it needs.
        let plan = submit(&hub, Some(&slug), submit_body(vec![pinned, task("api", &["db"]), task("db", &[]), task("docs", &[])])).await.unwrap();
        assert_eq!((plan.status, plan.revision, plan.project.as_deref()), (PlanStatus::Draft, 1, Some(slug.as_str())));
        assert!(hub.store.tasks().await.unwrap().is_empty(), "a draft creates no task");

        let approved = approve(&hub, plan.id).await.unwrap();
        let titles: Vec<&str> = approved.tasks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, vec!["Do db", "Do api", "Do ui", "Do docs"], "dependencies first, else as submitted");
        let by = |title: &str| approved.tasks.iter().find(|t| t.title == title).unwrap().clone();
        let (db, api, ui, docs) = (by("Do db"), by("Do api"), by("Do ui"), by("Do docs"));
        assert!(db.id < api.id && api.id < ui.id, "dependencies first");
        assert_eq!(api.depends_on, vec![db.id]);
        assert_eq!(ui.depends_on, vec![api.id, db.id]);
        assert!(docs.depends_on.is_empty());
        assert_eq!((ui.provider.as_deref(), ui.model.as_deref()), (Some("claude"), Some("opus")));
        assert!(approved.tasks.iter().all(|t| t.project.as_deref() == Some(slug.as_str()) && t.status == TaskStatus::Pending && t.prompt.starts_with("Goal: ")));
        assert_eq!(approved.plan.status, PlanStatus::Approved);
        assert!(approved.plan.decided_at.is_some());
        assert_eq!(approved.plan.tasks.iter().find(|t| t.key == "ui").unwrap().task_id, Some(ui.id));

        assert!(matches!(approve(&hub, plan.id).await, Err(PlanError::Conflict(_))), "approve twice = 409");
        assert_eq!(hub.store.tasks().await.unwrap().len(), 4, "the second approval created nothing");
        assert!(matches!(reject(&hub, plan.id, None).await, Err(PlanError::Conflict(_))));
        assert!(matches!(edit(&hub, plan.id, PlanEdit::default()).await, Err(PlanError::Conflict(_))));
        assert!(matches!(approve(&hub, 999).await, Err(PlanError::NotFound(_))));
        let events = hub.store.events(50, None).await.unwrap();
        assert!(events.iter().any(|e| e.kind == "plan.created") && events.iter().any(|e| e.kind == "plan.approved"));
    }

    #[tokio::test]
    async fn reject_edit_and_supersede() {
        let (hub, _, dir) = hub("").await;
        let slug = project(&hub, &dir.path().join("app"), "App").await;
        let other = project(&hub, &dir.path().join("other"), "Other").await;
        let first = submit(&hub, Some(&slug), submit_body(vec![task("t1", &[]), task("t2", &["t1"])])).await.unwrap();

        // The owner edits a draft: a prompt and a dependency.
        let mut tasks: Vec<PlanTaskInput> = first.tasks.iter().map(|t| PlanTaskInput { key: t.key.clone(), title: t.title.clone(), prompt: t.prompt.clone(), depends_on: Some(t.depends_on.clone()), provider: None, model: None }).collect();
        tasks[0].prompt = "Goal: better.".into();
        tasks[1].depends_on = Some(vec![]);
        let edited = edit(&hub, first.id, PlanEdit { title: Some("Bookings v1".into()), summary: None, tasks: Some(tasks.clone()) }).await.unwrap();
        assert_eq!((edited.title.as_str(), edited.tasks[0].prompt.as_str(), edited.tasks[1].depends_on.len(), edited.summary.as_str()), ("Bookings v1", "Goal: better.", 0, "Restated."));
        tasks[1].depends_on = Some(vec!["t2".into()]);
        assert!(matches!(edit(&hub, first.id, PlanEdit { tasks: Some(tasks), ..Default::default() }).await, Err(PlanError::Invalid(_))));

        // A revision replaces the draft.
        let mut body = submit_body(vec![task("t1", &[])]);
        body.replaces = Some(first.id);
        let second = submit(&hub, Some(&slug), body.clone()).await.unwrap();
        assert_eq!((second.revision, second.replaces), (2, Some(first.id)));
        assert_eq!(hub.store.plan(first.id).await.unwrap().unwrap().status, PlanStatus::Superseded);
        assert!(matches!(approve(&hub, first.id).await, Err(PlanError::Conflict(_))), "a superseded draft cannot be approved");
        // Another project's plan cannot be replaced from here.
        assert!(matches!(submit(&hub, Some(&other), body.clone()).await, Err(PlanError::NotFound(_))));

        let rejected = reject(&hub, second.id, Some(" too big ".into())).await.unwrap();
        assert_eq!((rejected.status, rejected.reason.as_deref()), (PlanStatus::Rejected, Some("too big")));
        // Revising a rejected plan keeps it rejected.
        body.replaces = Some(second.id);
        let third = submit(&hub, Some(&slug), body.clone()).await.unwrap();
        assert_eq!(third.revision, 3);
        assert_eq!(hub.store.plan(second.id).await.unwrap().unwrap().status, PlanStatus::Rejected);
        // An approved plan cannot be replaced.
        approve(&hub, third.id).await.unwrap();
        body.replaces = Some(third.id);
        assert!(matches!(submit(&hub, Some(&slug), body).await, Err(PlanError::Conflict(_))));
        assert!(hub.store.tasks().await.unwrap().iter().all(|t| t.project.as_deref() == Some(slug.as_str())));

        assert_eq!(hub.store.plans(Some(&slug), 10).await.unwrap().iter().map(|p| p.id).collect::<Vec<_>>(), vec![third.id, second.id, first.id]);
        assert!(hub.store.plans(Some(&other), 10).await.unwrap().is_empty());
        assert!(hub.store.plans(Some(""), 10).await.unwrap().is_empty());
        crate::projects::delete(&hub, &slug).await.unwrap();
        assert!(hub.store.plans(Some(&slug), 10).await.unwrap().is_empty(), "deleting a project deletes its plans");
    }

    #[tokio::test]
    async fn default_workspace_plans() {
        let (hub, _, _dir) = hub("").await;
        let plan = submit(&hub, None, submit_body(vec![task("a", &[]), task("b", &["a"])])).await.unwrap();
        assert_eq!(plan.project, None);
        assert_eq!(hub.store.plans(Some(""), 10).await.unwrap().len(), 1);
        assert_eq!(hub.store.plans(None, 10).await.unwrap().len(), 1);
        let approved = approve(&hub, plan.id).await.unwrap();
        assert!(approved.tasks.iter().all(|t| t.project.is_none()));
        assert_eq!(approved.tasks[1].depends_on, vec![approved.tasks[0].id]);
        assert!(matches!(submit(&hub, Some("ghost"), submit_body(vec![task("a", &[])])).await, Err(PlanError::NotFound(_))));
    }

    #[tokio::test]
    async fn planner_session_runs_in_the_project_with_its_instructions_and_no_task() {
        let (hub, term, dir) = hub("").await;
        let folder = dir.path().join("app");
        let slug = project(&hub, &folder, "App").await;
        std::fs::create_dir_all(folder.join("tasks/done")).unwrap();
        std::fs::create_dir_all(folder.join("tasks/todo")).unwrap();
        std::fs::write(folder.join("tasks/README.md"), "# Task cards\n").unwrap();
        std::fs::write(folder.join("tasks/done/007-old.md"), "").unwrap();
        std::fs::write(folder.join("tasks/todo/012b-next.md"), "").unwrap();
        submit(&hub, Some(&slug), submit_body(vec![task("t1", &[])])).await.unwrap();

        let info = info(&hub, Some(&slug)).await.unwrap();
        assert_eq!((info.session_name.as_str(), info.default_provider.as_deref(), info.instructions_source.as_str()), ("map-plan-app", Some("claude"), "built-in"));
        assert!(info.session.is_none());

        let s = start(&hub, Some(&slug), None, None).await.unwrap();
        assert_eq!((s.name.as_str(), s.provider.as_str(), s.model.as_str(), s.role, s.task_id, s.project.as_deref()), ("map-plan-app", "claude", "sonnet", Role::Supervisor, None, Some(slug.as_str())));
        let script = term.sessions.lock().unwrap()["map-plan-app"].script.clone();
        assert!(script.contains(&format!("export MAP_PLANS_URL=http://127.0.0.1:4400/api/projects/{slug}/plans")), "{script}");
        assert!(script.contains(&format!("export MAP_PROJECT_TOKEN={}", hub.project_token(&slug).unwrap())));
        assert!(!script.contains("MAP_RESULT_FILE"), "not a worker: no result file");
        let prompt = std::fs::read_to_string(hub.run_dir("map-plan-app").join("prompt.md")).unwrap();
        assert!(prompt.starts_with("# Planner\n"));
        assert!(prompt.contains("You never write code and never start tasks yourself."));
        assert!(prompt.contains("## Task cards") && prompt.contains("the next card number is 013"), "{prompt}");
        assert!(prompt.contains("- Project: App (`app`).") && prompt.contains("`claude` (opus, sonnet)") && !prompt.contains("`codex`"));
        assert!(prompt.contains("rev 1 draft · Bookings · 1 tasks"));
        assert!(!prompt.contains("<!-- cards -->"));

        // Started again while it runs: the same session, nothing new.
        let again = start(&hub, Some(&slug), Some("codex".into()), None).await.unwrap();
        assert_eq!(again.created_at, s.created_at);
        assert_eq!(term.sessions.lock().unwrap().len(), 1);
        assert_eq!(info_session(&hub, &slug).await, Some("map-plan-app".into()));

        // The orchestrator never adopts it: no task points at it.
        assert!(hub.store.tasks().await.unwrap().is_empty());
        // A project without task cards gets no card section.
        let plain = project(&hub, &dir.path().join("plain"), "Plain").await;
        start(&hub, Some(&plain), None, Some("opus".into())).await.unwrap();
        let prompt = std::fs::read_to_string(hub.run_dir(&format!("map-plan-{plain}")).join("prompt.md")).unwrap();
        assert!(!prompt.contains("## Task cards") && prompt.contains("do not write cards"));
        assert!(start(&hub, Some(&plain), Some("codex".into()), None).await.is_ok(), "running: returned as is");
        assert!(matches!(start(&hub, Some("ghost"), None, None).await, Err(PlanError::NotFound(_))));
    }

    async fn info_session(hub: &Hub, slug: &str) -> Option<String> {
        info(hub, Some(slug)).await.unwrap().session.map(|s| s.name)
    }

    #[tokio::test]
    async fn default_workspace_planner_gets_the_workspace_token() {
        let (hub, term, _dir) = hub("").await;
        assert!(matches!(start(&hub, None, Some("codex".into()), None).await, Err(PlanError::Invalid(_))), "codex is not installed");
        let s = start(&hub, None, None, None).await.unwrap();
        assert_eq!((s.name.as_str(), s.project.as_deref(), s.role), ("map-plan", None, Role::Supervisor));
        let script = term.sessions.lock().unwrap()["map-plan"].script.clone();
        assert!(script.contains("export MAP_PLANS_URL=http://127.0.0.1:4400/api/workspace/plans"));
        assert!(script.contains(&format!("export MAP_PROJECT_TOKEN={}", hub.workspace_token().unwrap())));
        assert!(!script.contains("MAP_DECIDE_URL"));
        let prompt = std::fs::read_to_string(hub.run_dir("map-plan").join("prompt.md")).unwrap();
        assert!(prompt.contains("the default workspace") && prompt.contains("- Plans so far: none."));
    }

    #[tokio::test]
    async fn planner_instructions_can_be_replaced_in_config() {
        let other = tempfile::tempdir().unwrap();
        let path = other.path().join("planner.md");
        std::fs::write(&path, "# My planner\nPlan my way.\n").unwrap();
        let (hub, _, _dir) = hub(&format!("planner_agent = \"{}\"", path.display())).await;
        let i = instructions(&hub).unwrap();
        assert_eq!((i.source, i.content.as_str()), (path.display().to_string(), "# My planner\nPlan my way.\n"));
        start(&hub, None, None, None).await.unwrap();
        assert!(std::fs::read_to_string(hub.run_dir("map-plan").join("prompt.md")).unwrap().starts_with("# My planner\nPlan my way.\n\n## This session"));
        std::fs::remove_file(&path).unwrap();
        assert!(instructions(&hub).is_err(), "a missing file is an error, not a silent fallback");
    }

    #[test]
    fn card_parts_are_kept_or_left_out() {
        assert_eq!(PLANNER_AGENT.lines().filter(|l| l.trim() == CARDS_START).count(), 1);
        assert_eq!(PLANNER_AGENT.lines().filter(|l| l.trim() == CARDS_END).count(), 1);
        assert!(marked_parts(PLANNER_AGENT, true).contains("## Task cards"));
        let without = marked_parts(PLANNER_AGENT, false);
        assert!(!without.contains("## Task cards") && without.contains("## Rules") && without.contains("## Submitting the plan"));
    }
}
