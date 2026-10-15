//! The orchestrator: start ready tasks on worker sessions, watch for their result files, then verify, audit and
//! finish them. Generalised from the Nutto `orchestrator.ts`: tasks live in the database instead of
//! `tasks/todo/*.md`, and the agent reports through `$MAP_RESULT_FILE` instead of moving its task sheet.

use std::collections::HashSet;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Deserialize;
use tokio::process::Command;

use crate::decisions::{self as dec, Kind, record};
use crate::detect::EndDecision;
use crate::hub::{Hub, LaunchRequest, quote};
use crate::model::{HookEvent, LandMode, Level, Role, SessionState, Task, TaskStatus};

pub(crate) const TICK: Duration = Duration::from_secs(5);
const OUTPUT_TAIL: usize = 4000;

#[derive(Debug, Deserialize)]
struct ResultFile {
    status: String,
    #[serde(default)]
    summary: String,
}

pub struct Orchestrator {
    hub: Arc<Hub>,
    /// Tasks whose verify/audit step is running in the background.
    finishing: Arc<Mutex<HashSet<i64>>>,
}

pub async fn run(hub: Arc<Hub>) {
    let mut orch = Orchestrator::new(hub);
    let mut step = false;
    loop {
        if (step || orch.hub.orchestrator_running())
            && let Err(err) = orch.tick().await
        {
            tracing::error!("orchestrator tick failed: {err:#}");
        }
        if step {
            orch.hub.step.finished();
        }
        // A step request (`POST /api/orchestrator/step`) ends the sleep and runs one tick, paused or not.
        step = orch.hub.step.sleep(TICK).await;
    }
}

fn tail(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(max)..].iter().collect()
}

/// Runs `sh -c command` in `dir` with the workspace env and MAP's gitconfig; returns (success, combined output tail).
pub async fn shell(hub: &Hub, dir: &std::path::Path, command: &str, timeout: Duration) -> Result<(bool, String)> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(dir).envs(hub.command_env());
    run_captured(cmd, timeout).await
}

/// [`shell`] with the project's Env & secrets (`env_vars.rs`) after the workspace env, for a project's hooks and
/// verify commands; any secret value in the output is replaced by `***` before it reaches events or a task.
pub async fn shell_for(hub: &Hub, project: Option<&str>, dir: &std::path::Path, command: &str, timeout: Duration) -> Result<(bool, String)> {
    let vars = crate::env_vars::run_vars(hub, project).await;
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(dir).envs(hub.command_env()).envs(vars.iter().map(|v| (v.key.as_str(), v.value.as_str())));
    let (ok, output) = run_captured(cmd, timeout).await?;
    Ok((ok, crate::env_vars::redact(&output, &crate::env_vars::secret_values(&vars))))
}

async fn run_captured(mut cmd: Command, timeout: Duration) -> Result<(bool, String)> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    let out = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(out) => out?,
        Err(_) => return Ok((false, format!("timed out after {}s", timeout.as_secs()))),
    };
    let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    Ok((out.status.success(), tail(&text, OUTPUT_TAIL)))
}

/// A commit's first 10 characters.
fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

fn looks_like_quota(output: &str) -> bool {
    let lower = output.to_lowercase();
    crate::detect::rate_limit_line(output).is_some() || lower.contains("credit balance is too low") || lower.contains("quota")
}

impl Orchestrator {
    pub fn new(hub: Arc<Hub>) -> Self {
        let finishing = Arc::clone(&hub.finishing);
        Self { hub, finishing }
    }

    pub async fn tick(&mut self) -> Result<()> {
        let tasks = self.hub.store.tasks().await?;
        for task in tasks.iter().filter(|t| t.status == TaskStatus::Running) {
            if let Err(err) = self.check_running(task.clone()).await {
                tracing::warn!(task = task.id, "check failed: {err:#}");
            }
        }
        // Verify/audit steps lost to a restart are picked up again.
        for task in tasks.iter().filter(|t| matches!(t.status, TaskStatus::Verifying | TaskStatus::Auditing | TaskStatus::Landing)) {
            self.spawn_finish(task.clone());
        }

        let mut tasks = self.hub.store.tasks().await?;
        let paused = crate::project_settings::paused(&self.hub).await;
        let projects: std::collections::HashMap<String, crate::model::ProjectSettings> =
            self.hub.store.projects().await.unwrap_or_default().into_iter().map(|p| (p.slug, p.settings)).collect();
        // The intake gate, before any LLM sees a task: once per task, whatever the free slots.
        for i in 0..tasks.len() {
            let t = &tasks[i];
            let on = t.project.as_ref().and_then(|p| projects.get(p)).filter(|s| s.reflex_gates.intake);
            if t.status != TaskStatus::Pending || t.intake.is_some() || crate::project_settings::is_paused(&paused, t) {
                continue;
            }
            let Some(settings) = on.cloned() else { continue };
            match crate::intake::run(&self.hub, t.clone(), &settings, &tasks).await {
                Ok(saved) => tasks[i] = saved,
                Err(err) => tracing::warn!(task = t.id, "intake: {err:#}"),
            }
        }
        let mut active = tasks.iter().filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Verifying | TaskStatus::Auditing | TaskStatus::Landing)).count();
        // Tasks that hold a folder: the active ones, plus those this tick starts.
        let mut holding: Vec<Task> = tasks.iter().filter(|t| crate::parallel::is_active(t)).cloned().collect();
        let mut analyse: std::collections::BTreeSet<Option<String>> = Default::default();
        for task in tasks.iter().filter(|t| t.status == TaskStatus::Pending && !crate::project_settings::is_paused(&paused, t)) {
            if active >= self.hub.max_parallel() {
                break;
            }
            // Held by the intake gate until the owner starts it anyway, edits it or cancels it.
            if task.intake.as_ref().is_some_and(|i| i.holds()) {
                continue;
            }
            let deps: Vec<&Task> = task.depends_on.iter().filter_map(|id| tasks.iter().find(|t| t.id == *id)).collect();
            if let Some(dead) = deps.iter().find(|d| matches!(d.status, TaskStatus::Failed | TaskStatus::Cancelled)) {
                let note = format!("Blocked: dependency #{} {}", dead.id, dead.status.as_str());
                if task.result.as_deref() != Some(note.as_str()) {
                    record(&self.hub, Kind::READY_BLOCKED, task, format!("dependency #{} {}", dead.id, dead.status.as_str())).await;
                    let mut t = task.clone();
                    t.result = Some(note);
                    self.hub.put_task(&mut t).await?;
                }
                continue;
            }
            let missing = task.depends_on.len() != deps.len();
            if missing || deps.iter().any(|d| d.status != TaskStatus::Done) {
                continue;
            }
            let settings = task.project.as_ref().and_then(|p| projects.get(p)).cloned().unwrap_or_default();
            let mine: Vec<&Task> = holding.iter().filter(|t| t.project == task.project).collect();
            match crate::parallel::gate(task, settings.branches.mode, &settings.parallel, &mine) {
                crate::parallel::Gate::Go => {}
                crate::parallel::Gate::Cleared(why) => record(&self.hub, Kind::READY_CLEARED, task, why).await,
                crate::parallel::Gate::Full(why) => {
                    record(&self.hub, Kind::READY_FULL, task, why).await;
                    continue;
                }
                crate::parallel::Gate::Busy(why) => {
                    record(&self.hub, Kind::READY_SHARED_BUSY, task, why).await;
                    continue;
                }
                crate::parallel::Gate::AwaitAnalyst(why) => {
                    record(&self.hub, Kind::READY_AWAIT_ANALYST, task, why).await;
                    analyse.insert(task.project.clone());
                    continue;
                }
            }
            record(&self.hub, Kind::READY_GO, task, dec::ready_detail(task, active, self.hub.max_parallel())).await;
            if self.start(task.clone()).await? {
                active += 1;
                if let Some(started) = self.hub.store.task(task.id).await? {
                    holding.push(started);
                }
            }
        }
        for project in analyse {
            if let Err(err) = crate::parallel::analyze(&self.hub, project, false).await {
                tracing::warn!("parallel analyst: {err:#}");
            }
        }
        Ok(())
    }

    async fn pick_provider(&self, task: &Task) -> Option<String> {
        let candidates = crate::project_settings::candidates(&self.hub, task).await;
        candidates.into_iter().find(|p| self.hub.provider_available(p) && self.hub.cooldown_until(p).is_none())
    }

    fn build_prompt(&self, task: &Task, session: &str, previous_failure: Option<&str>, verify: &[String]) -> String {
        let result_file = self.hub.result_file(session);
        let mut p = format!("{}\n\n---\nPlatform protocol (multi-agent-platform)\n", task.prompt.trim());
        p.push_str(&format!("- You are worker session `{session}` on task #{}: {}.\n", task.id, task.title));
        p.push_str("- Work autonomously in this repository; only stop to ask when you are truly blocked.\n");
        p.push_str(&format!(
            "- When you finish, write JSON to {} (also in $MAP_RESULT_FILE): {{\"status\": \"done\", \"summary\": \"<what changed and how you checked it>\"}}.\n",
            result_file.display()
        ));
        p.push_str("  If you cannot finish, write {\"status\": \"failed\", \"summary\": \"<why>\"} instead.\n");
        if !verify.is_empty() {
            p.push_str(&format!("- After you report done, the orchestrator runs: {}. Run them yourself first.\n", verify.join(" && ")));
        }
        if task.git.is_none()
            && let Some(scope) = task.scope.as_ref().filter(|s| s.independent)
        {
            p.push_str(&format!(
                "- Other agents may be working in this same folder and branch right now, cleared to because your task stays within: {}. \
                 Only change files there. Do not reformat, regenerate or install across the repo, and never commit, stash, reset, checkout or clean: \
                 that would take the others' work with it. If you must change something outside your paths, stop and report failed saying why.\n",
                scope.paths.join(", ")
            ));
        }
        if let Some(git) = &task.git {
            p.push_str(&format!(
                "- You work in your own git worktree on branch `{branch}`, made from `{base}`; other agents work on other branches at the same time. \
                 Commit your work on `{branch}` (MAP commits anything you leave uncommitted). Do not switch branches, push, or touch other worktrees or `{base}`: \
                 after the checks pass MAP lands the branch on `{base}`.\n",
                branch = git.branch,
                base = git.base
            ));
        }
        if let Some(slug) = &task.project {
            p.push_str(&format!(
                "- This is project `{slug}`. Before using an LLM to classify, route or decide something, POST it to $MAP_DECIDE_URL \
                 (with `-H \"Authorization: Bearer $MAP_PROJECT_TOKEN\"`): \
                 {{\"state\": \"<the facts; English reads best, Thai goes through the project's glossary>\", \"questions\": [{{\"id\", \"kind\": \"choice\"|\"score\"|\"noul\", \"prompt\", \"options\"}}]}}. \
                 Each answer has a `lane`; `abstain` means nobody decided, so handle it yourself.\n"
            ));
            p.push_str(
                "- Before you start, search the project's knowledge (its docs, requirements, past tasks and their outcomes, approved plans): \
                 `curl -s -G -H \"Authorization: Bearer $MAP_PROJECT_TOKEN\" \"$MAP_KNOWLEDGE_URL/search\" --data-urlencode \"q=<a few words>\"` \
                 (terms of 3+ characters, Thai or English; `&kind=card|requirement|task|rulebook|doc|memory`), then read a hit with \
                 `curl -s -G -H \"Authorization: Bearer $MAP_PROJECT_TOKEN\" \"$MAP_KNOWLEDGE_URL/doc\" --data-urlencode \"path=<path>\"` or open the file.\n",
            );
        }
        if let Some(failure) = previous_failure {
            p.push_str(&format!("\nThe previous attempt did not pass. Fix this first:\n```\n{}\n```\n", tail(failure, 2500)));
        }
        p
    }

    /// The project's Reflex tier gate: the provider's model for the tier Reflex is sure of, for a task without a
    /// pinned model. `None` (gate off, no tiers for `provider`, Reflex unsure) keeps the usual model.
    async fn tier_gate(&self, task: &Task, provider: &str) -> Option<String> {
        let slug = task.project.as_deref()?;
        if task.model.is_some() || !crate::project_settings::gates(&self.hub, task).await.tier {
            return None;
        }
        let tiers = self.hub.config.orchestrator.tiers.get(provider).filter(|t| !t.is_empty())?;
        // The intake gate's answer is the tier too (light, heavy): used without asking again. Without the intake gate
        // the same question is asked here; unclear or plan first only count at intake, so they keep the usual model.
        let (tier, said) = match task.intake.as_ref().filter(|i| i.tier_asked) {
            Some(i) => (i.tier.clone(), if i.tier.is_some() { "asked at intake".to_string() } else { "unsure at intake".to_string() }),
            None => {
                let said = crate::gates::ask(&self.hub, slug, &crate::gates::task_state(task), crate::gates::intake_question()).await;
                let tier = match &said {
                    crate::gates::Said::Answer { answer, .. } => crate::gates::tier_of(answer).map(String::from),
                    crate::gates::Said::Unsure(_) => None,
                };
                (tier, said.detail())
            }
        };
        let tier = tier.as_deref();
        let model = tier.and_then(|t| tiers.get(t)).map(String::from);
        let detail = match (tier, &model) {
            (Some(t), Some(m)) => format!("tier: {t} · {said} · {provider} → {m}"),
            (Some(t), None) => format!("tier: {t} · {said} · no {t} model for {provider}, usual model"),
            _ => format!("tier: {said} · usual model"),
        };
        record(&self.hub, Kind::tier(tier), task, detail).await;
        model
    }

    /// The project's Reflex audit gate; true when Reflex is sure the change needs no audit, which accepts the task on
    /// verification. Off, unsure, not running or "yes": false, and the audit runs as before.
    async fn audit_gate(&self, task: &Task, dir: &std::path::Path) -> bool {
        let Some(slug) = task.project.as_deref() else { return false };
        if !crate::project_settings::gates(&self.hub, task).await.audit {
            return false;
        }
        // Not running: no need to read the change.
        let said = match crate::gates::not_running(&self.hub) {
            Some(said) => said,
            None => {
                let state = crate::gates::audit_state(&self.hub, task, dir).await;
                crate::gates::ask(&self.hub, slug, &state, crate::gates::needs_audit_question()).await
            }
        };
        match &said {
            crate::gates::Said::Answer { answer, .. } if answer.as_str() == Some("no_audit") => {
                record(&self.hub, Kind::AUDIT_SKIPPED_REFLEX, task, format!("needs_audit: no · {} · accepted on verification", said.detail())).await;
                self.hub.event(Level::Info, "task.audit_skipped", task.session.as_deref(), Some(task.id), "Reflex: no audit needed; accepted on verification").await;
                true
            }
            crate::gates::Said::Answer { .. } => {
                record(&self.hub, Kind::AUDIT_REFLEX_SAID_AUDIT, task, format!("needs_audit: yes · {}", said.detail())).await;
                false
            }
            crate::gates::Said::Unsure(_) => {
                record(&self.hub, Kind::AUDIT_REFLEX_ABSTAINED, task, format!("needs_audit: {}", said.detail())).await;
                false
            }
        }
    }

    /// Returns true when a worker session was started.
    async fn start(&self, mut task: Task) -> Result<bool> {
        let Some(provider) = self.pick_provider(&task).await else {
            let note = "Waiting: no provider is available (missing binary, quota or auth cooldown)".to_string();
            if task.result.as_deref() != Some(note.as_str()) {
                record(&self.hub, Kind::PICK_NONE, &task, dec::pick_detail(&self.hub, &task, None).await).await;
                task.result = Some(note);
                self.hub.put_task(&mut task).await?;
            }
            return Ok(false);
        };
        let spec = self.hub.spec(&provider)?;
        // A model only carries over when it belongs to the chosen provider.
        let usual = task.model.clone().filter(|m| spec.models.contains(m)).or(crate::project_settings::model(&self.hub, &task, &provider).await);
        let model = self.tier_gate(&task, &provider).await.or(usual);
        record(&self.hub, Kind::PICK_CHOSEN, &task, dec::pick_detail(&self.hub, &task, Some(&provider)).await).await;
        task.attempts += 1;
        // `branches.mode = worktree`: the task's own branch and worktree, saved before the launch reads it.
        match crate::worktrees::prepare(&self.hub, &mut task).await {
            Ok(false) => {}
            Ok(true) => {
                let git = task.git.clone().unwrap_or_default();
                record(&self.hub, Kind::PREP_READY, &task, format!("{} from {} · {}", git.branch, git.base, git.worktree.unwrap_or_default())).await;
                self.hub.put_task(&mut task).await?;
            }
            Err(crate::worktrees::PrepareError::Retry(why)) => {
                record(&self.hub, Kind::PREP_FAILED, &task, dec::last_line(&why)).await;
                self.hub.put_task(&mut task).await?;
                self.retry_or_fail(task, &why).await?;
                return Ok(false);
            }
            Err(crate::worktrees::PrepareError::Fatal(why)) => {
                record(&self.hub, Kind::PREP_FATAL, &task, &why).await;
                task.status = TaskStatus::Failed;
                task.result = Some(why.clone());
                self.hub.put_task(&mut task).await?;
                self.hub.event(Level::Error, "task.failed", None, Some(task.id), format!("Failed: {why}")).await;
                self.on_fail(&task);
                return Ok(false);
            }
        }
        let session = format!("{}task-{}-{}", self.hub.config.workspace.session_prefix, task.id, task.attempts);
        // Configure → Hooks: pre_launch, in the task's folder (its worktree, made above). A blocking failure stops
        // the task for a person; no session is started.
        if let Some(reason) = self.blocking_hooks(HookEvent::PreLaunch, &task, Some(&session)).await {
            record(&self.hub, Kind::HOOK_PRE_LAUNCH_BLOCKED, &task, dec::last_line(reason.lines().next().unwrap_or_default())).await;
            self.fail_for_person(task, reason).await?;
            return Ok(false);
        }
        let previous_failure = if task.attempts > 1 { task.result.clone() } else { None };
        let prompt = self.build_prompt(&task, &session, previous_failure.as_deref(), &crate::project_settings::verify(&self.hub, &task).await);
        let req = LaunchRequest {
            name: Some(session.clone()),
            provider: provider.clone(),
            model,
            role: Role::Worker,
            task_id: Some(task.id),
            project: task.project.clone(),
            prompt: Some(prompt),
        };
        match self.hub.launch(req).await {
            Ok(s) => {
                task.status = TaskStatus::Running;
                task.session = Some(s.name.clone());
                task.result = None;
                self.hub.put_task(&mut task).await?;
                record(&self.hub, Kind::LAUNCH_STARTED, &task, format!("hub.launch {} · {provider} ({}) · attempt {}", s.name, s.model, task.attempts)).await;
                self.hub
                    .event(Level::Info, "task.started", Some(&s.name), Some(task.id), format!("Task #{} attempt {} on {provider} ({})", task.id, task.attempts, s.model))
                    .await;
                Ok(true)
            }
            Err(err) => {
                let reason = format!("Could not start {provider}: {err:#}");
                self.hub.event(Level::Error, "task.start_failed", None, Some(task.id), &reason).await;
                record(&self.hub, Kind::LAUNCH_ERROR, &task, format!("{reason} · {provider} cools down 5m")).await;
                self.hub.cool_down(&provider, Utc::now() + chrono::Duration::minutes(5));
                task.result = Some(reason);
                let used = format!("attempt {} of {} used", task.attempts, self.hub.config.orchestrator.max_attempts);
                if task.attempts >= self.hub.config.orchestrator.max_attempts {
                    record(&self.hub, Kind::RETRY_FAILED, &task, used).await;
                    task.status = TaskStatus::Failed;
                } else {
                    record(&self.hub, Kind::RETRY_AGAIN, &task, used).await;
                }
                self.hub.put_task(&mut task).await?;
                if task.status == TaskStatus::Failed {
                    self.on_fail(&task);
                }
                Ok(false)
            }
        }
    }

    async fn check_running(&self, task: Task) -> Result<()> {
        let Some(name) = task.session.clone() else {
            record(&self.hub, Kind::WATCH_LOST, &task, "task is running but has no session").await;
            return self.retry_or_fail(task, "Running without a session").await;
        };
        let result_path = self.hub.result_file(&name);
        if result_path.exists() {
            let text = std::fs::read_to_string(&result_path)?;
            let result: ResultFile = match serde_json::from_str(&text) {
                Ok(r) => r,
                // The agent may still be writing; a broken file is retried next tick, then fails on timeout.
                Err(_) => return Ok(()),
            };
            if result.status == "done" {
                record(&self.hub, Kind::WATCH_DONE, &task, format!("$MAP_RESULT_FILE status done · {}", dec::last_line(&result.summary))).await;
                let mut task = task;
                task.status = TaskStatus::Verifying;
                task.result = Some(result.summary);
                self.hub.put_task(&mut task).await?;
                self.hub.event(Level::Info, "task.reported_done", Some(&name), Some(task.id), "Agent reported done; verifying").await;
                self.spawn_finish(task);
                return Ok(());
            }
            record(&self.hub, Kind::WATCH_FAILED, &task, format!("$MAP_RESULT_FILE status {} · {}", result.status, dec::last_line(&result.summary))).await;
            return self.retry_or_fail(task, &format!("Agent reported failure: {}", result.summary)).await;
        }

        let session = self.hub.store.session(&name).await?;
        match session.as_ref().map(|s| s.state) {
            None | Some(SessionState::Exited) => {
                record(&self.hub, Kind::WATCH_EXITED, &task, format!("session {name} exited · no $MAP_RESULT_FILE")).await;
                self.retry_or_fail(task, "The session ended without writing a result file").await
            }
            Some(SessionState::NeedsAuth) => {
                let provider = session.map(|s| s.provider).unwrap_or_default();
                record(&self.hub, Kind::WATCH_AUTH, &task, format!("session {name} needs_auth · {provider} sign-in")).await;
                self.retry_or_fail(task, &format!("{provider} needs a new sign-in; moving to another provider")).await
            }
            Some(SessionState::NeedsTrust) => {
                // Blocked on the owner, not hung: no retry and no timeout while the question is on screen.
                let detail = session.and_then(|s| s.detail).unwrap_or_else(|| "asks to trust its folder".into());
                record(&self.hub, Kind::WATCH_TRUST, &task, format!("session {name} needs_trust · {detail} · waiting for the owner")).await;
                Ok(())
            }
            _ => {
                let limit = chrono::Duration::minutes(self.hub.config.orchestrator.task_timeout_minutes as i64);
                let started = session.map_or(task.updated_at, |s| s.created_at);
                if Utc::now() - started > limit {
                    record(&self.hub, Kind::WATCH_TIMEOUT, &task, format!("running longer than task_timeout {}m", limit.num_minutes())).await;
                    self.retry_or_fail(task, "Timed out").await
                } else {
                    Ok(())
                }
            }
        }
    }

    /// Ends the worker (unless a person is using it) and either re-queues the task or fails it.
    async fn retry_or_fail(&self, mut task: Task, reason: &str) -> Result<()> {
        let mut reason = reason.to_string();
        if let Some(name) = task.session.clone()
            && self.hub.store.session(&name).await?.is_some_and(|s| s.state.is_live())
        {
            if let Ok(pane) = self.hub.term.capture(&name, 3000, false).await {
                let last = crate::detect::last_assistant_message(&crate::detect::strip_ansi(&pane));
                if !last.is_empty() {
                    reason = format!("{reason}\n\nLast agent message:\n{}", tail(&last, 1500));
                }
            }
            let reason = reason.as_str();
            match self.hub.close_session(&name, false, reason).await? {
                EndDecision::End => {}
                kept => {
                    let why = if kept == EndDecision::KeepInput { "a person is typing" } else { "a client is attached" };
                    record(&self.hub, Kind::RETRY_KEPT, &task, format!("{why} · session {name} left alone")).await;
                    self.hub.event(Level::Warning, "task.kept", Some(&name), Some(task.id), format!("{reason}, but {why}; leaving the session alone")).await;
                    return Ok(());
                }
            }
        }
        let headline = reason.lines().next().unwrap_or_default().chars().take(300).collect::<String>();
        task.result = Some(reason);
        let used = format!("attempt {} of {} used", task.attempts, self.hub.config.orchestrator.max_attempts);
        if task.attempts < self.hub.config.orchestrator.max_attempts {
            record(&self.hub, Kind::RETRY_AGAIN, &task, format!("{used} · failure goes into the next prompt")).await;
            task.status = TaskStatus::Pending;
            self.hub.event(Level::Warning, "task.retry", task.session.as_deref(), Some(task.id), format!("Retrying: {headline}")).await;
        } else {
            record(&self.hub, Kind::RETRY_FAILED, &task, used).await;
            task.status = TaskStatus::Failed;
            self.hub.event(Level::Error, "task.failed", task.session.as_deref(), Some(task.id), format!("Failed: {headline}")).await;
        }
        self.hub.put_task(&mut task).await?;
        if task.status == TaskStatus::Failed {
            self.on_fail(&task);
        }
        Ok(())
    }

    /// Configure → Hooks: `on_fail`, in the background; it never changes the task.
    fn on_fail(&self, task: &Task) {
        crate::hooks::spawn_event(Arc::clone(&self.hub), HookEvent::OnFail, task.clone(), None);
    }

    /// Runs `event`'s hooks; the failure reason when a blocking one failed. A hook runner error (the store) is
    /// logged and lets the task go on.
    async fn blocking_hooks(&self, event: HookEvent, task: &Task, session: Option<&str>) -> Option<String> {
        match crate::hooks::run_event(&self.hub, event, task, session).await {
            Ok(outcome) => outcome.blocked,
            Err(err) => {
                tracing::warn!(task = task.id, "{} hooks: {err:#}", event.as_str());
                None
            }
        }
    }

    /// Fails the task without a retry (a blocking hook failed: a person decides), then runs `on_fail`.
    async fn fail_for_person(&self, mut task: Task, reason: String) -> Result<()> {
        let headline = reason.lines().next().unwrap_or_default().to_string();
        task.status = TaskStatus::Failed;
        task.result = Some(reason);
        self.hub.put_task(&mut task).await?;
        self.hub.event(Level::Error, "task.failed", task.session.as_deref(), Some(task.id), format!("Failed: {headline}")).await;
        self.on_fail(&task);
        Ok(())
    }

    fn spawn_finish(&self, task: Task) {
        if !self.finishing.lock().unwrap().insert(task.id) {
            return;
        }
        let hub = Arc::clone(&self.hub);
        let finishing = Arc::clone(&self.finishing);
        let orch = Orchestrator { hub: Arc::clone(&hub), finishing: Arc::clone(&finishing) };
        tokio::spawn(async move {
            let id = task.id;
            if let Err(err) = orch.finish(task).await {
                hub.event(Level::Error, "task.finish_error", None, Some(id), format!("Finishing failed: {err:#}")).await;
            }
            finishing.lock().unwrap().remove(&id);
        });
    }

    /// Runs the task's verify commands in `dir`; the failure (command and output) when one fails.
    async fn verify(&self, task: &Task, dir: &std::path::Path) -> Result<Option<String>> {
        let hub = &self.hub;
        let verify = crate::project_settings::verify(hub, task).await;
        for (i, command) in verify.iter().enumerate() {
            let of = format!("{} of {}", i + 1, verify.len());
            record(hub, Kind::VERIFY_CMD_START, task, format!("`{command}` · {of}")).await;
            let started = std::time::Instant::now();
            let (ok, output) = shell_for(hub, task.project.as_deref(), dir, command, Duration::from_secs(30 * 60)).await?;
            if !ok {
                record(hub, Kind::VERIFY_FAIL, task, format!("`{command}` failed · {of} · {}", dec::last_line(&output))).await;
                return Ok(Some(format!("`{command}` failed:\n{output}")));
            }
            record(hub, Kind::VERIFY_CMD_PASS, task, format!("`{command}` exit 0 · {of} · {}", dec::secs(started.elapsed()))).await;
        }
        record(hub, Kind::VERIFY_PASS, task, if verify.is_empty() { "no verify commands".to_string() } else { format!("all {} exit 0", verify.len()) }).await;
        Ok(None)
    }

    async fn finish(&self, mut task: Task) -> Result<()> {
        let hub = &self.hub;
        let cfg = &hub.config.orchestrator;
        let dir = hub.task_dir(&task).await;

        if task.status == TaskStatus::Verifying {
            if let Some(failure) = self.verify(&task, &dir).await? {
                return self.retry_or_fail(task, &failure).await;
            }
            // Configure → Hooks: post_verify, before the audit. A blocking failure waits for a person (no retry).
            if let Some(reason) = self.blocking_hooks(HookEvent::PostVerify, &task, None).await {
                record(hub, Kind::HOOK_POST_VERIFY_BLOCKED, &task, dec::last_line(reason.lines().next().unwrap_or_default())).await;
                return self.fail_for_person(task, reason).await;
            }
            if crate::project_settings::audit_enabled(hub, &task).await {
                task.status = TaskStatus::Auditing;
                hub.put_task(&mut task).await?;
            } else {
                record(hub, Kind::AUDIT_OFF, &task, "audit.enabled = false").await;
            }
        }

        if task.status == TaskStatus::Auditing && !self.audit_gate(&task, &dir).await {
            match self.audit(&task).await? {
                Verdict::Pass(by) => {
                    record(hub, Kind::AUDIT_PASS, &task, format!("{by}: VERDICT: PASS")).await;
                    hub.event(Level::Success, "task.audit_pass", task.session.as_deref(), Some(task.id), format!("Audit passed ({by})")).await
                }
                Verdict::Fail(reason) => {
                    record(hub, Kind::AUDIT_FAIL, &task, format!("VERDICT: FAIL · {}", dec::last_line(&reason))).await;
                    return self.retry_or_fail(task, &format!("Audit failed: {reason}")).await;
                }
                Verdict::Unavailable if cfg.audit.required => {
                    record(hub, Kind::AUDIT_NO_AUDITOR, &task, "no auditor in audit.chain answered · audit.required = true").await;
                    return self.retry_or_fail(task, "No auditor was available and audit is required").await;
                }
                Verdict::Unavailable => {
                    record(hub, Kind::AUDIT_SKIPPED, &task, "no auditor in audit.chain answered · audit.required = false").await;
                    hub.event(Level::Warning, "task.audit_skipped", task.session.as_deref(), Some(task.id), "No auditor available; accepted on verification").await
                }
            };
        }

        if task.status != TaskStatus::Landing {
            task.status = TaskStatus::Landing;
            hub.put_task(&mut task).await?;
        }
        if !self.land(&mut task, &dir).await? {
            return Ok(());
        }

        for template in &cfg.post_done {
            let command = template.replace("{id}", &task.id.to_string()).replace("{title}", &quote(&task.title)?);
            let (ok, output) = shell_for(hub, task.project.as_deref(), &dir, &command, Duration::from_secs(10 * 60)).await?;
            if !ok {
                record(hub, Kind::POST_FAIL, &task, format!("`{command}` failed · {}", dec::last_line(&output))).await;
                // The agent's work passed; a failing hook needs a person, not another agent run.
                task.status = TaskStatus::Failed;
                task.result = Some(format!("post_done `{command}` failed:\n{output}"));
                hub.put_task(&mut task).await?;
                hub.event(Level::Error, "task.post_done_failed", task.session.as_deref(), Some(task.id), format!("`{command}` failed")).await;
                self.on_fail(&task);
                return Ok(());
            }
            record(hub, Kind::POST_CMD_PASS, &task, format!("`{command}` exit 0")).await;
        }
        // Configure → Hooks: post_done, after the map.toml list (machine hooks, then the project's).
        let hooks = match crate::hooks::run_event(hub, HookEvent::PostDone, &task, None).await {
            Ok(o) => o,
            Err(err) => {
                tracing::warn!(task = task.id, "post_done hooks: {err:#}");
                Default::default()
            }
        };
        if let Some(reason) = hooks.blocked {
            let headline = reason.lines().next().unwrap_or_default().to_string();
            record(hub, Kind::POST_FAIL, &task, dec::last_line(&headline)).await;
            task.status = TaskStatus::Failed;
            task.result = Some(reason);
            hub.put_task(&mut task).await?;
            hub.event(Level::Error, "task.post_done_failed", task.session.as_deref(), Some(task.id), headline).await;
            self.on_fail(&task);
            return Ok(());
        }
        let total = cfg.post_done.len() + hooks.ran;
        let post = match (total, hooks.failures.len()) {
            (0, _) => "no post_done hooks".to_string(),
            (n, 0) => format!("all {n} hooks exit 0"),
            (n, f) => format!("{} of {n} hooks exit 0 · {f} non-blocking failed", n - f),
        };
        record(hub, Kind::POST_PASS, &task, post).await;

        let closed = if let Some(name) = task.session.clone()
            && hub.store.session(&name).await?.is_some_and(|s| s.state.is_live())
            && hub.close_session(&name, false, "Task done").await? != EndDecision::End
        {
            hub.event(Level::Info, "task.kept", Some(&name), Some(task.id), "Task done; session left open because a person is using it").await;
            record(hub, Kind::END_KEPT, &task, format!("a client is attached or the input line has text · {name} left open")).await;
            false
        } else {
            record(hub, Kind::END_CLOSED, &task, "no client attached · input line empty · worker session ended").await;
            true
        };
        // The branch landed (or had nothing): its worktree goes, unless a person is still in the session. A pushed
        // branch stays for its pull request.
        if closed && let Some(l) = crate::worktrees::Lander::of(hub, &task).await {
            let pushed = task.git.as_ref().is_some_and(|g| g.pushed);
            l.remove(!pushed).await;
            if let Some(git) = task.git.as_mut() {
                git.worktree = None;
            }
        }
        task.status = TaskStatus::Done;
        hub.put_task(&mut task).await?;
        hub.event(Level::Success, "task.done", task.session.as_deref(), Some(task.id), format!("Task #{} done: {}", task.id, task.title)).await;
        Ok(())
    }

    /// Lands the task's branch (`worktrees.rs`); `Ok(true)` goes on to post_done. A conflict or a failed verify
    /// after the rebase goes back to the agent; keep or a block parks the task in `review`.
    async fn land(&self, task: &mut Task, dir: &std::path::Path) -> Result<bool> {
        use crate::worktrees::{Landed, Lander, land_lock};
        let hub = &self.hub;
        let Some(l) = Lander::of(hub, task).await else {
            let why = if task.git.is_some() { "the task's worktree is gone · nothing to land" } else { "no worktree · the task ran in the project folder" };
            record(hub, Kind::LAND_SHARED, task, why).await;
            return Ok(true);
        };
        let _one_at_a_time = land_lock(task.project.as_deref().unwrap_or_default()).await;
        if let Some(sha) = l.commit(task).await? {
            record(hub, Kind::LAND_COMMITTED, task, format!("{} {}", l.branch, short(&sha))).await;
        }
        if l.ahead().await? == 0 {
            record(hub, Kind::LAND_NOTHING, task, format!("{} has no commits beyond {}", l.branch, l.base)).await;
            self.note(task, format!("nothing to land: `{}` has no commits beyond `{}`", l.branch, l.base)).await?;
            return Ok(true);
        }
        let landed = match l.mode {
            LandMode::Keep => Landed::Keep,
            LandMode::PullRequest => l.push(hub, task).await?,
            LandMode::Merge => match l.rebase().await? {
                Err(conflict) => conflict,
                Ok(rebased) => {
                    if rebased {
                        record(hub, Kind::LAND_REBASED, task, format!("{} rebased onto {}", l.branch, l.base)).await;
                        if let Some(failure) = self.verify(task, dir).await? {
                            record(hub, Kind::LAND_REVERIFY_FAIL, task, dec::last_line(&failure)).await;
                            let why = format!("After MAP rebased `{}` onto `{}` (it moved on while you worked), {failure}", l.branch, l.base);
                            self.retry_or_fail(task.clone(), &why).await?;
                            return Ok(false);
                        }
                    }
                    l.fast_forward().await?
                }
            },
        };
        match landed {
            Landed::Merged(sha) => {
                record(hub, Kind::LAND_MERGED, task, format!("{} → {} at {}", l.branch, l.base, short(&sha))).await;
                if let Some(git) = task.git.as_mut() {
                    git.merged = Some(sha.clone());
                }
                self.note(task, format!("merged into `{}` at {}", l.base, short(&sha))).await?;
                hub.event(Level::Success, "task.merged", task.session.as_deref(), Some(task.id), format!("Task #{} merged into {}", task.id, l.base)).await;
                Ok(true)
            }
            Landed::Pushed { pr_url, note } => {
                record(hub, Kind::LAND_PUSHED, task, &note).await;
                if let Some(git) = task.git.as_mut() {
                    git.pushed = true;
                    git.pr_url = pr_url;
                }
                self.note(task, note.clone()).await?;
                hub.event(Level::Success, "task.pushed", task.session.as_deref(), Some(task.id), note).await;
                Ok(true)
            }
            Landed::Conflict(why) => {
                record(hub, Kind::LAND_CONFLICT, task, dec::last_line(&why)).await;
                self.retry_or_fail(task.clone(), &why).await?;
                Ok(false)
            }
            Landed::Keep => {
                self.review(task, Kind::LAND_KEEP, format!("land = keep: `{}` waits for you to merge it into `{}`", l.branch, l.base)).await?;
                Ok(false)
            }
            Landed::Blocked(why) => {
                self.review(task, Kind::LAND_BLOCKED, why).await?;
                Ok(false)
            }
        }
    }

    async fn note(&self, task: &mut Task, note: String) -> Result<()> {
        if let Some(git) = task.git.as_mut() {
            git.note = Some(note);
        }
        self.hub.put_task(task).await
    }

    /// Parks the task in `review` for the owner, ending its worker session unless a person is using it.
    async fn review(&self, task: &mut Task, kind: Kind, why: String) -> Result<()> {
        let hub = &self.hub;
        record(hub, kind, task, &why).await;
        if let Some(name) = task.session.clone()
            && hub.store.session(&name).await?.is_some_and(|s| s.state.is_live())
        {
            hub.close_session(&name, false, "Task waits for review").await?;
        }
        task.status = TaskStatus::Review;
        if let Some(git) = task.git.as_mut() {
            git.note = Some(why.clone());
            git.land = None;
        }
        hub.put_task(task).await?;
        hub.event(Level::Warning, "task.review", task.session.as_deref(), Some(task.id), format!("Task #{} waits for review: {why}", task.id)).await;
        Ok(())
    }

    async fn audit(&self, task: &Task) -> Result<Verdict> {
        let hub = &self.hub;
        let prompt = format!(
            "You are a strict code auditor. Task #{id} \"{title}\" was just completed in this repository and passed its verification commands.\n\
             Task description:\n{body}\n\nWorker summary:\n{summary}\n\n\
             {changes} Check that the change does what the task asks, \
             has tests, and leaves nothing broken or half-done.\nAnswer with exactly one final line:\nVERDICT: PASS\nor\nVERDICT: FAIL: <reason>",
            changes = match &task.git {
                Some(g) => format!(
                    "The work is on branch `{b}`, made from `{base}`: inspect `git log {base}..HEAD`, `git diff $(git merge-base {base} HEAD)` and git status (uncommitted changes count too).",
                    b = g.branch,
                    base = g.base
                ),
                None => "Inspect the working tree and recent changes (git status, git diff, git log -5).".to_string(),
            },
            id = task.id,
            title = task.title,
            body = task.prompt.trim(),
            summary = task.result.as_deref().unwrap_or("(none)"),
        );
        for provider in &crate::project_settings::audit_chain(hub, task).await {
            if !hub.provider_available(provider) || hub.cooldown_until(provider).is_some() {
                record(hub, Kind::AUDIT_UNAVAILABLE, task, format!("{provider} · not found or cooling down")).await;
                continue;
            }
            let spec = hub.spec(provider)?;
            if spec.headless.is_empty() {
                record(hub, Kind::AUDIT_UNAVAILABLE, task, format!("{provider} · no headless command")).await;
                continue;
            }
            let argv: Vec<String> = spec
                .headless
                .iter()
                .map(|a| a.replace("{binary}", &spec.binary).replace("{model}", &spec.default_model).replace("{session}", "audit").replace("{prompt}", &prompt))
                .collect();
            let vars = crate::env_vars::run_vars(hub, task.project.as_deref()).await;
            let mut extra = hub.command_env();
            extra.extend(vars.iter().map(|v| (v.key.clone(), v.value.clone())));
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]).current_dir(hub.task_dir(task).await).envs(hub.run_env(provider, extra));
            hub.event(Level::Info, "task.audit", None, Some(task.id), format!("Auditing with {provider}")).await;
            record(hub, Kind::AUDIT_ASK, task, format!("{provider} · reviewing git status, git diff, git log -5")).await;
            let (ok, output) = run_captured(cmd, Duration::from_secs(hub.config.orchestrator.audit.timeout_secs)).await?;
            let output = crate::env_vars::redact(&output, &crate::env_vars::secret_values(&vars));
            if let Some(line) = output.lines().rev().find(|l| l.trim_start().starts_with("VERDICT:")) {
                let verdict = line.trim_start().trim_start_matches("VERDICT:").trim();
                if verdict == "PASS" {
                    return Ok(Verdict::Pass(provider.clone()));
                }
                if let Some(reason) = verdict.strip_prefix("FAIL") {
                    return Ok(Verdict::Fail(format!("{provider}: {}", reason.trim_start_matches(':').trim())));
                }
            }
            if !ok || looks_like_quota(&output) {
                hub.cool_down(provider, Utc::now() + chrono::Duration::minutes(hub.config.orchestrator.provider_cooldown_minutes as i64));
            }
            hub.event(Level::Warning, "task.audit_unavailable", None, Some(task.id), format!("{provider} gave no verdict; trying the next auditor")).await;
            record(hub, Kind::AUDIT_UNAVAILABLE, task, format!("{provider} · no VERDICT line")).await;
        }
        Ok(Verdict::Unavailable)
    }

    pub async fn retry(hub: &Hub, id: i64) -> Result<Task> {
        let mut task = hub.store.task(id).await?.context("unknown task")?;
        if !matches!(task.status, TaskStatus::Failed | TaskStatus::Cancelled) {
            bail!("only failed or cancelled tasks can be retried");
        }
        task.status = TaskStatus::Pending;
        task.attempts = 0;
        hub.put_task(&mut task).await?;
        Ok(task)
    }

    pub async fn cancel(hub: &Hub, id: i64) -> Result<Task> {
        let mut task = hub.store.task(id).await?.context("unknown task")?;
        if task.status.is_terminal() {
            bail!("task is already {}", task.status.as_str());
        }
        if let Some(name) = task.session.clone()
            && hub.store.session(&name).await?.is_some_and(|s| s.state.is_live())
        {
            hub.close_session(&name, true, "Task cancelled").await?;
        }
        task.status = TaskStatus::Cancelled;
        hub.put_task(&mut task).await?;
        hub.event(Level::Warning, "task.cancelled", task.session.as_deref(), Some(id), format!("Task #{id} cancelled")).await;
        Ok(task)
    }

    async fn in_review(hub: &Hub, id: i64) -> Result<Task> {
        let task = hub.store.task(id).await?.context("unknown task")?;
        if task.status != TaskStatus::Review {
            bail!("task #{id} is {}, not waiting for review", task.status.as_str());
        }
        Ok(task)
    }

    /// The owner lands a task from `review` (`mode` instead of the project's `branches.land`); it runs in the
    /// background like the orchestrator's own landing, paused orchestrator or not.
    pub async fn land_now(hub: &Arc<Hub>, id: i64, mode: LandMode) -> Result<Task> {
        let mut task = Self::in_review(hub, id).await?;
        let Some(git) = task.git.as_mut() else { bail!("task #{id} has no branch to land") };
        if git.worktree.as_deref().is_none_or(|w| !std::path::Path::new(w).join(".git").exists()) {
            bail!("task #{id}'s worktree is gone; send it back to an agent instead");
        }
        git.land = Some(mode);
        let word = if mode == LandMode::PullRequest { "push and open a pull request" } else { "merge" };
        let detail = format!("owner: {word} {} into {}", git.branch, git.base);
        record(hub, Kind::REVIEW_LAND, &task, detail).await;
        task.status = TaskStatus::Landing;
        hub.put_task(&mut task).await?;
        Orchestrator::new(Arc::clone(hub)).spawn_finish(task.clone());
        Ok(task)
    }

    /// Back to an agent from `review`, on the same branch, with the owner's note in its prompt.
    pub async fn send_back(hub: &Hub, id: i64, note: Option<&str>) -> Result<Task> {
        let mut task = Self::in_review(hub, id).await?;
        let note = note.map(str::trim).filter(|n| !n.is_empty()).unwrap_or("Look at the change again and improve it.");
        record(hub, Kind::REVIEW_SEND_BACK, &task, dec::last_line(note)).await;
        task.status = TaskStatus::Pending;
        // The next attempt's prompt carries `result` as what to fix first (it does from the second attempt on).
        task.attempts = task.attempts.max(1);
        task.result = Some(format!("The owner reviewed your branch and sent it back:\n{note}"));
        if let Some(git) = task.git.as_mut() {
            git.land = None;
        }
        hub.put_task(&mut task).await?;
        hub.event(Level::Info, "task.sent_back", None, Some(id), format!("Task #{id} sent back to an agent")).await;
        Ok(task)
    }

    /// Removes the task's worktree and its branch (unless pushed); a task in review or failed is cancelled.
    pub async fn discard(hub: &Hub, id: i64) -> Result<Task> {
        let mut task = hub.store.task(id).await?.context("unknown task")?;
        if !matches!(task.status, TaskStatus::Review | TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled) {
            bail!("task #{id} is {}: cancel it before discarding its branch", task.status.as_str());
        }
        if task.git.is_none() {
            bail!("task #{id} has no branch or worktree");
        }
        crate::worktrees::discard(hub, &mut task).await;
        if matches!(task.status, TaskStatus::Review | TaskStatus::Failed) {
            if let Some(name) = task.session.clone()
                && hub.store.session(&name).await?.is_some_and(|s| s.state.is_live())
            {
                hub.close_session(&name, true, "Task discarded").await?;
            }
            task.status = TaskStatus::Cancelled;
        }
        if let Some(git) = task.git.as_mut() {
            git.note = Some(if git.pushed { "worktree removed; the pushed branch stays on the remote".into() } else { "worktree and branch removed".into() });
        }
        hub.put_task(&mut task).await?;
        hub.event(Level::Warning, "task.discarded", None, Some(id), format!("Task #{id}: worktree removed")).await;
        Ok(task)
    }
}

enum Verdict {
    Pass(String),
    Fail(String),
    Unavailable,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::store::{NewTask, Store};
    use crate::terminal::fake::FakeTerminal;
    use std::path::Path;

    /// A config whose `claude` provider is `/bin/sh` so it counts as installed.
    async fn setup(extra: &str) -> (Arc<Hub>, Arc<FakeTerminal>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[orchestrator]\nprovider_order = [\"claude\"]\n{extra}\n[providers.claude]\nbinary = \"/bin/sh\"\n",
            dir.path().display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        let term = Arc::new(FakeTerminal::default());
        let hub = Hub::new(config, Store::memory().await.unwrap(), term.clone());
        (hub, term, dir)
    }

    fn new_task(title: &str, deps: Vec<i64>) -> NewTask {
        NewTask { title: title.into(), prompt: format!("do {title}"), depends_on: deps, provider: None, model: None, project: None }
    }

    async fn settle(orch: &mut Orchestrator) {
        orch.tick().await.unwrap();
        for _ in 0..50 {
            if orch.finishing.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn runs_dependencies_in_order_and_finishes_on_result_file() {
        let (hub, term, _dir) = setup("verify = [\"true\"]").await;
        let a = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let b = hub.store.create_task(new_task("b", vec![a.id])).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());

        orch.tick().await.unwrap();
        let a_now = hub.store.task(a.id).await.unwrap().unwrap();
        assert_eq!(a_now.status, TaskStatus::Running);
        assert_eq!(hub.store.task(b.id).await.unwrap().unwrap().status, TaskStatus::Pending);
        let session = a_now.session.unwrap();
        let script = term.sessions.lock().unwrap()[&session].script.clone();
        assert!(script.contains("MAP_RESULT_FILE"), "{script}");

        std::fs::write(hub.result_file(&session), r#"{"status":"done","summary":"did a"}"#).unwrap();
        settle(&mut orch).await;
        let a_now = hub.store.task(a.id).await.unwrap().unwrap();
        assert_eq!(a_now.status, TaskStatus::Done, "{:?}", a_now.result);
        assert!(!term.sessions.lock().unwrap().contains_key(&session), "worker session should be closed");

        orch.tick().await.unwrap();
        assert_eq!(hub.store.task(b.id).await.unwrap().unwrap().status, TaskStatus::Running);
    }

    #[tokio::test]
    async fn failed_verification_retries_with_the_failure_in_the_prompt() {
        let (hub, _term, _dir) = setup("verify = [\"echo broken-test; false\"]\nmax_attempts = 2").await;
        let t = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        let s1 = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
        std::fs::write(hub.result_file(&s1), r#"{"status":"done","summary":"x"}"#).unwrap();
        settle(&mut orch).await;
        let after = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(after.status, TaskStatus::Pending);

        orch.tick().await.unwrap();
        let after = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!((after.status, after.attempts), (TaskStatus::Running, 2));
        let prompt = std::fs::read_to_string(hub.run_dir(after.session.as_ref().unwrap()).join("prompt.md")).unwrap();
        assert!(prompt.contains("broken-test"), "{prompt}");

        std::fs::write(hub.result_file(after.session.as_ref().unwrap()), r#"{"status":"done","summary":"x"}"#).unwrap();
        settle(&mut orch).await;
        assert_eq!(hub.store.task(t.id).await.unwrap().unwrap().status, TaskStatus::Failed);
    }

    #[tokio::test]
    async fn verify_commands_get_the_env_vars_and_never_echo_a_secret() {
        let (hub, _term, _dir) = setup("verify = [\"echo \\\"plain=$PLAIN_V secret=$SECRET_V\\\"; false\"]\nmax_attempts = 2").await;
        let vars = vec![
            crate::model::EnvVarInput { key: "PLAIN_V".into(), value: Some("plainvalue".into()), secret: false },
            crate::model::EnvVarInput { key: "SECRET_V".into(), value: Some("hunter2-very-secret".into()), secret: true },
        ];
        crate::env_vars::replace(&hub, None, vars).await.unwrap();
        let t = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        let s1 = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
        std::fs::write(hub.result_file(&s1), r#"{"status":"done","summary":"x"}"#).unwrap();
        settle(&mut orch).await;
        orch.tick().await.unwrap();
        let after = hub.store.task(t.id).await.unwrap().unwrap();
        let prompt = std::fs::read_to_string(hub.run_dir(after.session.as_ref().unwrap()).join("prompt.md")).unwrap();
        assert!(prompt.contains("plain=plainvalue secret=***") && !prompt.contains("hunter2"), "{prompt}");
        let events = serde_json::to_string(&hub.store.events(1000, None).await.unwrap()).unwrap();
        let trace = serde_json::to_string(&hub.store.events_of_tasks(&[t.id]).await.unwrap()).unwrap();
        assert!(!events.contains("hunter2") && !trace.contains("hunter2"));
        let tasks = serde_json::to_string(&hub.store.tasks().await.unwrap()).unwrap();
        assert!(!tasks.contains("hunter2"), "{tasks}");
    }

    #[tokio::test]
    async fn worker_with_a_person_attached_is_not_killed() {
        let (hub, term, _dir) = setup("max_attempts = 1").await;
        let t = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        let s = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
        term.sessions.lock().unwrap().get_mut(&s).unwrap().attached = 1;
        std::fs::write(hub.result_file(&s), r#"{"status":"failed","summary":"stuck"}"#).unwrap();
        orch.tick().await.unwrap();
        assert!(term.sessions.lock().unwrap().contains_key(&s));
        assert_eq!(hub.store.task(t.id).await.unwrap().unwrap().status, TaskStatus::Running);
    }

    #[tokio::test]
    async fn a_worker_at_the_trust_question_waits_for_the_owner() {
        // A zero timeout would retry any other running task at once.
        let (hub, term, _dir) = setup("task_timeout_minutes = 0").await;
        let t = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        let s = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
        term.set_pane(&s, crate::detect::CLAUDE_TRUST_PANE);
        crate::watcher::Watcher::new(hub.clone()).tick(Utc::now()).await.unwrap();
        assert_eq!(hub.store.session(&s).await.unwrap().unwrap().state, SessionState::NeedsTrust);
        orch.tick().await.unwrap();
        orch.tick().await.unwrap();
        let task = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!((task.status, task.attempts), (TaskStatus::Running, 1), "not done, not retried, not timed out");
        assert!(term.sessions.lock().unwrap().contains_key(&s), "the worker is kept");
        let kinds = decisions_of(&hub, t.id).await;
        assert_eq!(kinds.iter().filter(|k| *k == "watch.trust").count(), 1, "{kinds:?}");
        assert!(!kinds.iter().any(|k| k.starts_with("retry.") || k == "watch.timeout"), "{kinds:?}");
    }

    #[tokio::test]
    async fn project_task_runs_in_the_project_workspace_with_its_decide_url() {
        // No Reflex binary here: the project still works, its engine just reports an error.
        let (hub, term, _dir) = setup("[reflex]\nbinary = \"/nonexistent/reflex\"").await;
        let project = crate::projects::create(
            &hub,
            crate::projects::NewProject { name: "Support".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None },
        )
        .await
        .unwrap();
        assert!(hub.workspace_of(&project).is_dir());
        let t = hub.store.create_task(NewTask { project: Some(project.slug.clone()), ..new_task("a", vec![]) }).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let session = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
        let script = term.sessions.lock().unwrap()[&session].script.clone();
        assert!(script.contains("MAP_DECIDE_URL=http://127.0.0.1:4400/api/projects/support/decide"), "{script}");
        assert_eq!(hub.store.session(&session).await.unwrap().unwrap().project.as_deref(), Some("support"));
        let prompt = std::fs::read_to_string(hub.run_dir(&session).join("prompt.md")).unwrap();
        assert!(prompt.contains("$MAP_DECIDE_URL"), "{prompt}");
    }

    // ---- a branch and worktree per task (worktrees.rs)

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git").args(["-c", "user.name=T", "-c", "user.email=t@t"]).args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// A project on a git folder (`main`, one commit of README) with `branches` set; two workers at once, no audit.
    async fn worktree_project(land: &str, max_attempts: u32) -> (Arc<Hub>, Arc<FakeTerminal>, tempfile::TempDir, std::path::PathBuf, String) {
        let extra = format!("max_parallel = 2\nmax_attempts = {max_attempts}\nverify = [\"test -f README\"]\n[orchestrator.audit]\nenabled = false");
        let (hub, term, dir) = setup(&extra).await;
        let folder = dir.path().join("app");
        std::fs::create_dir_all(&folder).unwrap();
        git(&folder, &["init", "-q", "-b", "main"]);
        std::fs::write(folder.join("README"), "one\ntwo\nthree\n").unwrap();
        git(&folder, &["add", "."]);
        git(&folder, &["commit", "-q", "-m", "first"]);
        let land = serde_json::from_value(serde_json::json!(land)).unwrap();
        let settings = crate::model::ProjectSettings {
            source: crate::model::ProjectSource { kind: crate::model::SourceKind::Local, location: Some(folder.display().to_string()), ..Default::default() },
            branches: crate::model::ProjectBranches { mode: crate::model::BranchMode::Worktree, land, ..Default::default() },
            ..Default::default()
        };
        let new = crate::projects::NewProject { name: "App".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let slug = crate::projects::create_with(&hub, new, settings).await.unwrap().0.slug;
        (hub, term, dir, folder, slug)
    }

    /// The task's worktree, after the worker has changed `file` there and reported done.
    async fn work(hub: &Hub, id: i64, file: &str, text: &str) -> std::path::PathBuf {
        let t = hub.store.task(id).await.unwrap().unwrap();
        let wt = std::path::PathBuf::from(t.git.as_ref().unwrap().worktree.clone().unwrap());
        std::fs::write(wt.join(file), text).unwrap();
        std::fs::write(hub.result_file(t.session.as_ref().unwrap()), format!(r#"{{"status":"done","summary":"changed {file}"}}"#)).unwrap();
        wt
    }

    async fn settle_all(orch: &mut Orchestrator) {
        for _ in 0..3 {
            settle(orch).await;
        }
    }

    #[tokio::test]
    async fn two_tasks_run_side_by_side_on_their_own_branches_and_both_land() {
        let (hub, term, _dir, folder, slug) = worktree_project("merge", 2).await;
        let a = hub.store.create_task(NewTask { project: Some(slug.clone()), ..new_task("a", vec![]) }).await.unwrap();
        let b = hub.store.create_task(NewTask { project: Some(slug.clone()), ..new_task("b", vec![]) }).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        let (ta, tb) = (hub.store.task(a.id).await.unwrap().unwrap(), hub.store.task(b.id).await.unwrap().unwrap());
        assert_eq!((ta.status, tb.status), (TaskStatus::Running, TaskStatus::Running));
        let (ga, gb) = (ta.git.clone().unwrap(), tb.git.clone().unwrap());
        assert_eq!((ga.branch.as_str(), ga.base.as_str()), (format!("map/task-{}", a.id).as_str(), "main"));
        assert_ne!(ga.worktree, gb.worktree);
        // The worker starts in its worktree and is told about its branch.
        let cwd = term.sessions.lock().unwrap()[ta.session.as_ref().unwrap()].cwd.clone();
        assert_eq!(cwd.display().to_string(), ga.worktree.clone().unwrap());
        let prompt = std::fs::read_to_string(hub.run_dir(ta.session.as_ref().unwrap()).join("prompt.md")).unwrap();
        assert!(prompt.contains(&format!("branch `{}`", ga.branch)), "{prompt}");

        // The workers leave their changes uncommitted; MAP commits and lands both, the second after a rebase.
        work(&hub, a.id, "a.txt", "from a\n").await;
        work(&hub, b.id, "b.txt", "from b\n").await;
        settle_all(&mut orch).await;
        for id in [a.id, b.id] {
            let t = hub.store.task(id).await.unwrap().unwrap();
            assert_eq!(t.status, TaskStatus::Done, "{:?} {:?}", t.result, t.git);
            let g = t.git.unwrap();
            assert!(g.merged.is_some() && g.worktree.is_none(), "{g:?}");
            assert!(!std::path::Path::new(&crate::worktrees::worktree_path(&hub, &slug, id)).exists());
        }
        // The project folder has main checked out, now with both changes and the owner's history.
        assert_eq!(std::fs::read_to_string(folder.join("a.txt")).unwrap(), "from a\n");
        assert_eq!(std::fs::read_to_string(folder.join("b.txt")).unwrap(), "from b\n");
        assert_eq!(git(&folder, &["rev-list", "--count", "main"]), "3");
        assert_eq!(git(&folder, &["branch", "--list", "map/*"]), "", "landed branches are deleted");
        assert!(git(&folder, &["log", "-1", "--format=%B"]).contains("MAP-Task:"));
        let kinds = decisions_of(&hub, b.id).await;
        assert!(kinds.iter().any(|k| k == "land.merged"), "{kinds:?}");
    }

    #[tokio::test]
    async fn a_conflict_goes_back_to_the_agent_on_the_same_branch() {
        let (hub, _term, _dir, folder, slug) = worktree_project("merge", 3).await;
        let a = hub.store.create_task(NewTask { project: Some(slug.clone()), ..new_task("a", vec![]) }).await.unwrap();
        let b = hub.store.create_task(NewTask { project: Some(slug.clone()), ..new_task("b", vec![]) }).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        work(&hub, a.id, "README", "one\nTWO from a\nthree\n").await;
        settle_all(&mut orch).await;
        assert_eq!(hub.store.task(a.id).await.unwrap().unwrap().status, TaskStatus::Done);
        let wt = work(&hub, b.id, "README", "one\nTWO from b\nthree\n").await;
        settle(&mut orch).await;
        let tb = hub.store.task(b.id).await.unwrap().unwrap();
        assert_eq!(tb.status, TaskStatus::Pending, "{:?}", tb.result);
        assert!(tb.result.as_deref().unwrap().contains("conflicts (README)"), "{:?}", tb.result);
        assert!(wt.join(".git").exists(), "the worktree stays for the next attempt");
        assert!(std::fs::read_to_string(folder.join("README")).unwrap().contains("from a"));

        // The next attempt runs in the same worktree, with the conflict in its prompt.
        orch.tick().await.unwrap();
        let tb = hub.store.task(b.id).await.unwrap().unwrap();
        assert_eq!((tb.status, tb.git.as_ref().unwrap().worktree.clone()), (TaskStatus::Running, Some(wt.display().to_string())));
        let prompt = std::fs::read_to_string(hub.run_dir(tb.session.as_ref().unwrap()).join("prompt.md")).unwrap();
        assert!(prompt.contains("git rebase main"), "{prompt}");
    }

    #[tokio::test]
    async fn keep_waits_for_the_owner_who_merges_or_discards() {
        let (hub, _term, _dir, folder, slug) = worktree_project("keep", 2).await;
        let a = hub.store.create_task(NewTask { project: Some(slug.clone()), ..new_task("a", vec![]) }).await.unwrap();
        let b = hub.store.create_task(NewTask { project: Some(slug.clone()), ..new_task("b", vec![]) }).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        work(&hub, a.id, "a.txt", "a\n").await;
        let wt_b = work(&hub, b.id, "b.txt", "b\n").await;
        settle_all(&mut orch).await;
        for id in [a.id, b.id] {
            assert_eq!(hub.store.task(id).await.unwrap().unwrap().status, TaskStatus::Review);
        }
        assert!(!folder.join("a.txt").exists(), "nothing lands without the owner");

        Orchestrator::land_now(&hub, a.id, LandMode::Merge).await.unwrap();
        for _ in 0..100 {
            if hub.finishing.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(hub.store.task(a.id).await.unwrap().unwrap().status, TaskStatus::Done);
        assert!(folder.join("a.txt").exists());

        let tb = Orchestrator::discard(&hub, b.id).await.unwrap();
        assert_eq!(tb.status, TaskStatus::Cancelled);
        assert!(!wt_b.exists());
        assert_eq!(git(&folder, &["branch", "--list", &format!("map/task-{}", b.id)]), "");
        assert!(Orchestrator::land_now(&hub, b.id, LandMode::Merge).await.is_err());
    }

    #[tokio::test]
    async fn changes_in_the_project_folder_park_the_task_in_review() {
        let (hub, _term, _dir, folder, slug) = worktree_project("merge", 2).await;
        let a = hub.store.create_task(NewTask { project: Some(slug.clone()), ..new_task("a", vec![]) }).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        // The owner edits README in the project folder while the agent changes it on its branch.
        std::fs::write(folder.join("README"), "one\nmine\nthree\n").unwrap();
        work(&hub, a.id, "README", "one\nagent\nthree\n").await;
        settle_all(&mut orch).await;
        let t = hub.store.task(a.id).await.unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Review, "{:?}", t.git);
        assert!(t.git.as_ref().unwrap().note.as_deref().unwrap().contains("Commit or stash"), "{:?}", t.git);
        assert_eq!(std::fs::read_to_string(folder.join("README")).unwrap(), "one\nmine\nthree\n", "the owner's edit is untouched");

        // Sent back, it runs again on the same branch with the owner's note.
        let t = Orchestrator::send_back(&hub, a.id, Some("keep my wording")).await.unwrap();
        assert_eq!(t.status, TaskStatus::Pending);
        orch.tick().await.unwrap();
        let t = hub.store.task(a.id).await.unwrap().unwrap();
        let prompt = std::fs::read_to_string(hub.run_dir(t.session.as_ref().unwrap()).join("prompt.md")).unwrap();
        assert!(prompt.contains("keep my wording"), "{prompt}");
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_repo_fails_the_task_with_a_reason() {
        let (hub, _term, dir) = setup("").await;
        let folder = dir.path().join("plain");
        std::fs::create_dir_all(&folder).unwrap();
        let settings = crate::model::ProjectSettings {
            source: crate::model::ProjectSource { kind: crate::model::SourceKind::Local, location: Some(folder.display().to_string()), ..Default::default() },
            branches: crate::model::ProjectBranches { mode: crate::model::BranchMode::Worktree, ..Default::default() },
            ..Default::default()
        };
        let new = crate::projects::NewProject { name: "Plain".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let slug = crate::projects::create_with(&hub, new, settings).await.unwrap().0.slug;
        // Creating a local project may have made it a repo; make sure it has no commit.
        let _ = std::fs::remove_dir_all(folder.join(".git"));
        let t = hub.store.create_task(NewTask { project: Some(slug), ..new_task("a", vec![]) }).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let t = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Failed);
        assert!(t.result.as_deref().unwrap().contains("not a git repo with a commit"), "{:?}", t.result);
    }

    #[tokio::test]
    async fn the_analyst_clears_independent_tasks_to_share_one_folder() {
        let dir = tempfile::tempdir().unwrap();
        // The analyst stand-in: answers for tasks 1 and 2, whatever it is asked.
        let analyst = dir.path().join("analyst.sh");
        std::fs::write(&analyst, "#!/bin/sh\necho 'looked at the code'\necho '{\"tasks\": [{\"id\": 1, \"paths\": [\"src/a/\"], \"independent\": true, \"reason\": \"own module\"}, {\"id\": 2, \"paths\": [\"src/b/\"], \"independent\": true, \"reason\": \"own module\"}]}'\n").unwrap();
        std::fs::set_permissions(&analyst, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let text = format!(
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[orchestrator]\nprovider_order = [\"claude\"]\nmax_parallel = 4\n[orchestrator.audit]\nchain = [\"fake\"]\n[providers.claude]\nbinary = \"/bin/sh\"\n[providers.fake]\nbinary = \"{1}\"\ninteractive = [\"{{binary}}\"]\nheadless = [\"{{binary}}\"]\n",
            dir.path().display(),
            analyst.display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        let hub = Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default()));
        let a = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let b = hub.store.create_task(new_task("b", vec![])).await.unwrap();
        assert_eq!((a.id, b.id), (1, 2));
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        // One folder: a starts, b waits for the analyst, who is started for both.
        assert_eq!(hub.store.task(a.id).await.unwrap().unwrap().status, TaskStatus::Running);
        assert_eq!(hub.store.task(b.id).await.unwrap().unwrap().status, TaskStatus::Pending);
        assert!(decisions_of(&hub, b.id).await.contains(&"ready.awaiting_analyst".to_string()));
        for _ in 0..100 {
            if hub.store.task(b.id).await.unwrap().unwrap().scope.is_some() && hub.store.task(a.id).await.unwrap().unwrap().scope.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let scope = hub.store.task(b.id).await.unwrap().unwrap().scope.unwrap();
        assert_eq!((scope.paths.clone(), scope.independent, scope.by.as_str()), (vec!["src/b/".to_string()], true, "analyst:fake"));
        tokio::time::sleep(Duration::from_millis(50)).await;
        orch.tick().await.unwrap();
        let b = hub.store.task(b.id).await.unwrap().unwrap();
        assert_eq!(b.status, TaskStatus::Running, "cleared: disjoint paths");
        assert!(decisions_of(&hub, b.id).await.contains(&"ready.cleared".to_string()));
        let prompt = std::fs::read_to_string(hub.run_dir(b.session.as_ref().unwrap()).join("prompt.md")).unwrap();
        assert!(prompt.contains("Only change files there") && prompt.contains("src/b/"), "{prompt}");

        // A third task the analyst marks as overlapping waits; the project's own cap holds even cleared ones.
        let c = hub.store.create_task(new_task("c", vec![])).await.unwrap();
        crate::parallel::save_scopes(&hub, None, vec![crate::parallel::SubmittedScope { id: c.id, paths: vec!["src/a/x.rs".into()], independent: true, reason: String::new() }], "agent").await.unwrap();
        orch.tick().await.unwrap();
        assert_eq!(hub.store.task(c.id).await.unwrap().unwrap().status, TaskStatus::Pending);
        assert!(decisions_of(&hub, c.id).await.contains(&"ready.shared_busy".to_string()));
    }

    // ---- orchestrator decisions

    /// The task's decision kinds, oldest first, without the `decision.` prefix.
    async fn decisions_of(hub: &Hub, id: i64) -> Vec<String> {
        let mut kinds: Vec<String> = hub.store.decision_events(id, 500).await.unwrap().into_iter().map(|e| e.kind.trim_start_matches("decision.").to_string()).collect();
        kinds.reverse();
        kinds
    }

    #[tokio::test]
    async fn a_passing_task_records_every_decision_on_the_happy_path() {
        let (hub, _term, _dir) = setup("verify = [\"true\", \"echo checked\"]").await;
        let t = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        assert_eq!(decisions_of(&hub, t.id).await, ["ready.go", "pick.chosen", "launch.started"]);
        let session = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
        std::fs::write(hub.result_file(&session), r#"{"status":"done","summary":"did a"}"#).unwrap();
        settle(&mut orch).await;
        assert_eq!(hub.store.task(t.id).await.unwrap().unwrap().status, TaskStatus::Done);
        assert_eq!(
            decisions_of(&hub, t.id).await,
            [
                "ready.go",
                "pick.chosen",
                "launch.started",
                "watch.done",
                "verify.cmd_start",
                "verify.cmd_pass",
                "verify.cmd_start",
                "verify.cmd_pass",
                "verify.pass",
                "audit.off",
                "land.shared",
                "post.pass",
                "end.closed"
            ]
        );
        let trace = hub.store.decision_events(t.id, 500).await.unwrap();
        let find = |kind: &str| trace.iter().find(|e| e.kind == kind).unwrap().message.clone();
        assert_eq!(find("decision.pick.chosen"), "order claude → claude");
        assert_eq!(find("decision.ready.go"), "no dependencies · slot 1/1");
        assert!(trace.iter().any(|e| e.kind == "decision.verify.cmd_pass" && e.message.starts_with("`echo checked` exit 0 · 2 of 2")), "{trace:?}");
        // Every recorded kind is in the model, so the dashboard can place it on an edge.
        assert!(trace.iter().all(|e| crate::decisions::ALL.iter().any(|k| k.kind == e.kind)));
        let counts = crate::decisions::counts(&hub, Utc::now() - chrono::Duration::hours(1), Some("")).await.unwrap();
        let n = |from, to| counts.edges.iter().find(|e| e.from == from && e.to == to).map_or(0, |e| e.count);
        use crate::model::DecisionNode as N;
        assert_eq!((n(N::Ready, N::Pick), n(N::Verify, N::Audit), n(N::End, N::Done), n(N::Verify, N::Verify)), (1, 1, 1, 0));
        assert!(crate::decisions::counts(&hub, Utc::now() - chrono::Duration::hours(1), Some("other")).await.unwrap().edges.is_empty());
    }

    #[tokio::test]
    async fn a_failing_verify_records_retry_then_failure() {
        let (hub, _term, _dir) = setup("verify = [\"echo broken-test; false\"]\nmax_attempts = 2").await;
        let t = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        for _ in 0..2 {
            orch.tick().await.unwrap();
            let s = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
            std::fs::write(hub.result_file(&s), r#"{"status":"done","summary":"x"}"#).unwrap();
            settle(&mut orch).await;
        }
        assert_eq!(hub.store.task(t.id).await.unwrap().unwrap().status, TaskStatus::Failed);
        let attempt = ["ready.go", "pick.chosen", "launch.started", "watch.done", "verify.cmd_start", "verify.fail"];
        let mut expected: Vec<&str> = attempt.to_vec();
        expected.push("retry.again");
        expected.extend(attempt);
        expected.push("retry.failed");
        assert_eq!(decisions_of(&hub, t.id).await, expected);
        let trace = hub.store.decision_events(t.id, 500).await.unwrap();
        assert_eq!(trace[0].message, "attempt 2 of 2 used");
        assert!(trace[1].message.starts_with("`echo broken-test; false` failed · 1 of 1 · broken-test"), "{}", trace[1].message);
    }

    #[tokio::test]
    async fn a_task_parked_at_a_branch_records_it_once() {
        let (hub, _term, _dir) = setup("").await;
        let t = hub.store.create_task(NewTask { provider: Some("nobody".into()), ..new_task("a", vec![]) }).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        for _ in 0..3 {
            orch.tick().await.unwrap();
        }
        assert_eq!(decisions_of(&hub, t.id).await, ["ready.go", "pick.none"]);
        assert_eq!(hub.store.decision_events(t.id, 1).await.unwrap()[0].message, "pinned: nobody · nobody not found on PATH");
    }

    #[tokio::test]
    async fn a_step_runs_one_tick_of_the_paused_loop() {
        let (hub, _term, _dir) = setup("").await;
        assert!(!hub.orchestrator_running());
        let t = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let looping = tokio::spawn(run(hub.clone()));
        assert!(hub.step.step(Duration::from_secs(10)).await);
        assert_eq!(hub.store.task(t.id).await.unwrap().unwrap().status, TaskStatus::Running);
        looping.abort();
    }

    // ---- Reflex gates

    use crate::gates::fake::{self, FakeReflex};
    use crate::model::{ProjectProvider, ProjectSettings, ProjectSource, ReflexGates, SourceKind};
    use serde_json::{Value, json};

    /// Audit on with an auditor that always passes (`echo`); `claude` and `codex` count as installed.
    async fn gate_setup(gates: ReflexGates, providers: Vec<ProjectProvider>) -> (Arc<Hub>, tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[orchestrator]\nprovider_order = [\"claude\"]\n[orchestrator.audit]\nenabled = true\nchain = [\"claude\"]\n\
             [reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"/bin/sh\"\nheadless = [\"/bin/echo\", \"VERDICT: PASS\"]\n[providers.codex]\nbinary = \"/bin/sh\"\n",
            dir.path().display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        let hub = Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default()));
        let source = ProjectSource { kind: SourceKind::Empty, location: Some(dir.path().join("app").display().to_string()), branch: None, remote: None };
        let new = crate::projects::NewProject { name: "App".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let settings = ProjectSettings { source, providers, reflex_gates: gates, ..Default::default() };
        let (p, _) = crate::projects::create_with(&hub, new, settings).await.unwrap();
        (hub, dir, p.slug)
    }

    async fn reflex_answering(hub: &Hub, slug: &str, answers: Vec<Value>) -> FakeReflex {
        let reflex = fake::start(answers).await;
        let _ = slug;
        fake::attach_gates(hub, reflex.port, Some(0.5));
        reflex
    }

    fn no_audit(confidence: f64) -> Value {
        fake::answer("needs_audit", json!({"choice": {"index": 1}}), confidence)
    }

    /// Runs one task of the project to its end (the worker reports done) and returns its decision kinds.
    async fn run_task(hub: &Arc<Hub>, task: NewTask) -> (Task, Vec<String>) {
        let t = hub.store.create_task(task).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        let session = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
        std::fs::write(hub.result_file(&session), r#"{"status":"done","summary":"fixed the typo in the README"}"#).unwrap();
        settle(&mut orch).await;
        (hub.store.task(t.id).await.unwrap().unwrap(), decisions_of(hub, t.id).await)
    }

    fn project_task(slug: &str) -> NewTask {
        NewTask { project: Some(slug.into()), ..new_task("Fix README typo", vec![]) }
    }

    fn after_verify(kinds: &[String]) -> Vec<&str> {
        let i = kinds.iter().position(|k| k == "verify.pass").unwrap();
        kinds[i + 1..].iter().map(String::as_str).collect()
    }

    #[tokio::test]
    async fn gates_off_leave_the_audit_and_model_alone() {
        let (hub, _dir, slug) = gate_setup(ReflexGates::default(), vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![no_audit(0.9), fake::answer("intake", json!({"choice": {"index": 2}}), 0.9)]).await;
        let (t, kinds) = run_task(&hub, project_task(&slug)).await;
        assert_eq!(t.status, TaskStatus::Done, "{:?}", t.result);
        assert_eq!(&kinds[..3], ["ready.go", "pick.chosen", "launch.started"]);
        assert_eq!(after_verify(&kinds), ["audit.ask", "audit.pass", "land.shared", "post.pass", "end.closed"]);
        assert_eq!(hub.store.session(t.session.as_deref().unwrap()).await.unwrap().unwrap().model, "sonnet");
        assert!(reflex.bodies.lock().unwrap().is_empty(), "Reflex is never asked");
        assert!(hub.store.decisions(&slug, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_confident_no_skips_the_audit() {
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: true, tier: false, ..Default::default() }, vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![no_audit(0.9)]).await;
        let (t, kinds) = run_task(&hub, project_task(&slug)).await;
        assert_eq!(t.status, TaskStatus::Done, "{:?}", t.result);
        assert_eq!(after_verify(&kinds), ["audit.skipped_reflex", "land.shared", "post.pass", "end.closed"]);
        let trace = hub.store.decision_events(t.id, 50).await.unwrap();
        let skipped = trace.iter().find(|e| e.kind == "decision.audit.skipped_reflex").unwrap();
        assert!(skipped.message.starts_with("needs_audit: no · confidence 9.0e-1 ≥ 5.0e-1 · decision #"), "{}", skipped.message);
        assert!(trace.iter().all(|e| crate::decisions::ALL.iter().any(|k| k.kind == e.kind)));

        // Reflex read the task, the worker's summary and the change; the question is in the project's decision log.
        let bodies = reflex.bodies.lock().unwrap().clone();
        let state = bodies[0]["state"].as_str().unwrap();
        assert!(state.contains("Fix README typo") && state.contains("fixed the typo in the README") && state.contains("Not a git repository"), "{state}");
        assert_eq!(bodies[0]["questions"][0]["id"], "needs_audit");
        let rows = hub.store.decisions(&slug, 10).await.unwrap();
        assert_eq!((rows.len(), rows[0].question.id.as_str(), rows[0].lane), (1, "needs_audit", crate::model::Lane::Reflex));
        assert!(rows[0].state.contains("Worker summary"));
        let counts = crate::decisions::counts(&hub, Utc::now() - chrono::Duration::hours(1), None).await.unwrap();
        use crate::model::DecisionNode as N;
        assert!(counts.edges.iter().any(|e| e.from == N::Audit && e.to == N::Land && e.count == 1));
    }

    #[tokio::test]
    async fn the_audit_runs_on_yes_abstain_low_confidence_or_reflex_down() {
        let cases: [(&str, Option<Vec<Value>>); 5] = [
            ("audit.reflex_said_audit", Some(vec![fake::answer("needs_audit", json!({"choice": {"index": 0}}), 0.9)])),
            ("audit.reflex_abstained", Some(vec![fake::answer("needs_audit", Value::Null, 0.0)])),
            ("audit.reflex_abstained", Some(vec![no_audit(0.1)])),
            // Reported running, but nothing answers on its port.
            ("audit.reflex_abstained", None),
            // Not running at all (no Reflex binary).
            ("audit.reflex_abstained", Some(vec![])),
        ];
        for (i, (kind, answers)) in cases.into_iter().enumerate() {
            let (hub, _dir, slug) = gate_setup(ReflexGates { audit: true, tier: false, ..Default::default() }, vec![]).await;
            match answers {
                Some(a) if a.is_empty() => {}
                Some(a) => {
                    reflex_answering(&hub, &slug, a).await;
                }
                None => fake::attach_gates(&hub, fake::closed_port(), Some(0.5)),
            }
            let (t, kinds) = run_task(&hub, project_task(&slug)).await;
            assert_eq!(t.status, TaskStatus::Done, "case {i}: {:?}", t.result);
            assert_eq!(after_verify(&kinds), [kind, "audit.ask", "audit.pass", "land.shared", "post.pass", "end.closed"], "case {i}");
            let message = hub.store.decision_events(t.id, 50).await.unwrap().into_iter().find(|e| e.kind == format!("decision.{kind}")).unwrap().message;
            let expect = ["needs_audit: yes · confidence 9.0e-1", "needs_audit: Reflex abstained", "below the threshold", "Reflex is not reachable", "the gates Reflex is stopped, not running"][i];
            assert!(message.contains(expect), "case {i}: {message}");
        }
    }

    #[tokio::test]
    async fn the_tier_gate_picks_the_light_model() {
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: false, tier: true, ..Default::default() }, vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![fake::answer("intake", json!({"choice": {"index": 2}}), 0.9)]).await;
        let t = hub.store.create_task(project_task(&slug)).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let t = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(hub.store.session(t.session.as_deref().unwrap()).await.unwrap().unwrap().model, "haiku");
        assert_eq!(decisions_of(&hub, t.id).await, ["ready.go", "pick.tier_light", "pick.chosen", "launch.started"]);
        let tier = hub.store.decision_events(t.id, 50).await.unwrap().into_iter().find(|e| e.kind == "decision.pick.tier_light").unwrap();
        assert!(tier.message.starts_with("tier: light · confidence 9.0e-1 ≥ 5.0e-1 · decision #") && tier.message.ends_with("claude → haiku"), "{}", tier.message);
        let bodies = reflex.bodies.lock().unwrap().clone();
        assert_eq!(bodies[0]["questions"][0]["options"], json!(["unclear", "plan_first", "light", "heavy"]));
        assert!(bodies[0]["state"].as_str().unwrap().starts_with("Task: Fix README typo"));
        assert_eq!(hub.store.decisions(&slug, 10).await.unwrap()[0].question.id, "intake");
    }

    #[tokio::test]
    async fn the_tier_gate_keeps_a_pinned_model_and_skips_providers_without_tiers() {
        // Pinned model: Reflex is not asked.
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: false, tier: true, ..Default::default() }, vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![fake::answer("intake", json!({"choice": {"index": 3}}), 0.9)]).await;
        let t = hub.store.create_task(NewTask { model: Some("fable".into()), ..project_task(&slug) }).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let t = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(hub.store.session(t.session.as_deref().unwrap()).await.unwrap().unwrap().model, "fable");
        assert_eq!(decisions_of(&hub, t.id).await, ["ready.go", "pick.chosen", "launch.started"]);
        assert!(reflex.bodies.lock().unwrap().is_empty());

        // codex has no [orchestrator.tiers.codex]: the gate does nothing, the project's model stays.
        let codex = vec![ProjectProvider { name: "codex".into(), model: Some("gpt-6-luna".into()), max_parallel: None }];
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: false, tier: true, ..Default::default() }, codex).await;
        let reflex = reflex_answering(&hub, &slug, vec![fake::answer("intake", json!({"choice": {"index": 3}}), 0.9)]).await;
        let t = hub.store.create_task(project_task(&slug)).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let t = hub.store.task(t.id).await.unwrap().unwrap();
        let session = hub.store.session(t.session.as_deref().unwrap()).await.unwrap().unwrap();
        assert_eq!((session.provider.as_str(), session.model.as_str()), ("codex", "gpt-6-luna"));
        assert_eq!(decisions_of(&hub, t.id).await, ["ready.go", "pick.chosen", "launch.started"]);
        assert!(reflex.bodies.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unsure_tier_keeps_the_usual_model() {
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: false, tier: true, ..Default::default() }, vec![]).await;
        reflex_answering(&hub, &slug, vec![fake::answer("intake", json!({"choice": {"index": 2}}), 0.1)]).await;
        let t = hub.store.create_task(project_task(&slug)).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let t = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(hub.store.session(t.session.as_deref().unwrap()).await.unwrap().unwrap().model, "sonnet");
        assert_eq!(decisions_of(&hub, t.id).await, ["ready.go", "pick.tier_abstained", "pick.chosen", "launch.started"]);
        let message = hub.store.decision_events(t.id, 50).await.unwrap().into_iter().find(|e| e.kind == "decision.pick.tier_abstained").unwrap().message;
        assert!(message.starts_with("tier: Confidence 1.0e-1 is below the threshold 5.0e-1") && message.ends_with("usual model"), "{message}");
    }

    #[tokio::test]
    async fn failed_dependency_blocks_dependents() {
        let (hub, _term, _dir) = setup("").await;
        let a = hub.store.create_task(new_task("a", vec![])).await.unwrap();
        let b = hub.store.create_task(new_task("b", vec![a.id])).await.unwrap();
        Orchestrator::cancel(&hub, a.id).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let b = hub.store.task(b.id).await.unwrap().unwrap();
        assert_eq!(b.status, TaskStatus::Pending);
        assert!(b.result.unwrap().starts_with("Blocked"));
        assert_eq!(decisions_of(&hub, b.id).await, ["ready.blocked"]);
    }

    // ---- the intake gate

    fn intake_says(index: u64, confidence: f64) -> Value {
        fake::answer("intake", json!({"choice": {"index": index}}), confidence)
    }

    #[tokio::test]
    async fn an_unclear_thai_task_waits_for_the_owner_until_started_anyway() {
        let (hub, _dir, slug) = gate_setup(ReflexGates { intake: true, ..Default::default() }, vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![intake_says(0, 0.9)]).await;
        let t = hub.store.create_task(NewTask { project: Some(slug.clone()), ..new_task("แก้หน้าเว็บ", vec![]) }).await.unwrap();
        let mut t2 = hub.store.task(t.id).await.unwrap().unwrap();
        t2.prompt = "ช่วยแก้หน้าเว็บให้ดีขึ้นหน่อยครับ".into();
        hub.put_task(&mut t2).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        let held = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!((held.status, held.session.as_deref()), (TaskStatus::Pending, None));
        let intake = held.intake.clone().unwrap();
        assert_eq!(intake.verdict, crate::model::IntakeVerdict::Unclear);
        assert!(intake.holds() && intake.reason.starts_with("intake: unclear · confidence 9.0e-1"), "{}", intake.reason);
        assert_eq!(decisions_of(&hub, t.id).await, ["intake.unclear"]);
        // Reflex read the Thai task as English words, in one call; the gate never asks again.
        let bodies = reflex.bodies.lock().unwrap().clone();
        assert_eq!(bodies.len(), 1);
        let state = bodies[0]["state"].as_str().unwrap();
        assert!(!crate::thai::has_thai(state) && state.contains("fix web page") && state.contains("improve"), "{state}");
        assert_eq!(bodies[0]["questions"][0]["id"], "intake");
        assert!(bodies[0]["questions"].as_array().unwrap().iter().any(|q| q["id"] == "shared"), "shared folder with the analyst on");
        orch.tick().await.unwrap();
        assert_eq!(reflex.bodies.lock().unwrap().len(), 1);
        assert!(hub.store.task(t.id).await.unwrap().unwrap().session.is_none());

        crate::intake::start_anyway(&hub, t.id).await.unwrap();
        orch.tick().await.unwrap();
        let started = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(started.status, TaskStatus::Running);
        assert_eq!(decisions_of(&hub, t.id).await, ["intake.unclear", "intake.released", "ready.go", "pick.chosen", "launch.started"]);
        assert!(crate::intake::start_anyway(&hub, t.id).await.is_err(), "no longer held");
    }

    #[tokio::test]
    async fn clarifying_a_task_asks_the_intake_again() {
        let (hub, _dir, slug) = gate_setup(ReflexGates { intake: true, ..Default::default() }, vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![intake_says(0, 0.9)]).await;
        let t = hub.store.create_task(project_task(&slug)).await.unwrap();
        let mut orch = Orchestrator::new(hub.clone());
        orch.tick().await.unwrap();
        assert!(hub.store.task(t.id).await.unwrap().unwrap().intake.unwrap().holds());
        let c = crate::intake::clarify(&hub, t.id, "the typo is in the install section").await.unwrap();
        assert!(c.prompt.ends_with("The owner adds:\nthe typo is in the install section") && c.intake.is_none());
        orch.tick().await.unwrap();
        assert_eq!(reflex.bodies.lock().unwrap().len(), 2);
        assert!(crate::intake::clarify(&hub, t.id, "  ").await.is_err());
    }

    #[tokio::test]
    async fn intake_answers_the_tier_and_shared_in_the_same_call() {
        let (hub, _dir, slug) = gate_setup(ReflexGates { intake: true, tier: true, ..Default::default() }, vec![]).await;
        let reflex = reflex_answering(
            &hub,
            &slug,
            vec![intake_says(2, 0.9), fake::answer("shared", json!({"choice": {"index": 0}}), 0.9)],
        )
        .await;
        let t = hub.store.create_task(project_task(&slug)).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let t = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Running);
        assert_eq!(hub.store.session(t.session.as_deref().unwrap()).await.unwrap().unwrap().model, "haiku");
        assert_eq!(reflex.bodies.lock().unwrap().len(), 1, "one Reflex call for intake (with the tier) and shared");
        let scope = t.scope.clone().unwrap();
        assert_eq!((scope.by.as_str(), scope.independent, scope.paths.clone()), ("reflex", false, vec!["*".to_string()]));
        let intake = t.intake.unwrap();
        assert!(intake.shared_files && intake.tier_asked && intake.tier.as_deref() == Some("light"));
        assert_eq!(decisions_of(&hub, t.id).await, ["intake.shared_files", "intake.ready", "ready.go", "pick.tier_light", "pick.chosen", "launch.started"]);
        let tier = hub.store.decision_events(t.id, 50).await.unwrap().into_iter().find(|e| e.kind == "decision.pick.tier_light").unwrap();
        assert_eq!(tier.message, "tier: light · asked at intake · claude → haiku");
    }

    #[tokio::test]
    async fn an_unsure_intake_lets_the_task_run_and_a_repeat_of_a_done_task_waits() {
        let (hub, _dir, slug) = gate_setup(ReflexGates { intake: true, ..Default::default() }, vec![]).await;
        // Confidence below the threshold (0.5): unsure, so on as before.
        let reflex = reflex_answering(&hub, &slug, vec![intake_says(0, 0.2)]).await;
        let t = hub.store.create_task(project_task(&slug)).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let mut t = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Running);
        assert_eq!(t.intake.as_ref().unwrap().verdict, crate::model::IntakeVerdict::Unsure);
        assert_eq!(decisions_of(&hub, t.id).await[0], "intake.unsure");

        t.status = TaskStatus::Done;
        hub.put_task(&mut t).await.unwrap();
        let again = hub.store.create_task(project_task(&slug)).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let again = hub.store.task(again.id).await.unwrap().unwrap();
        let intake = again.intake.unwrap();
        assert_eq!((intake.verdict, intake.similar), (crate::model::IntakeVerdict::Duplicate, Some(t.id)));
        assert!(again.session.is_none());
        assert_eq!(reflex.bodies.lock().unwrap().len(), 1, "a duplicate is found without Reflex");
    }

    #[tokio::test]
    async fn intake_off_never_asks() {
        let (hub, _dir, slug) = gate_setup(ReflexGates::default(), vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![intake_says(0, 0.9)]).await;
        let t = hub.store.create_task(project_task(&slug)).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let t = hub.store.task(t.id).await.unwrap().unwrap();
        assert!(t.intake.is_none() && t.status == TaskStatus::Running);
        assert!(reflex.bodies.lock().unwrap().is_empty());
    }
}
