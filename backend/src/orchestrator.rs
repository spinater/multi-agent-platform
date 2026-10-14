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
use crate::model::{Level, Role, SessionState, Task, TaskStatus};

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

/// Runs `sh -c command` in `dir` with the workspace env; returns (success, combined output tail).
pub async fn shell(hub: &Hub, dir: &std::path::Path, command: &str, timeout: Duration) -> Result<(bool, String)> {
    let mut cmd = Command::new("sh");
    cmd.arg("-c").arg(command).current_dir(dir).envs(&hub.config.workspace.env);
    run_captured(cmd, timeout).await
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

fn looks_like_quota(output: &str) -> bool {
    let lower = output.to_lowercase();
    crate::detect::rate_limit_line(output).is_some() || lower.contains("credit balance is too low") || lower.contains("quota")
}

impl Orchestrator {
    pub fn new(hub: Arc<Hub>) -> Self {
        Self { hub, finishing: Arc::new(Mutex::new(HashSet::new())) }
    }

    pub async fn tick(&mut self) -> Result<()> {
        let tasks = self.hub.store.tasks().await?;
        for task in tasks.iter().filter(|t| t.status == TaskStatus::Running) {
            if let Err(err) = self.check_running(task.clone()).await {
                tracing::warn!(task = task.id, "check failed: {err:#}");
            }
        }
        // Verify/audit steps lost to a restart are picked up again.
        for task in tasks.iter().filter(|t| matches!(t.status, TaskStatus::Verifying | TaskStatus::Auditing)) {
            self.spawn_finish(task.clone());
        }

        let tasks = self.hub.store.tasks().await?;
        let mut active = tasks.iter().filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Verifying | TaskStatus::Auditing)).count();
        let paused = crate::project_settings::paused(&self.hub).await;
        for task in tasks.iter().filter(|t| t.status == TaskStatus::Pending && !crate::project_settings::is_paused(&paused, t)) {
            if active >= self.hub.config.orchestrator.max_parallel {
                break;
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
            record(&self.hub, Kind::READY_GO, task, dec::ready_detail(task, active, self.hub.config.orchestrator.max_parallel)).await;
            if self.start(task.clone()).await? {
                active += 1;
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
        if let Some(slug) = &task.project {
            p.push_str(&format!(
                "- This is project `{slug}`. Before using an LLM to classify, route or decide something, POST it to $MAP_DECIDE_URL \
                 (with `-H \"Authorization: Bearer $MAP_PROJECT_TOKEN\"`): \
                 {{\"state\": \"<the facts, in English: Reflex reads English only>\", \"questions\": [{{\"id\", \"kind\": \"choice\"|\"score\"|\"noul\", \"prompt\", \"options\"}}]}}. \
                 Each answer has a `lane`; `abstain` means nobody decided, so handle it yourself.\n"
            ));
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
        let said = crate::gates::ask(&self.hub, slug, &crate::gates::task_state(task), crate::gates::tier_question()).await;
        let tier = match &said {
            crate::gates::Said::Answer { answer, .. } => answer.as_str(),
            crate::gates::Said::Unsure(_) => None,
        };
        let model = tier.and_then(|t| tiers.get(t)).map(String::from);
        let detail = match (tier, &model) {
            (Some(t), Some(m)) => format!("tier: {t} · {} · {provider} → {m}", said.detail()),
            (Some(t), None) => format!("tier: {t} · {} · no {t} model for {provider}, usual model", said.detail()),
            _ => format!("tier: {} · usual model", said.detail()),
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
        let said = match crate::gates::not_running(&self.hub, slug) {
            Some(said) => said,
            None => {
                let state = crate::gates::audit_state(&self.hub, task, dir).await;
                crate::gates::ask(&self.hub, slug, &state, crate::gates::needs_audit_question()).await
            }
        };
        match &said {
            crate::gates::Said::Answer { answer, .. } if answer == &serde_json::Value::Bool(false) => {
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
        let session = format!("{}task-{}-{}", self.hub.config.workspace.session_prefix, task.id, task.attempts);
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
        self.hub.put_task(&mut task).await
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

    async fn finish(&self, mut task: Task) -> Result<()> {
        let hub = &self.hub;
        let cfg = &hub.config.orchestrator;
        let dir = hub.work_dir(task.project.as_deref()).await;

        if task.status == TaskStatus::Verifying {
            let verify = crate::project_settings::verify(hub, &task).await;
            for (i, command) in verify.iter().enumerate() {
                let of = format!("{} of {}", i + 1, verify.len());
                record(hub, Kind::VERIFY_CMD_START, &task, format!("`{command}` · {of}")).await;
                let started = std::time::Instant::now();
                let (ok, output) = shell(hub, &dir, command, Duration::from_secs(30 * 60)).await?;
                if !ok {
                    record(hub, Kind::VERIFY_FAIL, &task, format!("`{command}` failed · {of} · {}", dec::last_line(&output))).await;
                    return self.retry_or_fail(task, &format!("`{command}` failed:\n{output}")).await;
                }
                record(hub, Kind::VERIFY_CMD_PASS, &task, format!("`{command}` exit 0 · {of} · {}", dec::secs(started.elapsed()))).await;
            }
            record(hub, Kind::VERIFY_PASS, &task, if verify.is_empty() { "no verify commands".to_string() } else { format!("all {} exit 0", verify.len()) }).await;
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

        for template in &cfg.post_done {
            let command = template.replace("{id}", &task.id.to_string()).replace("{title}", &quote(&task.title)?);
            let (ok, output) = shell(hub, &dir, &command, Duration::from_secs(10 * 60)).await?;
            if !ok {
                record(hub, Kind::POST_FAIL, &task, format!("`{command}` failed · {}", dec::last_line(&output))).await;
                // The agent's work passed; a failing hook needs a person, not another agent run.
                task.status = TaskStatus::Failed;
                task.result = Some(format!("post_done `{command}` failed:\n{output}"));
                hub.put_task(&mut task).await?;
                hub.event(Level::Error, "task.post_done_failed", task.session.as_deref(), Some(task.id), format!("`{command}` failed")).await;
                return Ok(());
            }
            record(hub, Kind::POST_CMD_PASS, &task, format!("`{command}` exit 0")).await;
        }
        record(hub, Kind::POST_PASS, &task, if cfg.post_done.is_empty() { "no post_done hooks".to_string() } else { format!("all {} hooks exit 0", cfg.post_done.len()) }).await;

        if let Some(name) = task.session.clone()
            && hub.store.session(&name).await?.is_some_and(|s| s.state.is_live())
            && hub.close_session(&name, false, "Task done").await? != EndDecision::End
        {
            hub.event(Level::Info, "task.kept", Some(&name), Some(task.id), "Task done; session left open because a person is using it").await;
            record(hub, Kind::END_KEPT, &task, format!("a client is attached or the input line has text · {name} left open")).await;
        } else {
            record(hub, Kind::END_CLOSED, &task, "no client attached · input line empty · worker session ended").await;
        }
        task.status = TaskStatus::Done;
        hub.put_task(&mut task).await?;
        hub.event(Level::Success, "task.done", task.session.as_deref(), Some(task.id), format!("Task #{} done: {}", task.id, task.title)).await;
        Ok(())
    }

    async fn audit(&self, task: &Task) -> Result<Verdict> {
        let hub = &self.hub;
        let prompt = format!(
            "You are a strict code auditor. Task #{id} \"{title}\" was just completed in this repository and passed its verification commands.\n\
             Task description:\n{body}\n\nWorker summary:\n{summary}\n\n\
             Inspect the working tree and recent changes (git status, git diff, git log -5). Check that the change does what the task asks, \
             has tests, and leaves nothing broken or half-done.\nAnswer with exactly one final line:\nVERDICT: PASS\nor\nVERDICT: FAIL: <reason>",
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
            let mut cmd = Command::new(&argv[0]);
            cmd.args(&argv[1..]).current_dir(hub.work_dir(task.project.as_deref()).await).envs(&hub.config.workspace.env);
            hub.event(Level::Info, "task.audit", None, Some(task.id), format!("Auditing with {provider}")).await;
            record(hub, Kind::AUDIT_ASK, task, format!("{provider} · reviewing git status, git diff, git log -5")).await;
            let (ok, output) = run_captured(cmd, Duration::from_secs(hub.config.orchestrator.audit.timeout_secs)).await?;
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
            "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[orchestrator]\nprovider_order = [\"claude\"]\n{extra}\n[providers.claude]\nbinary = \"/bin/sh\"\n",
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
            "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[orchestrator]\nprovider_order = [\"claude\"]\n[orchestrator.audit]\nenabled = true\nchain = [\"claude\"]\n\
             [reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"/bin/sh\"\nheadless = [\"/bin/echo\", \"VERDICT: PASS\"]\n[providers.codex]\nbinary = \"/bin/sh\"\n",
            dir.path().display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        let hub = Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default()));
        let source = ProjectSource { kind: SourceKind::Empty, location: Some(dir.path().join("app").display().to_string()), branch: None };
        let new = crate::projects::NewProject { name: "App".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let settings = ProjectSettings { source, providers, reflex_gates: gates, ..Default::default() };
        let (p, _) = crate::projects::create_with(&hub, new, settings).await.unwrap();
        (hub, dir, p.slug)
    }

    async fn reflex_answering(hub: &Hub, slug: &str, answers: Vec<Value>) -> FakeReflex {
        let reflex = fake::start(answers).await;
        fake::attach(hub, slug, reflex.port, Some(0.5)).await;
        reflex
    }

    fn no_audit(confidence: f64) -> Value {
        fake::answer("needs_audit", json!({"noul": {"yes": false}}), confidence)
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
        let reflex = reflex_answering(&hub, &slug, vec![no_audit(0.9), fake::answer("tier", json!({"choice": {"index": 0}}), 0.9)]).await;
        let (t, kinds) = run_task(&hub, project_task(&slug)).await;
        assert_eq!(t.status, TaskStatus::Done, "{:?}", t.result);
        assert_eq!(&kinds[..3], ["ready.go", "pick.chosen", "launch.started"]);
        assert_eq!(after_verify(&kinds), ["audit.ask", "audit.pass", "post.pass", "end.closed"]);
        assert_eq!(hub.store.session(t.session.as_deref().unwrap()).await.unwrap().unwrap().model, "sonnet");
        assert!(reflex.bodies.lock().unwrap().is_empty(), "Reflex is never asked");
        assert!(hub.store.decisions(&slug, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_confident_no_skips_the_audit() {
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: true, tier: false }, vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![no_audit(0.9)]).await;
        let (t, kinds) = run_task(&hub, project_task(&slug)).await;
        assert_eq!(t.status, TaskStatus::Done, "{:?}", t.result);
        assert_eq!(after_verify(&kinds), ["audit.skipped_reflex", "post.pass", "end.closed"]);
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
        assert!(counts.edges.iter().any(|e| e.from == N::Audit && e.to == N::Post && e.count == 1));
    }

    #[tokio::test]
    async fn the_audit_runs_on_yes_abstain_low_confidence_or_reflex_down() {
        let cases: [(&str, Option<Vec<Value>>); 5] = [
            ("audit.reflex_said_audit", Some(vec![fake::answer("needs_audit", json!({"noul": {"yes": true}}), 0.9)])),
            ("audit.reflex_abstained", Some(vec![fake::answer("needs_audit", Value::Null, 0.0)])),
            ("audit.reflex_abstained", Some(vec![no_audit(0.1)])),
            // Reported running, but nothing answers on its port.
            ("audit.reflex_abstained", None),
            // Not running at all (no Reflex binary).
            ("audit.reflex_abstained", Some(vec![])),
        ];
        for (i, (kind, answers)) in cases.into_iter().enumerate() {
            let (hub, _dir, slug) = gate_setup(ReflexGates { audit: true, tier: false }, vec![]).await;
            match answers {
                Some(a) if a.is_empty() => {}
                Some(a) => {
                    reflex_answering(&hub, &slug, a).await;
                }
                None => fake::attach(&hub, &slug, fake::closed_port(), Some(0.5)).await,
            }
            let (t, kinds) = run_task(&hub, project_task(&slug)).await;
            assert_eq!(t.status, TaskStatus::Done, "case {i}: {:?}", t.result);
            assert_eq!(after_verify(&kinds), [kind, "audit.ask", "audit.pass", "post.pass", "end.closed"], "case {i}");
            let message = hub.store.decision_events(t.id, 50).await.unwrap().into_iter().find(|e| e.kind == format!("decision.{kind}")).unwrap().message;
            let expect = ["needs_audit: yes · confidence 9.0e-1", "needs_audit: Reflex abstained", "below the threshold", "Reflex is not reachable", "Reflex is error, not running"][i];
            assert!(message.contains(expect), "case {i}: {message}");
        }
    }

    #[tokio::test]
    async fn the_tier_gate_picks_the_light_model() {
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: false, tier: true }, vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![fake::answer("tier", json!({"choice": {"index": 0}}), 0.9)]).await;
        let t = hub.store.create_task(project_task(&slug)).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let t = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(hub.store.session(t.session.as_deref().unwrap()).await.unwrap().unwrap().model, "haiku");
        assert_eq!(decisions_of(&hub, t.id).await, ["ready.go", "pick.tier_light", "pick.chosen", "launch.started"]);
        let tier = hub.store.decision_events(t.id, 50).await.unwrap().into_iter().find(|e| e.kind == "decision.pick.tier_light").unwrap();
        assert!(tier.message.starts_with("tier: light · confidence 9.0e-1 ≥ 5.0e-1 · decision #") && tier.message.ends_with("claude → haiku"), "{}", tier.message);
        let bodies = reflex.bodies.lock().unwrap().clone();
        assert_eq!(bodies[0]["questions"][0]["options"], json!(["light", "standard", "heavy"]));
        assert!(bodies[0]["state"].as_str().unwrap().starts_with("Task: Fix README typo"));
        assert_eq!(hub.store.decisions(&slug, 10).await.unwrap()[0].question.id, "tier");
    }

    #[tokio::test]
    async fn the_tier_gate_keeps_a_pinned_model_and_skips_providers_without_tiers() {
        // Pinned model: Reflex is not asked.
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: false, tier: true }, vec![]).await;
        let reflex = reflex_answering(&hub, &slug, vec![fake::answer("tier", json!({"choice": {"index": 2}}), 0.9)]).await;
        let t = hub.store.create_task(NewTask { model: Some("fable".into()), ..project_task(&slug) }).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let t = hub.store.task(t.id).await.unwrap().unwrap();
        assert_eq!(hub.store.session(t.session.as_deref().unwrap()).await.unwrap().unwrap().model, "fable");
        assert_eq!(decisions_of(&hub, t.id).await, ["ready.go", "pick.chosen", "launch.started"]);
        assert!(reflex.bodies.lock().unwrap().is_empty());

        // codex has no [orchestrator.tiers.codex]: the gate does nothing, the project's model stays.
        let codex = vec![ProjectProvider { name: "codex".into(), model: Some("gpt-6-luna".into()), max_parallel: None }];
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: false, tier: true }, codex).await;
        let reflex = reflex_answering(&hub, &slug, vec![fake::answer("tier", json!({"choice": {"index": 2}}), 0.9)]).await;
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
        let (hub, _dir, slug) = gate_setup(ReflexGates { audit: false, tier: true }, vec![]).await;
        reflex_answering(&hub, &slug, vec![fake::answer("tier", json!({"choice": {"index": 0}}), 0.1)]).await;
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
}
