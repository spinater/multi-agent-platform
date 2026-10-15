//! Hooks: shell commands MAP runs around a task (Configure → Hooks). Machine hooks are stored as JSON in the
//! `runtime_settings` table (key [`MACHINE_KEY`]), a project's in its `ProjectSettings::hooks`; `[orchestrator]
//! post_done` in map.toml is shown as read-only machine hooks (`toml:post_done:<n>`) and still run by the
//! orchestrator itself, before these.
//!
//! The orchestrator calls [`run_event`] for `pre_launch` (before `hub.launch`), `post_verify` (after verify, before
//! the audit), `post_done` (after the toml `post_done` commands) and `on_fail`; the watcher for `on_stall`. Every run
//! records `decision.hook.<event>.pass` / `.fail` / `.timeout` through `Hub::event`.

use std::collections::HashSet;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::decisions::Kind;
use crate::hub::Hub;
use crate::model::{ConfigSource, Hook, HookEvent, HookEventInfo, HookItem, HookRun, HooksOverview, Task};

/// The `runtime_settings` key holding the machine hooks (a JSON list of [`Hook`]).
pub const MACHINE_KEY: &str = "hooks";
/// Ids of the read-only hooks made from `[orchestrator] post_done`.
pub const TOML_PREFIX: &str = "toml:";
pub const MAX_TIMEOUT_SECS: u32 = 3600;
/// Lines of output kept in [`HookRun::output_tail`].
const TAIL_LINES: usize = 20;
/// Bytes of output kept while a command runs (the tail is cut from these).
const KEEP_BYTES: usize = 64 * 1024;

pub fn describe(event: HookEvent) -> &'static str {
    match event {
        HookEvent::PreLaunch => "before an agent session starts, in the task's folder; a blocking failure fails the task",
        HookEvent::PostVerify => "after verify passes, before audit; a blocking failure fails the task (no retry)",
        HookEvent::PostDone => "after the task is accepted, after the orchestrator's post_done list; a blocking failure fails the task",
        HookEvent::OnFail => "when a task ends failed and needs a person; never changes the task",
        HookEvent::OnStall => "when the watcher marks a task's session stalled (once per stall); never changes the task",
    }
}

pub fn events() -> Vec<HookEventInfo> {
    HookEvent::ALL.iter().map(|&event| HookEventInfo { event, description: describe(event).into(), can_block: event.can_block() }).collect()
}

// ---- storage and the merged list

/// The machine hooks MAP stores (not the toml ones).
pub async fn machine_hooks(hub: &Hub) -> Result<Vec<Hook>> {
    match hub.store.runtime_setting(MACHINE_KEY).await? {
        Some(text) => serde_json::from_str(&text).context("the stored machine hooks are not valid JSON"),
        None => Ok(vec![]),
    }
}

async fn project_hooks(hub: &Hub, slug: &str) -> Result<Vec<Hook>> {
    Ok(hub.store.project(slug).await?.with_context(|| format!("project `{slug}` does not exist"))?.settings.hooks)
}

/// `[orchestrator] post_done` as read-only machine hooks (run by the orchestrator, blocking, 10 minutes each).
pub fn toml_hooks(hub: &Hub) -> Vec<HookItem> {
    hub.config
        .orchestrator
        .post_done
        .iter()
        .enumerate()
        .map(|(i, command)| HookItem {
            id: format!("{TOML_PREFIX}post_done:{}", i + 1),
            event: HookEvent::PostDone,
            command: command.clone(),
            blocking: true,
            timeout_secs: 600,
            enabled: true,
            source: ConfigSource::Machine,
            read_only: true,
        })
        .collect()
}

/// Machine hooks then the project's: a project hook with a machine hook's id takes its place (`override`).
pub fn merge(machine: &[Hook], project: &[Hook]) -> Vec<(Hook, ConfigSource)> {
    let mut out: Vec<(Hook, ConfigSource)> = machine
        .iter()
        .map(|m| match project.iter().find(|p| p.id == m.id) {
            Some(p) => (p.clone(), ConfigSource::Override),
            None => (m.clone(), ConfigSource::Machine),
        })
        .collect();
    out.extend(project.iter().filter(|p| !machine.iter().any(|m| m.id == p.id)).map(|p| (p.clone(), ConfigSource::Project)));
    out
}

fn item(h: Hook, source: ConfigSource) -> HookItem {
    HookItem { id: h.id, event: h.event, command: h.command, blocking: h.blocking, timeout_secs: h.timeout_secs, enabled: h.enabled, source, read_only: false }
}

/// `GET /api/hooks`: the toml hooks, the machine's, and with `project` the project's merged in.
pub async fn overview(hub: &Hub, project: Option<&str>) -> Result<HooksOverview> {
    let machine = machine_hooks(hub).await?;
    let own = match project {
        Some(slug) => project_hooks(hub, slug).await?,
        None => vec![],
    };
    let mut hooks = toml_hooks(hub);
    hooks.extend(merge(&machine, &own).into_iter().map(|(h, s)| item(h, s)));
    Ok(HooksOverview { project: project.map(String::from), hooks, events: events() })
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn new_id(event: HookEvent, taken: &HashSet<String>) -> String {
    loop {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or_default();
        let n = nanos.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let id = format!("{}-{:08x}", event.as_str(), n as u32);
        if !taken.contains(&id) {
            return id;
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || "-_.:".contains(c))
}

/// Checks a scope's list: a command, a timeout of 1 to 3600 seconds, unique ids of `[A-Za-z0-9-_.:]`, none `toml:`.
pub fn validate(hooks: &[Hook]) -> Result<()> {
    let mut seen = HashSet::new();
    for h in hooks {
        if h.command.trim().is_empty() {
            bail!("a {} hook needs a command", h.event.as_str());
        }
        if h.command.len() > 4000 {
            bail!("hook `{}`: the command is longer than 4000 characters", h.id);
        }
        if !(1..=MAX_TIMEOUT_SECS).contains(&h.timeout_secs) {
            bail!("hook `{}`: timeout must be 1 to {MAX_TIMEOUT_SECS} seconds, not {}", h.command.trim(), h.timeout_secs);
        }
        if !valid_id(&h.id) {
            bail!("hook id `{}` must be 1 to 64 letters, digits, `-`, `_`, `.` or `:`", h.id);
        }
        if h.id.starts_with(TOML_PREFIX) {
            bail!("hook id `{}`: `{TOML_PREFIX}` ids are the read-only map.toml hooks", h.id);
        }
        if !seen.insert(h.id.as_str()) {
            bail!("hook id `{}` is used twice", h.id);
        }
    }
    Ok(())
}

/// A list from `PUT /api/hooks`, ready to save: `toml:` hooks dropped (they are read-only and come back from
/// map.toml), commands trimmed, ids made for new hooks, then [`validate`]d.
pub fn normalize(hooks: Vec<Hook>) -> Result<Vec<Hook>> {
    let mut hooks: Vec<Hook> = hooks.into_iter().filter(|h| !h.id.starts_with(TOML_PREFIX)).collect();
    let mut taken: HashSet<String> = hooks.iter().map(|h| h.id.trim().to_string()).filter(|i| !i.is_empty()).collect();
    for h in &mut hooks {
        h.command = h.command.trim().to_string();
        h.id = h.id.trim().to_string();
        if h.id.is_empty() {
            h.id = new_id(h.event, &taken);
            taken.insert(h.id.clone());
        }
    }
    validate(&hooks)?;
    Ok(hooks)
}

/// `PUT /api/hooks`: replaces the machine's list, or the project's.
pub async fn save(hub: &Hub, project: Option<&str>, hooks: Vec<Hook>) -> Result<HooksOverview> {
    let hooks = normalize(hooks)?;
    let count = hooks.len();
    match project {
        None => hub.store.set_runtime_setting(MACHINE_KEY, Some(&serde_json::to_string(&hooks)?)).await?,
        Some(slug) => {
            let mut p = hub.store.project(slug).await?.with_context(|| format!("project `{slug}` does not exist"))?;
            p.settings.hooks = hooks;
            hub.put_project(&p).await?;
        }
    }
    let place = project.map_or("the machine".to_string(), |s| format!("project {s}"));
    hub.event(crate::model::Level::Info, "config.hooks", None, None, format!("{count} hook(s) saved for {place}")).await;
    overview(hub, project).await
}

// ---- running

/// Runs `sh -c command` in `dir` with `env`, killing it (and everything it started) after `timeout`. Fails only when
/// the shell cannot be started.
pub async fn run_command(dir: &Path, command: &str, timeout: Duration, env: &[(String, String)]) -> Result<HookRun> {
    let mut cmd = Command::new("sh");
    // One pipe for both streams, in the order they were written.
    cmd.arg("-c").arg(format!("exec 2>&1\n{command}")).current_dir(dir).envs(env.iter().cloned());
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true).process_group(0);
    let started = Instant::now();
    let mut child = cmd.spawn().with_context(|| format!("starting `sh` in {}", dir.display()))?;
    let pid = child.id();
    let mut stdout = child.stdout.take().context("no stdout")?;
    let kept: Arc<std::sync::Mutex<Vec<u8>>> = Default::default();
    let sink = Arc::clone(&kept);
    let reader = tokio::spawn(async move {
        let mut buf = [0u8; 8192];
        while let Ok(n) = stdout.read(&mut buf).await {
            if n == 0 {
                break;
            }
            let mut kept = sink.lock().unwrap();
            kept.extend_from_slice(&buf[..n]);
            if kept.len() > 2 * KEEP_BYTES {
                let cut = kept.len() - KEEP_BYTES;
                kept.drain(..cut);
            }
        }
    });
    let (exit_code, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(status) => (status?.code(), false),
        Err(_) => {
            if let Some(pid) = pid {
                // The whole process group: `sh` and whatever it started.
                // SAFETY: kill(2) with a negative pid signals that process group; no memory is touched.
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            let _ = child.kill().await;
            (None, true)
        }
    };
    let duration_ms = started.elapsed().as_millis() as u64;
    // A grandchild that left the group could hold the pipe open: do not wait for it forever.
    let _ = tokio::time::timeout(Duration::from_secs(2), reader).await;
    let bytes = std::mem::take(&mut *kept.lock().unwrap());
    let mut output_tail = tail_lines(&String::from_utf8_lossy(&bytes), TAIL_LINES);
    if timed_out {
        output_tail.push_str(&format!("{}timed out after {}s", if output_tail.is_empty() { "" } else { "\n" }, timeout.as_secs()));
    }
    Ok(HookRun { exit_code, duration_ms, output_tail, timed_out })
}

/// The last `n` non-empty lines, each with secrets masked (`trace::mask`).
fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|l| crate::trace::mask(l)).collect::<Vec<_>>().join("\n")
}

/// The variables every hook gets: `[workspace.env]` and MAP's gitconfig, then `MAP_EVENT`, `MAP_TASK_ID`,
/// `MAP_TASK_TITLE`, and `MAP_PROJECT`, `MAP_SESSION`, `MAP_BRANCH` when known.
pub fn hook_env(hub: &Hub, event: &str, task: Option<&Task>, project: Option<&str>, session: Option<&str>) -> Vec<(String, String)> {
    let mut env = hub.command_env();
    env.push(("MAP_EVENT".into(), event.into()));
    if let Some(t) = task {
        env.push(("MAP_TASK_ID".into(), t.id.to_string()));
        env.push(("MAP_TASK_TITLE".into(), t.title.clone()));
        if let Some(git) = &t.git {
            env.push(("MAP_BRANCH".into(), git.branch.clone()));
        }
    }
    if let Some(p) = project.or(task.and_then(|t| t.project.as_deref())) {
        env.push(("MAP_PROJECT".into(), p.into()));
    }
    if let Some(s) = session.or(task.and_then(|t| t.session.as_deref())) {
        env.push(("MAP_SESSION".into(), s.into()));
    }
    env
}

/// `POST /api/hooks/test`: the command now, in the project's folder (or `workspace.dir`), with `MAP_EVENT=test`.
pub async fn test(hub: &Hub, project: Option<&str>, command: &str, timeout_secs: u32) -> Result<HookRun> {
    if command.trim().is_empty() {
        bail!("give a command to test");
    }
    if !(1..=MAX_TIMEOUT_SECS).contains(&timeout_secs) {
        bail!("timeout must be 1 to {MAX_TIMEOUT_SECS} seconds");
    }
    let dir = hub.work_dir(project).await;
    if !dir.is_dir() {
        bail!("the folder {} does not exist", dir.display());
    }
    let env = hook_env(hub, "test", None, project, None);
    run_command(&dir, command, Duration::from_secs(timeout_secs as u64), &env).await
}

/// What the hooks of one event did.
#[derive(Debug, Default, Clone)]
pub struct HookOutcome {
    pub ran: usize,
    pub passed: usize,
    /// One line per failed hook: `<event> hook failed: <cmd>: exit N`.
    pub failures: Vec<String>,
    /// A blocking hook failed (the rest of the event's hooks did not run): the task's failure reason, then the
    /// hook's output tail.
    pub blocked: Option<String>,
}

/// The decision kind of one hook run.
fn kind(event: HookEvent, run: &HookRun) -> Kind {
    let outcome = if run.timed_out {
        2
    } else if run.ok() {
        0
    } else {
        1
    };
    Kind::hook(event, outcome)
}

/// Runs the enabled hooks of `event` for `task` (machine then project, in order) in the task's folder. A blocking
/// failure of `pre_launch`, `post_verify` or `post_done` stops there and is returned in [`HookOutcome::blocked`];
/// the caller fails the task. Never changes the task itself.
pub async fn run_event(hub: &Hub, event: HookEvent, task: &Task, session: Option<&str>) -> Result<HookOutcome> {
    let machine = machine_hooks(hub).await?;
    let own = match task.project.as_deref() {
        Some(slug) => hub.store.project(slug).await?.map(|p| p.settings.hooks).unwrap_or_default(),
        None => vec![],
    };
    let hooks: Vec<Hook> = merge(&machine, &own).into_iter().map(|(h, _)| h).filter(|h| h.enabled && h.event == event).collect();
    let mut out = HookOutcome::default();
    if hooks.is_empty() {
        return Ok(out);
    }
    let dir = hub.task_dir(task).await;
    let env = hook_env(hub, event.as_str(), Some(task), None, session);
    let session = session.or(task.session.as_deref());
    for h in hooks {
        let timeout = Duration::from_secs(h.timeout_secs.clamp(1, MAX_TIMEOUT_SECS) as u64);
        let run = match run_command(&dir, &h.command, timeout, &env).await {
            Ok(run) => run,
            Err(err) => HookRun { exit_code: None, duration_ms: 0, output_tail: format!("{err:#}"), timed_out: false },
        };
        out.ran += 1;
        let k = kind(event, &run);
        let cmd = crate::trace::mask(&h.command);
        let secs = format!("{:.1}s", run.duration_ms as f64 / 1000.0);
        let how = match (run.timed_out, run.exit_code) {
            (true, _) => format!("timed out after {}s", timeout.as_secs()),
            (false, Some(c)) => format!("exit {c}"),
            (false, None) if run.duration_ms == 0 => "could not start".to_string(),
            (false, None) => "killed by a signal".to_string(),
        };
        let mut message = format!("`{cmd}` {how} · {secs}");
        if !run.ok() {
            message.push_str(&format!(" · {}", crate::decisions::last_line(&run.output_tail)));
        }
        hub.event(k.level, k.kind, session, Some(task.id), &message).await;
        if run.ok() {
            out.passed += 1;
            continue;
        }
        let reason = format!("{} hook failed: {cmd}: {how}", event.as_str());
        out.failures.push(reason.clone());
        if h.blocking && event.can_block() {
            out.blocked = Some(if run.output_tail.is_empty() { reason } else { format!("{reason}\n{}", run.output_tail) });
            break;
        }
    }
    Ok(out)
}

/// Runs `event`'s hooks in the background (for `on_fail` and `on_stall`, which never wait on the task).
pub fn spawn_event(hub: Arc<Hub>, event: HookEvent, task: Task, session: Option<String>) {
    tokio::spawn(async move {
        if let Err(err) = run_event(&hub, event, &task, session.as_deref()).await {
            tracing::warn!(task = task.id, "{} hooks: {err:#}", event.as_str());
        }
    });
}

#[cfg(test)]
pub(crate) mod tests;
