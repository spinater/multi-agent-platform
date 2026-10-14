//! The orchestrator's decision model: every branch point of a task's flow through `orchestrator.rs` records a
//! `decision.<node>.<outcome>` event (task id set, the message is the detail the branch looked at). The Decisions
//! screen replays them as a graph; `kinds()` maps each event kind to the edge it takes.
//!
//! Not to be confused with Reflex decisions (`decide.rs`, `model::Decision`), which answer questions for projects.

use std::time::Duration;

use chrono::{DateTime, Utc};
use tokio::sync::{Notify, watch};

use crate::hub::Hub;
use crate::model::{
    DecisionCounts, DecisionEdge, DecisionEdgeCount, DecisionKind, DecisionModel, DecisionNode as N, DecisionPoint, Level, Task,
};

/// One kind of decision event: where the task was, where the decision sends it, and the outcome in words.
#[derive(Debug, Clone, Copy)]
pub struct Kind {
    pub kind: &'static str,
    pub from: N,
    pub to: N,
    pub level: Level,
    pub outcome: &'static str,
}

const fn k(kind: &'static str, from: N, to: N, level: Level, outcome: &'static str) -> Kind {
    Kind { kind, from, to, level, outcome }
}

use Level::{Error as E, Info as I, Success as S, Warning as W};

impl Kind {
    // Orchestrator::tick: dependencies and worker slots.
    pub const READY_GO: Kind = k("decision.ready.go", N::Ready, N::Pick, I, "ready → pick provider");
    pub const READY_FULL: Kind = k("decision.ready.project_full", N::Ready, N::Ready, I, "project at its parallel limit · waiting");
    pub const READY_SHARED_BUSY: Kind = k("decision.ready.shared_busy", N::Ready, N::Ready, I, "shared folder busy · not cleared to run beside it · waiting");
    pub const READY_AWAIT_ANALYST: Kind = k("decision.ready.awaiting_analyst", N::Ready, N::Ready, I, "shared folder busy · waiting for the parallel analyst");
    pub const READY_CLEARED: Kind = k("decision.ready.cleared", N::Ready, N::Ready, S, "analyst cleared it to share the folder · paths disjoint");
    pub const READY_BLOCKED: Kind = k("decision.ready.blocked", N::Ready, N::Blocked, W, "blocked by a dependency");
    // start() / pick_provider().
    pub const PICK_CHOSEN: Kind = k("decision.pick.chosen", N::Pick, N::Launch, I, "provider chosen → launch");
    pub const PICK_NONE: Kind = k("decision.pick.none", N::Pick, N::Pick, W, "no provider available · waiting");
    // start(): the project's Reflex tier gate, for a task without a pinned model.
    pub const PICK_TIER_LIGHT: Kind = k("decision.pick.tier_light", N::Pick, N::Pick, I, "Reflex: light task · tier model");
    pub const PICK_TIER_STANDARD: Kind = k("decision.pick.tier_standard", N::Pick, N::Pick, I, "Reflex: standard task · tier model");
    pub const PICK_TIER_HEAVY: Kind = k("decision.pick.tier_heavy", N::Pick, N::Pick, I, "Reflex: heavy task · tier model");
    pub const PICK_TIER_ABSTAINED: Kind = k("decision.pick.tier_abstained", N::Pick, N::Pick, I, "Reflex unsure or not running · usual model");
    // start() / hub.launch().
    pub const LAUNCH_STARTED: Kind = k("decision.launch.started", N::Launch, N::Watch, I, "session started → watch");
    pub const LAUNCH_ERROR: Kind = k("decision.launch.error", N::Launch, N::Retry, E, "launch error → retry or fail");
    // check_running().
    pub const WATCH_DONE: Kind = k("decision.watch.done", N::Watch, N::Verify, I, "reported done → verify");
    pub const WATCH_FAILED: Kind = k("decision.watch.failed", N::Watch, N::Retry, W, "reported failure → retry or fail");
    pub const WATCH_EXITED: Kind = k("decision.watch.exited", N::Watch, N::Retry, W, "session exited without a result → retry or fail");
    pub const WATCH_AUTH: Kind = k("decision.watch.auth", N::Watch, N::Retry, W, "provider needs sign-in → retry or fail");
    pub const WATCH_TRUST: Kind = k("decision.watch.trust", N::Watch, N::Watch, W, "CLI asks to trust its folder · waiting for the owner");
    pub const WATCH_TIMEOUT: Kind = k("decision.watch.timeout", N::Watch, N::Retry, W, "timed out → retry or fail");
    pub const WATCH_LOST: Kind = k("decision.watch.lost", N::Watch, N::Retry, W, "running without a session → retry or fail");
    // finish(): verify commands.
    pub const VERIFY_CMD_START: Kind = k("decision.verify.cmd_start", N::Verify, N::Verify, I, "running a verify command");
    pub const VERIFY_CMD_PASS: Kind = k("decision.verify.cmd_pass", N::Verify, N::Verify, I, "command passed · next");
    pub const VERIFY_FAIL: Kind = k("decision.verify.fail", N::Verify, N::Retry, W, "command failed → retry or fail");
    pub const VERIFY_PASS: Kind = k("decision.verify.pass", N::Verify, N::Audit, S, "all exit 0 → audit");
    // finish() / audit().
    pub const AUDIT_OFF: Kind = k("decision.audit.off", N::Audit, N::Land, I, "audit off → land");
    // finish(): the project's Reflex audit gate, before any auditor is asked.
    pub const AUDIT_SKIPPED_REFLEX: Kind = k("decision.audit.skipped_reflex", N::Audit, N::Land, S, "Reflex: no audit needed → land");
    pub const AUDIT_REFLEX_SAID_AUDIT: Kind = k("decision.audit.reflex_said_audit", N::Audit, N::Audit, I, "Reflex: audit needed · asking an auditor");
    pub const AUDIT_REFLEX_ABSTAINED: Kind = k("decision.audit.reflex_abstained", N::Audit, N::Audit, I, "Reflex unsure or not running · asking an auditor");
    pub const AUDIT_ASK: Kind = k("decision.audit.ask", N::Audit, N::Audit, I, "asking an auditor");
    pub const AUDIT_UNAVAILABLE: Kind = k("decision.audit.unavailable", N::Audit, N::Audit, W, "auditor unavailable · next in chain");
    pub const AUDIT_PASS: Kind = k("decision.audit.pass", N::Audit, N::Land, S, "VERDICT: PASS → land");
    pub const AUDIT_FAIL: Kind = k("decision.audit.fail", N::Audit, N::Retry, W, "VERDICT: FAIL → retry or fail");
    pub const AUDIT_SKIPPED: Kind = k("decision.audit.skipped", N::Audit, N::Land, W, "no auditor · accepted on verification → land");
    pub const AUDIT_NO_AUDITOR: Kind = k("decision.audit.no_auditor", N::Audit, N::Retry, W, "no auditor and audit required → retry or fail");
    // finish(): post_done hooks.
    // worktrees.rs: a branch and worktree per task, made at launch and landed after the audit.
    pub const PREP_READY: Kind = k("decision.launch.worktree", N::Launch, N::Launch, I, "worktree ready on the task's branch");
    pub const PREP_FAILED: Kind = k("decision.launch.worktree_failed", N::Launch, N::Retry, W, "worktree or setup failed → retry or fail");
    pub const PREP_FATAL: Kind = k("decision.launch.worktree_fatal", N::Launch, N::Failed, E, "the project cannot have worktrees → failed (a person decides)");
    pub const LAND_SHARED: Kind = k("decision.land.shared", N::Land, N::Post, I, "shared folder · nothing to land → post_done");
    pub const LAND_COMMITTED: Kind = k("decision.land.committed", N::Land, N::Land, I, "committed what the worker left");
    pub const LAND_REBASED: Kind = k("decision.land.rebased", N::Land, N::Land, I, "base moved on · rebased · verifying again");
    pub const LAND_REVERIFY_FAIL: Kind = k("decision.land.reverify_fail", N::Land, N::Retry, W, "verify failed after the rebase → retry or fail");
    pub const LAND_CONFLICT: Kind = k("decision.land.conflict", N::Land, N::Retry, W, "rebase conflict → the agent resolves it");
    pub const LAND_NOTHING: Kind = k("decision.land.nothing", N::Land, N::Post, I, "no commits beyond the base → post_done");
    pub const LAND_MERGED: Kind = k("decision.land.merged", N::Land, N::Post, S, "fast-forwarded the base → post_done");
    pub const LAND_PUSHED: Kind = k("decision.land.pushed", N::Land, N::Post, S, "pushed · pull request → post_done");
    pub const LAND_KEEP: Kind = k("decision.land.keep", N::Land, N::Review, I, "land = keep → the owner reviews it");
    pub const LAND_BLOCKED: Kind = k("decision.land.blocked", N::Land, N::Review, W, "cannot land without a person → review");
    pub const REVIEW_LAND: Kind = k("decision.review.land", N::Review, N::Land, I, "the owner lands it");
    pub const REVIEW_SEND_BACK: Kind = k("decision.review.send_back", N::Review, N::Ready, W, "the owner sends it back to an agent");

    pub const POST_CMD_PASS: Kind = k("decision.post.cmd_pass", N::Post, N::Post, I, "hook passed · next");
    pub const POST_PASS: Kind = k("decision.post.pass", N::Post, N::End, I, "hooks ok → end decision");
    pub const POST_FAIL: Kind = k("decision.post.fail", N::Post, N::Failed, E, "hook failed → failed (a person decides)");
    // finish(): detect::end_decision via hub.close_session.
    pub const END_CLOSED: Kind = k("decision.end.closed", N::End, N::Done, S, "EndDecision::End · session closed → done");
    pub const END_KEPT: Kind = k("decision.end.kept", N::End, N::Done, S, "person using the session · left open → done");
    // retry_or_fail() and start()'s launch error.
    pub const RETRY_AGAIN: Kind = k("decision.retry.again", N::Retry, N::Ready, W, "attempt left → back to ready, failure added to the prompt");
    pub const RETRY_FAILED: Kind = k("decision.retry.failed", N::Retry, N::Failed, E, "no attempts left → failed");
    pub const RETRY_KEPT: Kind = k("decision.retry.kept", N::Retry, N::Watch, W, "person attached or typing → keep watching");
}

impl Kind {
    /// The tier gate's outcome for Reflex's answer (`None`: it abstained).
    pub fn tier(tier: Option<&str>) -> Kind {
        match tier {
            Some("light") => Kind::PICK_TIER_LIGHT,
            Some("standard") => Kind::PICK_TIER_STANDARD,
            Some("heavy") => Kind::PICK_TIER_HEAVY,
            _ => Kind::PICK_TIER_ABSTAINED,
        }
    }
}

pub const ALL: &[Kind] = &[
    Kind::READY_GO,
    Kind::READY_BLOCKED,
    Kind::READY_FULL,
    Kind::READY_SHARED_BUSY,
    Kind::READY_AWAIT_ANALYST,
    Kind::READY_CLEARED,
    Kind::PICK_CHOSEN,
    Kind::PICK_NONE,
    Kind::PICK_TIER_LIGHT,
    Kind::PICK_TIER_STANDARD,
    Kind::PICK_TIER_HEAVY,
    Kind::PICK_TIER_ABSTAINED,
    Kind::LAUNCH_STARTED,
    Kind::LAUNCH_ERROR,
    Kind::WATCH_DONE,
    Kind::WATCH_FAILED,
    Kind::WATCH_EXITED,
    Kind::WATCH_AUTH,
    Kind::WATCH_TRUST,
    Kind::WATCH_TIMEOUT,
    Kind::WATCH_LOST,
    Kind::VERIFY_CMD_START,
    Kind::VERIFY_CMD_PASS,
    Kind::VERIFY_FAIL,
    Kind::VERIFY_PASS,
    Kind::AUDIT_OFF,
    Kind::AUDIT_SKIPPED_REFLEX,
    Kind::AUDIT_REFLEX_SAID_AUDIT,
    Kind::AUDIT_REFLEX_ABSTAINED,
    Kind::AUDIT_ASK,
    Kind::AUDIT_UNAVAILABLE,
    Kind::AUDIT_PASS,
    Kind::AUDIT_FAIL,
    Kind::AUDIT_SKIPPED,
    Kind::AUDIT_NO_AUDITOR,
    Kind::PREP_READY,
    Kind::PREP_FAILED,
    Kind::PREP_FATAL,
    Kind::LAND_SHARED,
    Kind::LAND_COMMITTED,
    Kind::LAND_REBASED,
    Kind::LAND_REVERIFY_FAIL,
    Kind::LAND_CONFLICT,
    Kind::LAND_NOTHING,
    Kind::LAND_MERGED,
    Kind::LAND_PUSHED,
    Kind::LAND_KEEP,
    Kind::LAND_BLOCKED,
    Kind::REVIEW_LAND,
    Kind::REVIEW_SEND_BACK,
    Kind::POST_CMD_PASS,
    Kind::POST_PASS,
    Kind::POST_FAIL,
    Kind::END_CLOSED,
    Kind::END_KEPT,
    Kind::RETRY_AGAIN,
    Kind::RETRY_FAILED,
    Kind::RETRY_KEPT,
];

/// Records a decision for `task` through `hub.event`. A decision identical (kind and message) to one of the task's
/// last two is dropped, so a task parked at a branch (no provider, a person attached) does not log every tick.
pub async fn record(hub: &Hub, kind: Kind, task: &Task, detail: impl AsRef<str>) {
    let detail = detail.as_ref();
    match hub.store.decision_events(task.id, 2).await {
        Ok(last) if last.iter().any(|e| e.kind == kind.kind && e.message == detail) => return,
        Ok(_) => {}
        Err(err) => tracing::warn!(task = task.id, "reading the decision trace: {err:#}"),
    }
    // Before a launch, `task.session` is still the previous attempt's.
    let fresh = matches!(kind.from, N::Ready | N::Pick) || kind.kind == Kind::LAUNCH_ERROR.kind;
    let session = if fresh { None } else { task.session.as_deref() };
    hub.event(kind.level, kind.kind, session, Some(task.id), detail).await;
}

// ---- details for the messages

/// "#1 done · #2 done · slot 3/8" for a task the tick is about to start.
pub fn ready_detail(task: &Task, active: usize, max_parallel: usize) -> String {
    let deps = if task.depends_on.is_empty() { "no dependencies".to_string() } else { task.depends_on.iter().map(|d| format!("#{d} done")).collect::<Vec<_>>().join(" · ") };
    format!("{deps} · slot {}/{max_parallel}", active + 1)
}

/// Why each candidate before `chosen` was skipped, e.g. "order claude → codex · claude cooling down until 14:05 → codex".
pub async fn pick_detail(hub: &Hub, task: &Task, chosen: Option<&str>) -> String {
    // The same candidates as `pick_provider`: pinned, else the project's providers, else `provider_order`, minus
    // providers at the project's session limit.
    let candidates = crate::project_settings::candidates(hub, task).await;
    let head = match &task.provider {
        Some(p) => format!("pinned: {p}"),
        None if candidates.is_empty() => "no candidate provider (all at the project's session limit)".to_string(),
        None => format!("order {}", candidates.join(" → ")),
    };
    let mut parts = vec![head];
    for p in candidates.iter().take_while(|p| Some(p.as_str()) != chosen) {
        let why = if !hub.provider_available(p) {
            "not found on PATH".to_string()
        } else if let Some(until) = hub.cooldown_until(p) {
            format!("cooling down until {}", until.with_timezone(&chrono::Local).format("%H:%M"))
        } else {
            "skipped".to_string()
        };
        parts.push(format!("{p} {why}"));
    }
    match chosen {
        Some(p) => format!("{} → {p}", parts.join(" · ")),
        None => parts.join(" · "),
    }
}

/// The last non-empty line of a command's output, shortened for a message.
pub fn last_line(output: &str) -> String {
    let line = output.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("no output");
    let short: String = line.chars().take(160).collect();
    if short.len() < line.len() { format!("{short}…") } else { short }
}

pub fn secs(d: Duration) -> String {
    format!("{:.1}s", d.as_secs_f64())
}

// ---- the model and counts for the API

fn point(id: N, label: &str, question: &str, rule: &str, source: &str, terminal: bool) -> DecisionPoint {
    DecisionPoint { id, label: label.into(), question: question.into(), rule: rule.into(), source: source.into(), terminal }
}

fn edge(from: N, to: N, label: &str) -> DecisionEdge {
    DecisionEdge { from, to, label: label.into() }
}

pub fn model(hub: &Hub, tick_secs: u64) -> DecisionModel {
    let cfg = &hub.config.orchestrator;
    let nodes = vec![
        point(N::Ready, "Ready?", "Are all depends_on tasks done, and is a worker slot free?", "every depends_on task is done && active < max_parallel (machine-wide, live) && the project's active tasks < parallel.max; in a shared-folder project a task starts beside running ones only when the parallel analyst cleared all of them as independent with disjoint paths; a failed or cancelled dependency blocks", "Orchestrator::tick · max_parallel · project parallel · parallel.rs", false),
        point(N::Pick, "Pick provider", "Which provider takes this task, with which model?", "task.provider if pinned, else the first of the project's providers (or provider_order) that is on PATH, not cooling down and under its session limit; with the project's tier gate on and no pinned model, a confident Reflex `tier` answer (light | standard | heavy) takes that provider's model from orchestrator.tiers, an unsure one keeps the usual model", "pick_provider() · tier_gate() · project settings · orchestrator.provider_order · orchestrator.tiers", false),
        point(N::Launch, "Launch", "Did the worker session start?", "hub.launch(LaunchRequest) returns Ok; an error cools the provider down for 5m", "start() · hub.launch", false),
        point(N::Watch, "Watch", "What has the worker reported?", "$MAP_RESULT_FILE status done | failed; session exited or needs_auth; started > task_timeout_minutes ago; needs_trust waits for the owner (no timeout)", "check_running() · orchestrator.task_timeout_minutes", false),
        point(N::Verify, "Verify", "Does every verify command exit 0?", "each cfg.verify command, in order, exits 0 (30m limit each)", "finish() · orchestrator.verify", false),
        point(N::Audit, "Audit", "Does the change need an audit, and what does the auditor answer?", "with the project's audit gate on, a confident Reflex `needs_audit` = no accepts the task on verification; otherwise (yes, unsure, Reflex not running) the first auditor in audit.chain with a VERDICT line: PASS | FAIL; none → audit.required decides", "audit_gate() · audit() · orchestrator.audit · project reflex_gates", false),
        point(N::Land, "Land", "Where does the task's branch go?", "shared folder: nothing to land; else commit what the worker left, then by branches.land: merge (rebase onto the base, verify again if it moved, fast-forward the base; a conflict goes back to the agent) | pull_request (push, open a PR with gh or glab) | keep (the owner reviews)", "finish() · worktrees.rs · project branches", false),
        point(N::Review, "Review", "Waiting for the owner to land it.", "land = keep, the project folder has changes in the way, or a push failed; the owner merges, opens a pull request, sends it back or discards it", "TaskStatus::Review · PATCH /api/tasks/{id}", false),
        point(N::Post, "post_done", "Do the post_done hooks pass?", "each cfg.post_done command exits 0; a failure fails the task without a retry", "finish() · orchestrator.post_done", false),
        point(N::End, "End decision", "Can the worker session be closed?", "End unless a tmux client is attached or the input line has text", "detect::end_decision(clients, pane)", false),
        point(N::Retry, "Retry or fail", "Is there an attempt left?", "attempts < max_attempts → pending again with the failure in the prompt; a person in the session keeps it running", "retry_or_fail() · orchestrator.max_attempts", false),
        point(N::Blocked, "Blocked", "A dependency failed or was cancelled.", "a depends_on task is failed or cancelled; retry it to unblock", "tick · \"Blocked: dependency #n\"", true),
        point(N::Failed, "Failed", "Task failed. A person decides what next.", "no attempts left, or a post_done hook failed", "TaskStatus::Failed", true),
        point(N::Done, "Done", "Task is done.", "verified, audited (or accepted), hooks ran", "TaskStatus::Done", true),
    ];
    let edges = vec![
        edge(N::Ready, N::Pick, "deps done · slot free"),
        edge(N::Ready, N::Blocked, "dep failed"),
        edge(N::Pick, N::Launch, "first available"),
        edge(N::Launch, N::Watch, "started"),
        edge(N::Launch, N::Retry, "launch error"),
        edge(N::Watch, N::Verify, "result: done"),
        edge(N::Watch, N::Retry, "failed · exited · auth · timeout"),
        edge(N::Verify, N::Audit, "all pass"),
        edge(N::Verify, N::Retry, "command failed"),
        edge(N::Launch, N::Failed, "no worktree possible"),
        edge(N::Audit, N::Land, "PASS · off · skipped · Reflex: no audit"),
        edge(N::Land, N::Post, "merged · pushed · nothing · shared"),
        edge(N::Land, N::Review, "keep · blocked"),
        edge(N::Land, N::Retry, "conflict · verify failed after rebase"),
        edge(N::Review, N::Land, "owner: merge or PR"),
        edge(N::Review, N::Ready, "owner: send back"),
        edge(N::Audit, N::Retry, "FAIL"),
        edge(N::Post, N::End, "hooks ok"),
        edge(N::Post, N::Failed, "hook failed"),
        edge(N::End, N::Done, "End"),
        edge(N::Retry, N::Ready, "attempt left"),
        edge(N::Retry, N::Failed, "no attempts left"),
        edge(N::Retry, N::Watch, "person attached"),
    ];
    let kinds = ALL.iter().map(|k| DecisionKind { kind: k.kind.into(), from: k.from, to: k.to, outcome: k.outcome.into() }).collect();
    DecisionModel {
        nodes,
        edges,
        kinds,
        tick_secs,
        max_parallel: hub.max_parallel(),
        max_attempts: cfg.max_attempts,
        task_timeout_minutes: cfg.task_timeout_minutes,
        provider_order: cfg.provider_order.clone(),
        verify: cfg.verify.clone(),
        post_done: cfg.post_done.clone(),
        audit_enabled: cfg.audit.enabled,
        audit_required: cfg.audit.required,
        audit_chain: cfg.audit.chain.clone(),
    }
}

/// Edge counts since `since`; progress kinds (`from == to`) are left out.
pub async fn counts(hub: &Hub, since: DateTime<Utc>, project: Option<&str>) -> anyhow::Result<DecisionCounts> {
    let mut edges: Vec<DecisionEdgeCount> = vec![];
    for (kind, n) in hub.store.decision_kind_counts(since, project).await? {
        let Some(k) = ALL.iter().find(|k| k.kind == kind) else { continue };
        if k.from == k.to {
            continue;
        }
        match edges.iter_mut().find(|e| e.from == k.from && e.to == k.to) {
            Some(e) => e.count += n,
            None => edges.push(DecisionEdgeCount { from: k.from, to: k.to, count: n }),
        }
    }
    Ok(DecisionCounts { since, edges })
}

// ---- stepping a paused orchestrator

/// Lets the API run exactly one tick of the (paused) orchestrator loop and wait for it.
pub struct Stepper {
    request: Notify,
    done: watch::Sender<u64>,
}

impl Default for Stepper {
    fn default() -> Self {
        Self { request: Notify::new(), done: watch::Sender::new(0) }
    }
}

impl Stepper {
    /// Sleeps for `tick`; returns true early when a step was requested.
    pub async fn sleep(&self, tick: Duration) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(tick) => false,
            _ = self.request.notified() => true,
        }
    }

    pub fn finished(&self) {
        self.done.send_modify(|n| *n += 1);
    }

    /// Asks the loop for one tick; false when it did not finish within `timeout`.
    pub async fn step(&self, timeout: Duration) -> bool {
        let mut rx = self.done.subscribe();
        rx.mark_unchanged();
        self.request.notify_one();
        tokio::time::timeout(timeout, rx.changed()).await.is_ok_and(|r| r.is_ok())
    }
}
