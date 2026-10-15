//! A task that runs a workflow (Configure › Workflows, `workflows.rs`) instead of the fixed pipeline.
//!
//! - **Choice.** When a task first becomes ready, its pinned workflow, else the first whose trigger matches (a
//!   keyword in its title or prompt, else a `default` one), is recorded on the task (`decision.workflow.chosen`).
//!   No match: the fixed pipeline, untouched.
//! - **Agent** nodes start a worker session as any task does (`start`: worktree, folder trust, launch), with the
//!   agent's provider and model when it names an available one (else the usual pick), and its instructions and the
//!   node's prompt before the task's. When the worker reports done its session is closed and the `next` edge taken.
//!   A worker that fails, exits or times out takes the node's `fail` edge, else is launched again with the failure
//!   (up to `orchestrator.max_attempts` retries in the run).
//! - **Verify** runs its command in the task's folder: exit 0 takes `pass`; a failure takes `fail` (usually back to an
//!   agent, which gets the output) while the node has run fewer than `max_attempts` times, else the failed end.
//! - **Audit** asks the named auditor (an agent's provider run headless, with its instructions) for the node's
//!   verdict text: `pass` or `fail` as verify (no auditor: `audit.required` decides).
//! - **Hook** runs its command with its timeout; a blocking failure takes `fail` (or fails the task), a non-blocking
//!   one is logged and the task goes on.
//! - **End**: done goes on to landing and `post_done` as every task; failed fails the task.
//!
//! The current node, its visits and the retries are saved on the task (`workflow_state`) at every step, so a restart
//! resumes at the node it was at (a verify, audit or hook there runs again).

use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use tokio::process::Command;

use super::{Orchestrator, looks_like_quota, run_captured, shell};
use crate::decisions::{self as dec, Kind, record};
use crate::model::{AgentProvider, Level, Task, TaskStatus, TaskWorkflowState, Workflow, WorkflowNode, WorkflowNodeKind as NK, WorkflowOutcome};
use crate::workflows;

/// What `start` needs to launch a workflow's agent node.
pub(super) struct AgentStep {
    pub node: String,
    pub agent: String,
    /// The agent's provider when it names one that is installed and not cooling down; `None`: the usual pick.
    pub provider: Option<String>,
    pub model: Option<String>,
    /// The agent's instructions and the node's prompt, then the task's.
    pub prompt: String,
    pub failure: Option<String>,
    pub verify: Vec<String>,
}

enum Audit {
    Pass(String),
    Fail(String),
    Unavailable(String),
}

fn provider_name(p: AgentProvider) -> &'static str {
    match p {
        AgentProvider::Claude => "claude",
        AgentProvider::Codex => "codex",
        AgentProvider::Gemini => "gemini",
    }
}

impl Orchestrator {
    async fn workflow_of(&self, task: &Task) -> Option<Workflow> {
        let id = task.workflow.as_deref()?;
        match workflows::find(&self.hub, task.project.as_deref(), id).await {
            Ok(w) => w,
            Err(err) => {
                tracing::warn!(task = task.id, "workflow {id}: {err:#}");
                None
            }
        }
    }

    /// Starts a ready task: through its workflow when it has one (or a trigger picks one now), else `start`.
    pub(super) async fn begin(&self, mut task: Task) -> Result<bool> {
        let hub = &self.hub;
        if task.workflow_state.is_none() {
            // Only a fresh task is matched against the triggers; a retry of the fixed pipeline stays on it.
            if task.workflow.is_none() && task.attempts > 0 {
                return self.start(task).await;
            }
            let list = match workflows::list(hub, task.project.as_deref()).await {
                Ok(list) => list,
                Err(err) => {
                    tracing::warn!(task = task.id, "workflows: {err:#}");
                    vec![]
                }
            };
            let (wf, why) = match task.workflow.clone() {
                Some(id) => match list.iter().find(|w| w.id == id) {
                    Some(w) => (w.clone(), "pinned".to_string()),
                    None => {
                        let why = format!("workflow `{id}` does not exist");
                        self.workflow_failed(task, Kind::WF_ERROR, format!("{id} · {why}"), why).await?;
                        return Ok(false);
                    }
                },
                None => match workflows::choose(&list, &task) {
                    Some(chosen) => chosen,
                    None => return self.start(task).await,
                },
            };
            let Some((trigger, first)) = workflows::trigger(&wf).and_then(|t| Some((t.id.clone(), workflows::success(&wf, &t.id)?.to_string()))) else {
                let why = format!("workflow `{}` has no trigger with a next step", wf.id);
                self.workflow_failed(task, Kind::WF_ERROR, format!("{} · {why}", wf.id), why).await?;
                return Ok(false);
            };
            task.workflow = Some(wf.id.clone());
            task.workflow_state =
                Some(TaskWorkflowState { node: first.clone(), chosen_by: why.clone(), steps: 1, visits: [(trigger.clone(), 1)].into(), ..Default::default() });
            hub.put_task(&mut task).await?;
            record(hub, Kind::WF_CHOSEN, &task, format!("{} · {why}", wf.id)).await;
            record(hub, Kind::WF_TRIGGER_NEXT, &task, format!("{trigger} · → {first}")).await;
        }
        let Some(wf) = self.workflow_of(&task).await else {
            let why = format!("workflow `{}` no longer exists", task.workflow.clone().unwrap_or_default());
            self.workflow_failed(task, Kind::WF_ERROR, why.clone(), why).await?;
            return Ok(false);
        };
        let at = task.workflow_state.as_ref().map(|s| s.node.clone()).unwrap_or_default();
        match workflows::node(&wf, &at).map(|n| n.kind) {
            Some(NK::Agent) => self.start(task).await,
            Some(_) => {
                // A verify, audit, hook or end comes first: the graph runs in the background from there.
                task.status = TaskStatus::Verifying;
                hub.put_task(&mut task).await?;
                self.spawn_finish(task);
                Ok(true)
            }
            None => {
                let why = format!("workflow `{}` has no node `{at}`", wf.id);
                self.workflow_failed(task, Kind::WF_ERROR, format!("{at} · {why}"), why).await?;
                Ok(false)
            }
        }
    }

    /// The agent node the task is at, resolved for `start`; `None` for a task without a workflow.
    pub(super) async fn agent_step(&self, task: &Task) -> Option<AgentStep> {
        let st = task.workflow_state.as_ref()?;
        let wf = self.workflow_of(task).await?;
        let node = workflows::node(&wf, &st.node).filter(|n| n.kind == NK::Agent)?;
        let agent = node.agent.clone().unwrap_or_default();
        let (provider, model, instructions) = self.resolve_agent(task.project.as_deref(), &agent, false).await;
        let mut prompt = String::new();
        if !instructions.trim().is_empty() {
            prompt.push_str(&format!("## Your role ({agent})\n{}\n\n", instructions.trim()));
        }
        if let Some(extra) = node.prompt.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            prompt.push_str(&format!("## This step ({})\n{extra}\n\n", if node.label.is_empty() { &node.id } else { &node.label }));
        }
        if !prompt.is_empty() {
            prompt.push_str("## The task\n");
        }
        prompt.push_str(task.prompt.trim());
        Some(AgentStep { node: node.id.clone(), agent, provider, model, prompt, failure: st.failure.clone(), verify: workflows::verify_commands(&wf) })
    }

    /// An agent (or provider) name → its provider (installed, not cooling down; with a headless command when
    /// `headless`), model and instructions. A name no provider of the agent can serve gives `None` and its
    /// instructions.
    async fn resolve_agent(&self, project: Option<&str>, name: &str, headless: bool) -> (Option<String>, Option<String>, String) {
        let hub = &self.hub;
        let usable = |p: &str| hub.spec(p).is_ok_and(|s| !headless || !s.headless.is_empty()) && hub.provider_available(p) && hub.cooldown_until(p).is_none();
        if hub.config.providers.contains_key(name) {
            return (usable(name).then(|| name.to_string()), None, String::new());
        }
        let Some(def) = workflows::agent_def(hub, project, name).await else { return (None, None, String::new()) };
        let provider = def.targets.iter().map(|p| provider_name(*p)).find(|p| usable(p));
        let model = provider.and_then(|p| match p {
            "claude" => def.models.claude.clone(),
            "codex" => def.models.codex.clone(),
            _ => def.models.gemini.clone(),
        });
        (provider.map(String::from), model.filter(|m| m != "inherit"), def.instructions)
    }

    /// Notes on the task that the agent node's session started.
    pub(super) fn agent_launched(&self, task: &mut Task, step: &AgentStep) {
        if let Some(st) = task.workflow_state.as_mut() {
            st.launched = true;
            st.last_agent = Some(step.node.clone());
            *st.visits.entry(step.node.clone()).or_default() += 1;
            st.steps += 1;
            st.failure = None;
        }
    }

    /// Fails a workflow task: the decision, its worker session ended (unless a person is using it), `task.failed`.
    async fn workflow_failed(&self, mut task: Task, kind: Kind, detail: String, reason: String) -> Result<()> {
        let hub = &self.hub;
        record(hub, kind, &task, detail).await;
        if let Some(name) = task.session.clone()
            && hub.store.session(&name).await?.is_some_and(|s| s.state.is_live())
        {
            hub.close_session(&name, false, "Task failed").await?;
        }
        let headline = reason.lines().next().unwrap_or_default().chars().take(300).collect::<String>();
        task.status = TaskStatus::Failed;
        task.result = Some(reason);
        hub.put_task(&mut task).await?;
        hub.event(Level::Error, "task.failed", task.session.as_deref(), Some(task.id), format!("Failed: {headline}")).await;
        Ok(())
    }

    /// Moves the task to `node` (saved), with the failure the next agent gets.
    async fn goto(&self, task: &mut Task, node: &str, failure: Option<String>) -> Result<()> {
        if let Some(st) = task.workflow_state.as_mut() {
            st.node = node.to_string();
            st.launched = false;
            if failure.is_some() {
                st.failure = failure;
            }
        }
        self.hub.put_task(task).await
    }

    /// Counts a run of `node` (saved with the status it runs under); returns its visit number.
    async fn visit(&self, task: &mut Task, node: &str, status: TaskStatus) -> Result<u32> {
        let st = task.workflow_state.get_or_insert_with(Default::default);
        let n = st.visits.entry(node.to_string()).or_default();
        *n += 1;
        let visit = *n;
        st.steps += 1;
        task.status = status;
        self.hub.put_task(task).await?;
        Ok(visit)
    }

    /// Walks the graph from the task's node until it must wait (an agent to launch: the task goes back to pending
    /// for the tick) or ends. `Ok(true)`: a done end, the task is `landing` and the caller lands it.
    pub(super) async fn run_workflow(&self, task: &mut Task) -> Result<bool> {
        let hub = &self.hub;
        let Some(wf) = self.workflow_of(task).await else {
            let why = format!("workflow `{}` no longer exists", task.workflow.clone().unwrap_or_default());
            self.workflow_failed(task.clone(), Kind::WF_ERROR, why.clone(), why).await?;
            return Ok(false);
        };
        let dir = hub.task_dir(task).await;
        loop {
            let st = task.workflow_state.clone().unwrap_or_default();
            if st.steps > workflows::MAX_STEPS {
                let why = format!("the workflow ran {} steps without ending; a loop in `{}`?", workflows::MAX_STEPS, wf.id);
                self.workflow_failed(task.clone(), Kind::WF_ERROR, format!("{} · {why}", st.node), why).await?;
                return Ok(false);
            }
            let Some(node) = workflows::node(&wf, &st.node).cloned() else {
                let why = format!("workflow `{}` has no node `{}`", wf.id, st.node);
                self.workflow_failed(task.clone(), Kind::WF_ERROR, format!("{} · {why}", st.node), why).await?;
                return Ok(false);
            };
            let id = node.id.clone();
            let Some(next) = workflows::success(&wf, &id).map(String::from).or_else(|| (node.kind == NK::End).then(String::new)) else {
                let why = format!("{} `{id}` has no next step", node.kind.as_str());
                self.workflow_failed(task.clone(), Kind::WF_ERROR, format!("{id} · {why}"), why).await?;
                return Ok(false);
            };
            match node.kind {
                NK::Trigger => {
                    record(hub, Kind::WF_TRIGGER_NEXT, task, format!("{id} · → {next}")).await;
                    self.goto(task, &next, None).await?;
                }
                NK::Agent if st.launched && task.status != TaskStatus::Pending => {
                    // The node's worker reported done (`check_running`): its session ends, the task goes on.
                    let visit = st.visits.get(&id).copied().unwrap_or(1);
                    let summary = dec::last_line(task.result.as_deref().unwrap_or_default());
                    record(hub, Kind::WF_AGENT_DONE, task, format!("{id} · reported done · visit {visit} · {summary} → {next}")).await;
                    self.end_step_session(task).await?;
                    self.goto(task, &next, None).await?;
                }
                NK::Agent => {
                    // To be launched: the tick starts it as it starts any ready task.
                    task.status = TaskStatus::Pending;
                    self.goto(task, &id, None).await?;
                    return Ok(false);
                }
                NK::Verify => {
                    let visit = self.visit(task, &id, TaskStatus::Verifying).await?;
                    let max = node.max_attempts.unwrap_or(1);
                    let command = node.command.clone().unwrap_or_default();
                    record(hub, Kind::WF_VERIFY_START, task, format!("{id} · `{command}` · visit {visit} of {max}")).await;
                    let started = std::time::Instant::now();
                    let (ok, output) = shell(hub, &dir, &command, Duration::from_secs(30 * 60)).await?;
                    if ok {
                        record(hub, Kind::WF_VERIFY_PASS, task, format!("{id} · `{command}` exit 0 · visit {visit} · {}", dec::secs(started.elapsed()))).await;
                        self.goto(task, &next, None).await?;
                    } else if !self.gate_failed(task, &wf, &node, visit, max, format!("`{command}` failed:\n{output}"), (Kind::WF_VERIFY_FAIL, Kind::WF_VERIFY_EXHAUSTED)).await? {
                        return Ok(false);
                    }
                }
                NK::Audit => {
                    let visit = self.visit(task, &id, TaskStatus::Auditing).await?;
                    let max = node.max_attempts.unwrap_or(hub.config.orchestrator.max_attempts.max(1) as u32);
                    let verdict = node.verdict.clone().unwrap_or_else(|| workflows::DEFAULT_VERDICT.into());
                    let auditor = node.agent.clone().unwrap_or_default();
                    record(hub, Kind::WF_AUDIT_START, task, format!("{id} · {auditor} · looking for `{verdict}` · visit {visit} of {max}")).await;
                    let failure = match self.workflow_audit(task, &node, &verdict).await? {
                        Audit::Pass(by) => {
                            record(hub, Kind::WF_AUDIT_PASS, task, format!("{id} · {by}: {verdict} · visit {visit}")).await;
                            hub.event(Level::Success, "task.audit_pass", task.session.as_deref(), Some(task.id), format!("Audit passed ({by})")).await;
                            None
                        }
                        Audit::Fail(reason) => Some(format!("Audit failed: {reason}")),
                        Audit::Unavailable(why) if hub.config.orchestrator.audit.required => Some(format!("No auditor was available and audit is required: {why}")),
                        Audit::Unavailable(why) => {
                            record(hub, Kind::WF_AUDIT_SKIPPED, task, format!("{id} · {why} · audit.required = false · visit {visit}")).await;
                            None
                        }
                    };
                    match failure {
                        None => self.goto(task, &next, None).await?,
                        Some(f) => {
                            if !self.gate_failed(task, &wf, &node, visit, max, f, (Kind::WF_AUDIT_FAIL, Kind::WF_AUDIT_EXHAUSTED)).await? {
                                return Ok(false);
                            }
                        }
                    }
                }
                NK::Hook => {
                    let visit = self.visit(task, &id, TaskStatus::Verifying).await?;
                    let command = node.command.clone().unwrap_or_default();
                    let timeout = node.timeout_secs.unwrap_or(workflows::HOOK_TIMEOUT_SECS);
                    let event = node.event.clone().unwrap_or_else(|| "hook".into());
                    record(hub, Kind::WF_HOOK_START, task, format!("{id} · {event} `{command}` · visit {visit}")).await;
                    let (ok, output) = shell(hub, &dir, &command, Duration::from_secs(timeout)).await?;
                    if ok {
                        record(hub, Kind::WF_HOOK_PASS, task, format!("{id} · `{command}` exit 0 · visit {visit}")).await;
                        self.goto(task, &next, None).await?;
                    } else if !node.blocking.unwrap_or(true) {
                        record(hub, Kind::WF_HOOK_IGNORED, task, format!("{id} · `{command}` failed · not blocking · {}", dec::last_line(&output))).await;
                        self.goto(task, &next, None).await?;
                    } else {
                        let failure = format!("hook `{command}` failed:\n{output}");
                        match workflows::fail(&wf, &id) {
                            Some(to) => {
                                record(hub, Kind::WF_HOOK_FAIL, task, format!("{id} · `{command}` failed · visit {visit} · {} → {to}", dec::last_line(&output))).await;
                                let to = to.to_string();
                                self.goto(task, &to, Some(failure)).await?;
                            }
                            None => {
                                let detail = format!("{id} · `{command}` failed · blocking, no fail edge · {}", dec::last_line(&output));
                                self.workflow_failed(task.clone(), Kind::WF_HOOK_FAIL, detail, failure).await?;
                                return Ok(false);
                            }
                        }
                    }
                }
                NK::End => {
                    let st = task.workflow_state.get_or_insert_with(Default::default);
                    *st.visits.entry(id.clone()).or_default() += 1;
                    st.steps += 1;
                    if node.outcome.unwrap_or_default() == WorkflowOutcome::Done {
                        record(hub, Kind::WF_END_DONE, task, format!("{id} · done → land, post_done")).await;
                        task.status = TaskStatus::Landing;
                        hub.put_task(task).await?;
                        return Ok(true);
                    }
                    let reason = st.failure.clone().unwrap_or_else(|| format!("The workflow reached its failed end `{id}`"));
                    self.workflow_failed(task.clone(), Kind::WF_END_FAILED, format!("{id} · failed · {}", dec::last_line(&reason)), reason).await?;
                    return Ok(false);
                }
            }
        }
    }

    /// A verify or audit that did not pass: its fail edge while the node has run fewer than `max` times, else the
    /// failed end (or the task fails). `Ok(false)`: the task ended here.
    #[allow(clippy::too_many_arguments)]
    async fn gate_failed(&self, task: &mut Task, wf: &Workflow, node: &WorkflowNode, visit: u32, max: u32, failure: String, kinds: (Kind, Kind)) -> Result<bool> {
        let hub = &self.hub;
        let id = &node.id;
        let line = dec::last_line(&failure);
        if let Some(to) = workflows::fail(wf, id).filter(|_| visit < max) {
            record(hub, kinds.0, task, format!("{id} · {line} · visit {visit} of {max} → {to}")).await;
            let to = to.to_string();
            self.goto(task, &to, Some(failure)).await?;
            return Ok(true);
        }
        let why = if workflows::fail(wf, id).is_some() { "no attempts left" } else { "no fail edge" };
        match workflows::failed_end(wf) {
            Some(end) => {
                record(hub, kinds.1, task, format!("{id} · {line} · visit {visit} of {max} · {why} → {end}")).await;
                let end = end.to_string();
                self.goto(task, &end, Some(failure)).await?;
                Ok(true)
            }
            None => {
                self.workflow_failed(task.clone(), kinds.1, format!("{id} · {line} · visit {visit} of {max} · {why}"), failure).await?;
                Ok(false)
            }
        }
    }

    /// Ends the finished agent step's session unless a person is using it.
    async fn end_step_session(&self, task: &Task) -> Result<()> {
        let hub = &self.hub;
        if let Some(name) = task.session.clone()
            && hub.store.session(&name).await?.is_some_and(|s| s.state.is_live())
            && hub.close_session(&name, false, "Workflow step done").await? != crate::detect::EndDecision::End
        {
            hub.event(Level::Info, "task.kept", Some(&name), Some(task.id), "Workflow step done; session left open because a person is using it").await;
        }
        Ok(())
    }

    /// `retry_or_fail` for a workflow task (its session already ended): an agent node's fail edge, else the agent
    /// (the node, or the last agent for a failure after it such as a land conflict) again with the failure, up to
    /// `max_attempts` retries in the run; then failed.
    pub(super) async fn workflow_retry(&self, mut task: Task, reason: String) -> Result<()> {
        let hub = &self.hub;
        let max = hub.config.orchestrator.max_attempts;
        let headline = reason.lines().next().unwrap_or_default().chars().take(300).collect::<String>();
        let Some(wf) = self.workflow_of(&task).await else {
            let why = format!("workflow `{}` no longer exists", task.workflow.clone().unwrap_or_default());
            return self.workflow_failed(task, Kind::WF_ERROR, why, reason).await;
        };
        let mut st = task.workflow_state.clone().unwrap_or_default();
        let at = workflows::node(&wf, &st.node).filter(|n| n.kind == NK::Agent).map(|n| n.id.clone());
        task.result = Some(reason.clone());
        if let Some(id) = &at
            && let Some(to) = workflows::fail(&wf, id)
        {
            record(hub, Kind::WF_AGENT_FAIL, &task, format!("{id} · {} → {to}", dec::last_line(&headline))).await;
            let to_agent = workflows::node(&wf, to).is_some_and(|n| n.kind == NK::Agent);
            st.node = to.to_string();
            st.launched = false;
            st.failure = Some(reason);
            task.workflow_state = Some(st);
            task.status = if to_agent { TaskStatus::Pending } else { TaskStatus::Verifying };
            hub.put_task(&mut task).await?;
            if !to_agent {
                // Inside `finish` (a land conflict) this is refused; the next tick picks the task up.
                self.spawn_finish(task);
            }
            return Ok(());
        }
        let target = at.or(st.last_agent.clone());
        st.retries += 1;
        let used = format!("retry {} of {} in the workflow run", st.retries, (max - 1).max(0));
        match target {
            Some(agent) if (st.retries as i64) < max => {
                record(hub, Kind::RETRY_AGAIN, &task, format!("{used} · failure goes into the next prompt")).await;
                record(hub, Kind::WF_AGENT_RETRY, &task, format!("{agent} · {} · {used}", dec::last_line(&headline))).await;
                st.node = agent;
                st.launched = false;
                st.failure = Some(reason);
                task.workflow_state = Some(st);
                task.status = TaskStatus::Pending;
                hub.event(Level::Warning, "task.retry", task.session.as_deref(), Some(task.id), format!("Retrying: {headline}")).await;
                hub.put_task(&mut task).await
            }
            _ => {
                record(hub, Kind::RETRY_FAILED, &task, used).await;
                task.workflow_state = Some(st);
                task.status = TaskStatus::Failed;
                hub.event(Level::Error, "task.failed", task.session.as_deref(), Some(task.id), format!("Failed: {headline}")).await;
                hub.put_task(&mut task).await
            }
        }
    }

    /// The audit node's auditor, run headless in the task's folder, asked for `verdict`.
    async fn workflow_audit(&self, task: &Task, node: &WorkflowNode, verdict: &str) -> Result<Audit> {
        let hub = &self.hub;
        let name = node.agent.clone().unwrap_or_default();
        let (provider, model, instructions) = self.resolve_agent(task.project.as_deref(), &name, true).await;
        let Some(provider) = provider else { return Ok(Audit::Unavailable(format!("{name}: no installed provider with a headless command is free"))) };
        let spec = hub.spec(&provider)?;
        let mut prompt = String::new();
        if !instructions.trim().is_empty() {
            prompt.push_str(&format!("{}\n\n", instructions.trim()));
        }
        prompt.push_str(&Self::audit_prompt(task, verdict));
        let model = model.filter(|m| spec.models.is_empty() || spec.models.contains(m)).unwrap_or_else(|| spec.default_model.clone());
        let argv: Vec<String> =
            spec.headless.iter().map(|a| a.replace("{binary}", &spec.binary).replace("{model}", &model).replace("{session}", "audit").replace("{prompt}", &prompt)).collect();
        let mut cmd = Command::new(&argv[0]);
        cmd.args(&argv[1..]).current_dir(hub.task_dir(task).await).envs(hub.run_env(&provider, hub.command_env()));
        hub.event(Level::Info, "task.audit", None, Some(task.id), format!("Auditing with {name} ({provider})")).await;
        let (ok, output) = run_captured(cmd, Duration::from_secs(hub.config.orchestrator.audit.timeout_secs)).await?;
        let pass_at = output.rfind(verdict);
        let fail_at = output.rfind("VERDICT: FAIL");
        match (pass_at, fail_at) {
            (Some(p), Some(f)) if p > f => return Ok(Audit::Pass(format!("{name} ({provider})"))),
            (Some(_), None) => return Ok(Audit::Pass(format!("{name} ({provider})"))),
            (_, Some(f)) => {
                let line = output[f..].lines().next().unwrap_or_default();
                let reason = line.trim_start_matches("VERDICT: FAIL").trim_start_matches(':').trim();
                return Ok(Audit::Fail(format!("{name} ({provider}): {reason}")));
            }
            (None, None) => {}
        }
        if !ok || looks_like_quota(&output) {
            hub.cool_down(&provider, Utc::now() + chrono::Duration::minutes(hub.config.orchestrator.provider_cooldown_minutes as i64));
        }
        Ok(Audit::Unavailable(format!("{name} ({provider}) gave no verdict")))
    }
}
