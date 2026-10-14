//! Parallel tasks per project. A project caps its own active tasks (`parallel.max`, applied at the next tick). In a
//! worktree project every task has its own branch, so the cap is all there is. In the shared folder (one folder, one
//! branch) a second task starts beside a running one only when the parallel analyst, an agent run headless, has
//! cleared both: it read the tasks and the code and declared the paths each will touch, and those are disjoint.
//! Tasks the analyst has not seen wait for it; with the analyst off, a shared project runs one task at a time.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;

use crate::hub::Hub;
use crate::model::{BranchMode, Level, ProjectParallel, Task, TaskScope, TaskStatus};

/// A failed or empty analysis of the same tasks is not repeated sooner than this.
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

/// Tasks of the same project that hold its folder (or a worktree) right now.
pub fn is_active(t: &Task) -> bool {
    matches!(t.status, TaskStatus::Running | TaskStatus::Verifying | TaskStatus::Auditing | TaskStatus::Landing)
}

/// `src/billing/**` → `src/billing`; `*`, `.`, `**` → `` (the whole repo). A glob in the middle keeps the folder
/// before it, which is the safe (wider) reading.
fn prefix(path: &str) -> String {
    let p = path.trim().trim_start_matches("./").trim_start_matches('/');
    let cut = p.find(['*', '?', '[', '{']).unwrap_or(p.len());
    p[..cut].trim_end_matches('/').to_string()
}

/// Do two path lists touch the same files? Folders cover what is under them; an empty list counts as the whole repo.
pub fn overlap(a: &[String], b: &[String]) -> bool {
    if a.is_empty() || b.is_empty() {
        return true;
    }
    a.iter().map(|x| prefix(x)).any(|x| {
        b.iter().map(|y| prefix(y)).any(|y| x.is_empty() || y.is_empty() || x == y || x.starts_with(&format!("{y}/")) || y.starts_with(&format!("{x}/")))
    })
}

/// What the tick may do with a ready task, given the project's active tasks.
#[derive(Debug, PartialEq)]
pub enum Gate {
    Go,
    /// Cleared to share the folder with these tasks.
    Cleared(String),
    /// At the project's own limit.
    Full(String),
    /// The shared folder is busy and the task is not cleared to run beside the active ones.
    Busy(String),
    /// The analyst has not looked at it (or a busy one) yet.
    AwaitAnalyst(String),
}

pub fn gate(task: &Task, mode: BranchMode, p: &ProjectParallel, active: &[&Task]) -> Gate {
    if let Some(max) = p.max
        && active.len() as u32 >= max
    {
        return Gate::Full(format!("{} of {max} of the project's tasks running", active.len()));
    }
    if active.is_empty() || mode == BranchMode::Worktree {
        return Gate::Go;
    }
    let ids = active.iter().map(|t| format!("#{}", t.id)).collect::<Vec<_>>().join(", ");
    if !p.analyst {
        return Gate::Busy(format!("shared folder busy with {ids} · the parallel analyst is off, so one task at a time"));
    }
    let Some(mine) = task.scope.as_ref() else { return Gate::AwaitAnalyst(format!("shared folder busy with {ids} · waiting for the parallel analyst")) };
    if !mine.independent {
        return Gate::Busy(format!("shared folder busy with {ids} · the analyst found this task not independent: {}", mine.reason));
    }
    for other in active {
        match other.scope.as_ref() {
            None => return Gate::AwaitAnalyst(format!("shared folder busy with #{} · waiting for the parallel analyst to scope it", other.id)),
            Some(s) if !s.independent => return Gate::Busy(format!("shared folder busy with #{}, which the analyst found not independent", other.id)),
            Some(s) if overlap(&mine.paths, &s.paths) => return Gate::Busy(format!("shared folder busy with #{} · their paths overlap", other.id)),
            Some(_) => {}
        }
    }
    Gate::Cleared(format!("cleared to run beside {ids} · paths disjoint ({})", mine.paths.join(", ")))
}

/// One project's analyst: running now, and the tasks the last run covered and when it started.
type Run = (bool, Vec<i64>, Option<Instant>);

/// The analyst's runs per project.
#[derive(Default)]
pub struct Analyses {
    state: std::sync::Mutex<HashMap<String, Run>>,
}

impl Analyses {
    /// Claims a run for `key` over `ids`; `false` while one runs or when the same tasks were just analysed.
    fn claim(&self, key: &str, ids: &[i64], force: bool) -> bool {
        let mut map = self.state.lock().unwrap();
        let entry = map.entry(key.to_string()).or_default();
        if entry.0 || (!force && entry.1 == ids && entry.2.is_some_and(|at| at.elapsed() < RETRY_AFTER)) {
            return false;
        }
        *entry = (true, ids.to_vec(), Some(Instant::now()));
        true
    }

    fn release(&self, key: &str) {
        if let Some(e) = self.state.lock().unwrap().get_mut(key) {
            e.0 = false;
        }
    }
}

#[derive(Deserialize)]
pub struct SubmittedScope {
    pub id: i64,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub independent: bool,
    #[serde(default)]
    pub reason: String,
}

#[derive(Deserialize)]
struct Answer {
    tasks: Vec<SubmittedScope>,
}

/// Saves scopes for tasks of `project` (others are ignored); how many were saved.
pub async fn save_scopes(hub: &Hub, project: Option<&str>, scopes: Vec<SubmittedScope>, by: &str) -> Result<usize> {
    let mut saved = 0;
    for s in scopes {
        let Some(mut task) = hub.store.task(s.id).await? else { continue };
        if task.project.as_deref() != project || task.status.is_terminal() {
            continue;
        }
        let paths: Vec<String> = s.paths.into_iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).take(50).collect();
        // No paths means "anywhere": never independent.
        let independent = s.independent && !paths.is_empty();
        task.scope = Some(TaskScope { paths, independent, reason: s.reason.chars().take(500).collect(), by: by.to_string(), at: Utc::now() });
        hub.put_task(&mut task).await?;
        saved += 1;
    }
    Ok(saved)
}

/// The JSON object with a `tasks` list in the analyst's reply: the last one that parses.
fn parse_answer(output: &str) -> Option<Answer> {
    output.match_indices('{').rev().find_map(|(i, _)| {
        let v: Value = serde_json::Deserializer::from_str(&output[i..]).into_iter::<Value>().next()?.ok()?;
        v.get("tasks")?.as_array()?;
        serde_json::from_value(v).ok()
    })
}

fn prompt(slug: Option<&str>, tasks: &[Task]) -> String {
    let mut p = format!(
        "You are the parallel analyst for {}. Several tasks may run at the same time in ONE shared folder on ONE git branch, so each agent sees \
         the others' half-done edits. Decide which tasks can safely run side by side.\n\
         Two tasks are independent only if neither changes files the other changes, they do not both touch shared files (lockfiles, generated \
         code, migrations and their numbering, shared config, schema, routes registries, CLAUDE.md, package manifests), and neither one's build or \
         tests would break on the other's unfinished work. When unsure, say not independent.\n\
         Inspect the repository READ-ONLY (list, read, grep; never edit, format, install, commit or switch branches) to find what each task will \
         change.\n\nTasks:\n",
        slug.map(|s| format!("project `{s}`")).unwrap_or_else(|| "the default workspace".into())
    );
    for t in tasks {
        let state = if is_active(t) { "running now" } else { "waiting to start" };
        let body: String = t.prompt.chars().take(1500).collect();
        p.push_str(&format!("\n### #{} ({state}): {}\n{}\n", t.id, t.title, body.trim()));
    }
    p.push_str(
        "\nEnd your reply with exactly one JSON object (nothing after it):\n\
         {\"tasks\": [{\"id\": <task id>, \"paths\": [\"<file or folder it will change>\", …], \"independent\": true|false, \"reason\": \"<one line>\"}]}\n\
         One entry per task above. `paths` are repository-relative; be generous (tests, fixtures, generated files and lockfiles it will touch); a \
         folder ends with `/`. A task that touches broad or shared areas, or whose scope you cannot pin down, is `\"independent\": false`.\n",
    );
    p
}

/// Starts the analyst for the project's tasks without a scope (active ones and the ready pending ones) in the
/// background; why not, when it does not.
pub async fn analyze(hub: &std::sync::Arc<Hub>, project: Option<String>, force: bool) -> Result<Option<String>> {
    let key = project.clone().unwrap_or_default();
    let settings = match &project {
        Some(slug) => hub.store.project(slug).await?.map(|p| p.settings).unwrap_or_default(),
        None => Default::default(),
    };
    let all = hub.store.tasks().await?;
    let mut tasks: Vec<Task> = all
        .iter()
        .filter(|t| t.project == project && t.scope.is_none() && (is_active(t) || t.status == TaskStatus::Pending))
        .filter(|t| t.depends_on.iter().all(|d| all.iter().any(|x| x.id == *d && x.status == TaskStatus::Done)))
        .cloned()
        .collect();
    tasks.truncate(12);
    // The scoped active tasks are context too: the new ones must not overlap them.
    let ids: Vec<i64> = tasks.iter().map(|t| t.id).collect();
    if tasks.is_empty() {
        return Ok(Some("every open task already has a scope".into()));
    }
    let Some(provider) = analyst_provider(hub, &settings.parallel, project.as_deref()).await else {
        return Ok(Some("no provider with a headless command is available for the analyst".into()));
    };
    if !hub.analyses.claim(&key, &ids, force) {
        return Ok(Some("the analyst is already running for these tasks, or just did".into()));
    }
    let hub = std::sync::Arc::clone(hub);
    tokio::spawn(async move {
        let label = project.as_deref().unwrap_or("the default workspace").to_string();
        hub.event(Level::Info, "parallel.analysis", None, None, format!("Parallel analyst ({provider}) is scoping {} task(s) of {label}", ids.len())).await;
        let (level, text) = match run(&hub, &provider, project.as_deref(), &tasks).await {
            Ok(n) => (Level::Success, format!("Parallel analyst scoped {n} task(s) of {label}")),
            Err(err) => (Level::Warning, format!("Parallel analyst for {label} gave no answer: {err:#}")),
        };
        hub.event(level, "parallel.analysis", None, None, text).await;
        hub.analyses.release(project.as_deref().unwrap_or_default());
    });
    Ok(None)
}

async fn run(hub: &Hub, provider: &str, project: Option<&str>, tasks: &[Task]) -> Result<usize> {
    let spec = hub.spec(provider)?;
    let text = prompt(project, tasks);
    let argv: Vec<String> =
        spec.headless.iter().map(|a| a.replace("{binary}", &spec.binary).replace("{model}", &spec.default_model).replace("{session}", "parallel-analyst").replace("{prompt}", &text)).collect();
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(hub.work_dir(project).await).envs(hub.provider_env(provider)).envs(hub.command_env());
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(hub.config.orchestrator.audit.timeout_secs.max(120)), cmd.output()).await??;
    let output = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let answer = parse_answer(&output).ok_or_else(|| anyhow::anyhow!("no JSON with a `tasks` list in the reply ({})", crate::decisions::last_line(&output)))?;
    save_scopes(hub, project, answer.tasks, &format!("analyst:{provider}")).await
}

/// The analyst's provider: the project's choice, else its audit provider, else the first of its providers (or the
/// audit chain) that is installed, not cooling down and has a headless command.
async fn analyst_provider(hub: &Hub, p: &ProjectParallel, project: Option<&str>) -> Option<String> {
    let mut candidates: Vec<String> = p.analyst_provider.iter().cloned().collect();
    if let Some(slug) = project
        && let Ok(Some(proj)) = hub.store.project(slug).await
    {
        candidates.extend(proj.settings.audit.and_then(|a| a.provider));
        candidates.extend(proj.settings.providers.into_iter().map(|p| p.name));
    }
    candidates.extend(hub.config.orchestrator.audit.chain.iter().cloned());
    candidates.extend(hub.config.orchestrator.provider_order.iter().cloned());
    candidates.into_iter().find(|c| hub.spec(c).is_ok_and(|s| !s.headless.is_empty()) && hub.provider_available(c) && hub.cooldown_until(c).is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: i64, scope: Option<(&[&str], bool)>) -> Task {
        Task {
            id,
            title: format!("t{id}"),
            prompt: String::new(),
            depends_on: vec![],
            provider: None,
            model: None,
            project: Some("p".into()),
            status: TaskStatus::Pending,
            session: None,
            attempts: 0,
            result: None,
            git: None,
            scope: scope.map(|(paths, independent)| TaskScope { paths: paths.iter().map(|p| p.to_string()).collect(), independent, reason: "r".into(), by: "analyst:claude".into(), at: Utc::now() }),
            intake: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn paths_overlap_by_folder() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(!overlap(&v(&["src/billing/"]), &v(&["src/booking/**", "web/app/x.tsx"])));
        assert!(overlap(&v(&["src/billing/"]), &v(&["src/billing/invoice.rs"])));
        assert!(overlap(&v(&["./src/*/mod.rs"]), &v(&["src/billing/x.rs"])), "a glob reads as its folder");
        assert!(overlap(&v(&["*"]), &v(&["docs/a.md"])));
        assert!(overlap(&v(&[]), &v(&["docs/a.md"])));
        assert!(!overlap(&v(&["src/bill"]), &v(&["src/billing/x.rs"])), "a name prefix is not a folder");
    }

    #[test]
    fn the_gate_needs_both_tasks_cleared_and_disjoint() {
        let on = ProjectParallel::default();
        let running_a = task(1, Some((&["src/a/"], true)));
        let b = task(2, Some((&["src/b/"], true)));
        assert_eq!(gate(&b, BranchMode::Shared, &on, &[]), Gate::Go);
        assert!(matches!(gate(&b, BranchMode::Shared, &on, &[&running_a]), Gate::Cleared(_)));
        assert!(matches!(gate(&task(3, Some((&["src/a/x.rs"], true))), BranchMode::Shared, &on, &[&running_a]), Gate::Busy(_)));
        assert!(matches!(gate(&task(4, None), BranchMode::Shared, &on, &[&running_a]), Gate::AwaitAnalyst(_)));
        assert!(matches!(gate(&b, BranchMode::Shared, &on, &[&task(5, None)]), Gate::AwaitAnalyst(_)));
        assert!(matches!(gate(&task(6, Some((&["src/c/"], false))), BranchMode::Shared, &on, &[&running_a]), Gate::Busy(_)));
        let off = ProjectParallel { analyst: false, ..Default::default() };
        assert!(matches!(gate(&b, BranchMode::Shared, &off, &[&running_a]), Gate::Busy(_)));
        // Worktrees need no analyst; the project's own cap applies to both modes.
        assert_eq!(gate(&task(7, None), BranchMode::Worktree, &on, &[&running_a]), Gate::Go);
        let capped = ProjectParallel { max: Some(1), ..Default::default() };
        assert!(matches!(gate(&b, BranchMode::Worktree, &capped, &[&running_a]), Gate::Full(_)));
    }

    #[test]
    fn the_answer_is_the_last_json_with_tasks() {
        let out = "I looked around. {\"note\": 1}\n```json\n{\"tasks\": [{\"id\": 3, \"paths\": [\"src/a/\"], \"independent\": true, \"reason\": \"own module\"}]}\n```\n";
        let a = parse_answer(out).unwrap();
        assert_eq!((a.tasks.len(), a.tasks[0].id, a.tasks[0].independent), (1, 3, true));
        assert!(parse_answer("no json here").is_none());
    }
}
