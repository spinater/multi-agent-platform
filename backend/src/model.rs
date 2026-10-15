//! Types shared by the store, the API and the frontend. `cargo test` exports them to
//! `frontend/lib/generated/` through ts-rs, so the dashboard never drifts from the server.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// What a session is for. Workers run tasks, the supervisor role is for a long-lived planning agent,
/// manual sessions are opened by a person from the dashboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Role {
    Worker,
    Supervisor,
    Manual,
}

/// Session state as the watcher sees it on its last tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SessionState {
    Starting,
    Working,
    Idle,
    /// A person has typed on the input line, or the agent is asking a question.
    WaitingInput,
    /// The provider reported a quota or rate limit; the watcher wakes the session at `reset_at`.
    RateLimited,
    NeedsAuth,
    /// The CLI asks whether to trust the folder it runs in (`trust_markers`); a person answers it, or presses
    /// "Trust this folder" on the dashboard.
    NeedsTrust,
    Error,
    /// No pane change for the configured stall window.
    Stalled,
    /// The tmux session is gone.
    Exited,
}

impl SessionState {
    pub fn is_live(self) -> bool {
        self != SessionState::Exited
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Session {
    /// The tmux session name; always starts with the configured prefix.
    pub name: String,
    pub provider: String,
    pub model: String,
    pub role: Role,
    #[ts(type = "number | null")]
    pub task_id: Option<i64>,
    /// The project whose workspace the agent runs in; `None` is `workspace.dir`.
    pub project: Option<String>,
    pub state: SessionState,
    /// Human-readable reason for the state (error line, rate-limit text, stall duration).
    pub detail: Option<String>,
    pub rate_limit_reset_at: Option<DateTime<Utc>>,
    #[ts(type = "number")]
    pub wakeups: i64,
    pub last_line: Option<String>,
    pub attached: bool,
    pub created_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    /// Who started it: `orchestrator` (a task's worker), `owner`, `map` (a manager MAP keeps alive), or
    /// `manager:<slug>` (a lane a project's manager opened). Empty for sessions from before this was recorded.
    #[serde(default)]
    pub spawned_by: String,
}

/// `POST /api/sessions/{name}/trust`: what MAP did about the folder-trust question on the session's screen.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrustAnswer {
    pub session: String,
    /// The folder the session runs in (canonical path).
    pub folder: String,
    /// The CLI's own record: "marked in <file>", "already trusted in <file>", why it was not written.
    pub recorded: String,
    /// The keys sent to the pane, in order (`Down`, `Enter`).
    pub keys: Vec<String>,
    /// The question is gone from the screen after the keys.
    pub answered: bool,
}

/// `GET /api/projects/{slug}/trust`: whether each installed CLI trusts the project's folder (read from its config).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderTrust {
    /// The folder (canonical path).
    pub folder: String,
    /// `workspace.trust_folders`: MAP marks folders trusted before it starts an agent there.
    pub enabled: bool,
    pub providers: Vec<ProviderTrust>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderTrust {
    pub provider: String,
    /// The CLI's config file MAP reads and writes; `None` when MAP cannot mark folders for this CLI.
    pub file: Option<String>,
    /// `None`: unknown (no file MAP can read, or it does not parse).
    pub trusted: Option<bool>,
    pub note: String,
}

/// `POST /api/projects/{slug}/trust`: what marking the folder did, and the state after it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FolderTrusted {
    pub checks: Vec<CheckLine>,
    pub trust: FolderTrust,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TaskStatus {
    /// Waiting for dependencies or a free worker slot.
    Pending,
    Running,
    Verifying,
    Auditing,
    /// Committing the task's branch and landing it (rebase + fast-forward into its base, or push + pull request).
    Landing,
    /// The work passed but waits for the owner: `land = keep`, a project folder with changes in the way, or a push
    /// that failed. Merge, push, send back or discard it from the dashboard.
    Review,
    Done,
    Failed,
    Cancelled,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Pending => "pending",
            TaskStatus::Running => "running",
            TaskStatus::Verifying => "verifying",
            TaskStatus::Auditing => "auditing",
            TaskStatus::Landing => "landing",
            TaskStatus::Review => "review",
            TaskStatus::Done => "done",
            TaskStatus::Failed => "failed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Task {
    #[ts(type = "number")]
    pub id: i64,
    pub title: String,
    pub prompt: String,
    #[ts(type = "number[]")]
    pub depends_on: Vec<i64>,
    /// Pin the task to one provider; `None` lets the orchestrator pick by `provider_order`.
    pub provider: Option<String>,
    pub model: Option<String>,
    /// Runs in this project's workspace; `None` is `workspace.dir`.
    pub project: Option<String>,
    pub status: TaskStatus,
    pub session: Option<String>,
    #[ts(type = "number")]
    pub attempts: i64,
    /// Summary from the agent's result file, or the reason the task failed.
    pub result: Option<String>,
    /// The task's own branch and worktree, for a project with `branches.mode = worktree`; `None` when it runs in the
    /// project folder itself.
    pub git: Option<TaskGit>,
    /// The parallel analyst's verdict, for a task of a shared-folder project; `None` until it has looked.
    pub scope: Option<TaskScope>,
    /// The intake gate's verdict, asked before any LLM runs the task; `None` until it has looked (or the gate is off).
    pub intake: Option<TaskIntake>,
    /// Who created it: `owner`, `manager` (a project's manager, also for Telegram `/ask`), `planner` (an approved
    /// plan).
    #[serde(default)]
    pub created_by: String,
    /// The workflow it runs (`Workflow::id`): pinned at creation, or chosen by a workflow's trigger when the task first
    /// becomes ready (`decision.workflow.chosen`). `None`: the fixed pipeline (verify, audit, land, post_done).
    #[serde(default)]
    pub workflow: Option<String>,
    /// Where the task is in its workflow, saved at every step so a restart resumes there.
    #[serde(default)]
    pub workflow_state: Option<TaskWorkflowState>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A workflow task's place in its graph (`orchestrator.rs`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct TaskWorkflowState {
    /// The node the task is at.
    pub node: String,
    /// An agent node's session was started for this visit (it is the node's worker, not one still to launch).
    pub launched: bool,
    /// Times each node was run (verify and audit nodes stop following their fail edge at `max_attempts`).
    pub visits: BTreeMap<String, u32>,
    /// Retries after an agent failed, exited, timed out or hit a conflict (bounded by `orchestrator.max_attempts`).
    pub retries: u32,
    /// Nodes run in all; a run past 200 fails, so a graph with a loop cannot spin forever.
    pub steps: u32,
    /// The last agent node launched: a land conflict goes back there.
    pub last_agent: Option<String>,
    /// The failure the next agent launch gets in its prompt (a verify's output, an audit's reason).
    pub failure: Option<String>,
    /// Why this workflow: `pinned`, `keyword <word>` or `default`.
    pub chosen_by: String,
}

/// What the intake gate made of a task before an LLM sees it: the project's Reflex (Thai through the glossary) and
/// a local look for a done task it repeats. A held task waits for the owner: start it anyway, edit it or cancel it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct TaskIntake {
    pub verdict: IntakeVerdict,
    /// Why, in one line: Reflex's confidence and decision, or the similar task.
    pub reason: String,
    /// Reflex's `tier` from the same call, used by the tier gate without asking again.
    pub tier: Option<String>,
    /// The call asked `tier` (the tier gate was on): the tier gate uses `tier` (`None`: unsure) instead of asking.
    pub tier_asked: bool,
    /// Reflex is sure the task changes files other tasks share: it runs alone, without asking the parallel analyst.
    pub shared_files: bool,
    /// The done task this one looks like (`duplicate`).
    #[ts(type = "number | null")]
    pub similar: Option<i64>,
    /// The owner started it anyway.
    pub released: bool,
    pub at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum IntakeVerdict {
    /// Clear enough: on to the worker.
    #[default]
    Ready,
    /// Reflex is sure the prompt is too unclear to start: the owner says more first.
    Unclear,
    /// Reflex is sure it is big enough to plan first (the planner splits it into tasks).
    PlanFirst,
    /// Nearly the same as a task already done.
    Duplicate,
    /// Reflex unsure or not running: on to the worker as before.
    Unsure,
}

impl TaskIntake {
    /// Waiting for the owner.
    pub fn holds(&self) -> bool {
        !self.released && matches!(self.verdict, IntakeVerdict::Unclear | IntakeVerdict::PlanFirst | IntakeVerdict::Duplicate)
    }
}

/// A task's branch (`map/task-<id>`), made from `base` in a worktree of its own so tasks of one project run side
/// by side without touching each other's files.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct TaskGit {
    pub branch: String,
    /// The branch it was made from and lands into.
    pub base: String,
    /// The worktree folder while it exists; `None` once removed.
    pub worktree: Option<String>,
    /// How the owner asked to land it from `review` (`None`: the project's `branches.land`).
    pub land: Option<LandMode>,
    /// The base commit it landed as, after a merge.
    pub merged: Option<String>,
    /// Set once the branch has been pushed.
    pub pushed: bool,
    /// The pull or merge request opened for it.
    pub pr_url: Option<String>,
    /// What landing did last, or why the task waits in `review`.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Level {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Event {
    #[ts(type = "number")]
    pub id: i64,
    pub at: DateTime<Utc>,
    pub level: Level,
    /// Short machine-readable category, e.g. `session.rate_limited` or `task.done`.
    pub kind: String,
    pub session: Option<String>,
    #[ts(type = "number | null")]
    pub task_id: Option<i64>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderInfo {
    pub name: String,
    pub models: Vec<String>,
    pub default_model: String,
    pub available: bool,
    /// Set while the orchestrator avoids the provider after a quota or auth failure.
    pub cooldown_until: Option<DateTime<Utc>>,
    /// From the provider's credential files (existence and parse-ability only) or its API key variable;
    /// `None` when the platform has no way to tell.
    pub signed_in: Option<bool>,
    /// How the provider signs in; `POST /api/providers/{name}/login` runs it in a helper session.
    pub login: LoginInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OrchestratorStatus {
    pub running: bool,
    /// Tasks at once across all projects: `[orchestrator] max_parallel`, or the value set from the dashboard
    /// (`POST /api/orchestrator {"max_parallel"}`, kept across restarts).
    #[ts(type = "number")]
    pub max_parallel: usize,
    /// `[orchestrator] max_parallel` in map.toml, which the dashboard's value replaces.
    #[ts(type = "number")]
    pub config_max_parallel: usize,
    #[ts(type = "number")]
    pub active: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Overview {
    pub sessions: Vec<Session>,
    pub tasks: Vec<Task>,
    pub providers: Vec<ProviderInfo>,
    pub orchestrator: OrchestratorStatus,
    pub projects: Vec<ProjectInfo>,
    pub server: ServerInfo,
}

/// Facts about this server for the dashboard's status bar; all of them come from `map.toml`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServerInfo {
    pub version: String,
    /// `workspace.dir`: where tasks and sessions without a project run.
    pub workspace_dir: String,
    pub session_prefix: String,
    /// The SQLite database file.
    pub database: String,
    #[ts(type = "number")]
    pub watcher_interval_secs: u64,
    /// `reflex --version` of `reflex.binary`, e.g. `reflex 0.2.4`; `None` when it is not installed.
    pub reflex_version: Option<String>,
    /// `workspace.tmux_socket`: the private tmux server (`tmux -L <name>`) agents run on; `None`: tmux's default.
    pub tmux_socket: Option<String>,
    /// What a project fallback of `api` calls, from `[reflex] api_url`: "Anthropic API", "OpenRouter API", …
    pub api_fallback: String,
}

/// Messages pushed to dashboard clients over `/api/ws`. Sent once and dropped, so a large `Task` variant costs nothing.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
#[allow(clippy::large_enum_variant)]
pub enum ServerMessage {
    Event { event: Event },
    Session { session: Session },
    SessionRemoved { name: String },
    Task { task: Task },
    TaskRemoved {
        #[ts(type = "number")]
        id: i64,
    },
    Orchestrator { status: OrchestratorStatus },
    /// A project was created or changed, its Reflex engine changed state, or it recorded a decision.
    Project { project: Box<ProjectInfo> },
    ProjectRemoved { slug: String },
    /// The client fell behind and missed messages; reload `/api/overview`.
    Resync,
    /// A new usage reading (every `usage.interval_secs`, or after `POST /api/usage/refresh`).
    Usage { report: UsageReport },
    /// A new update check (every `updates.interval_secs`, after `POST /api/updates/refresh`, or when an update
    /// session ends), or an update session started.
    Updates { report: UpdatesReport },
    /// A plan was submitted, edited, approved, rejected or superseded.
    Plan { plan: Box<Plan> },
    /// A provider's availability, sign-in, API key or cooldown changed (checked every watcher tick and when a
    /// sign-in session ends or reports success): the whole list, as `/api/providers` returns it.
    Providers { providers: Vec<ProviderInfo> },
}

// ---- projects and decisions

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum QuestionKind {
    /// Pick one of `options`.
    Choice,
    /// Rate against `options`, ordered lowest level first.
    Score,
    /// Yes or no; takes no `options`.
    Noul,
}

/// A typed question, in Reflex's wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Question {
    pub id: String,
    pub kind: QuestionKind,
    pub prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub options: Option<Vec<String>>,
    /// What decides between the options.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub criteria: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Project {
    pub slug: String,
    pub name: String,
    pub description: String,
    /// Loopback port of the project's Reflex process.
    pub port: u16,
    /// Lowest Reflex confidence accepted without the fallback. `None` sends every question to the fallback.
    pub threshold: Option<f64>,
    /// Who answers when Reflex is unsure: `none`, `api` (Anthropic API) or a provider name run headless.
    pub fallback: String,
    /// Model for the fallback; `None` uses `reflex.api_model` or the provider's default.
    pub model: Option<String>,
    /// Saved questions for the playground and for callers.
    pub questions: Vec<Question>,
    pub created_at: DateTime<Utc>,
    /// Where the code came from and how the orchestrator treats the project's tasks.
    #[serde(default)]
    pub settings: ProjectSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ReflexState {
    Stopped,
    Starting,
    Running,
    /// The corpus repo has no domains yet.
    NoCorpus,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReflexStatus {
    pub state: ReflexState,
    pub message: String,
    /// Top-level folders (or `.md` files) of the corpus, one per Reflex domain.
    pub domains: Vec<String>,
    pub since: DateTime<Utc>,
}

/// Which path produced an answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Lane {
    /// Reflex answered at or above the threshold: no LLM call.
    Reflex,
    /// Reflex abstained or was below the threshold, and the fallback answered.
    Llm,
    /// Nobody answered (no fallback, or the fallback failed).
    Abstain,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionStats {
    #[ts(type = "number")]
    pub total: i64,
    #[ts(type = "number")]
    pub reflex: i64,
    #[ts(type = "number")]
    pub llm: i64,
    #[ts(type = "number")]
    pub abstain: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectInfo {
    pub project: Project,
    pub reflex: ReflexStatus,
    pub stats: DecisionStats,
    /// The folder the project's agents work in.
    pub workspace: String,
    /// The project folder (`[projects] path`, or the root of `roots`) its folder is inside; `None` when outside.
    pub root: Option<String>,
    /// Its folder is outside the configured project folder (made before it was set, or since moved): it keeps
    /// working, but a project could not be created there now.
    pub outside_roots: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Answer {
    pub question_id: String,
    /// The chosen option text (choice, score), a boolean (noul), or `null` when nobody answered.
    #[ts(type = "string | boolean | null")]
    pub answer: serde_json::Value,
    pub lane: Lane,
    pub confidence: Option<f64>,
    pub probabilities: Option<Vec<f64>>,
    /// What Reflex chose, even when it was below the threshold.
    #[ts(type = "string | boolean | null")]
    pub reflex_answer: serde_json::Value,
    pub llm_raw: Option<String>,
    pub error: Option<String>,
    #[ts(type = "number")]
    pub ms: u64,
    /// Why the answer took its lane, in one sentence (threshold comparison, shadow mode, Thai input…).
    pub reason: String,
    /// The decision log row of this answer, for feedback; 0 when it could not be recorded.
    #[ts(type = "number")]
    pub decision_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecideResponse {
    pub answers: Vec<Answer>,
    pub threshold: Option<f64>,
    /// Reflex's own explanation of its routing, when it answered.
    pub reflex_reason: Option<String>,
    pub reflex_error: Option<String>,
    #[ts(type = "number")]
    pub ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Decision {
    #[ts(type = "number")]
    pub id: i64,
    pub project: String,
    pub at: DateTime<Utc>,
    pub state: String,
    pub question: Question,
    #[ts(type = "string | boolean | null")]
    pub answer: serde_json::Value,
    pub lane: Lane,
    pub confidence: Option<f64>,
    #[ts(type = "string | boolean | null")]
    pub reflex_answer: serde_json::Value,
    #[ts(type = "number")]
    pub ms: i64,
    pub error: Option<String>,
    /// The user's verdict on the answer: `true` right, `false` wrong, `null` none yet.
    pub feedback: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoFile {
    pub path: String,
    #[ts(type = "number")]
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Commit {
    pub sha: String,
    pub author: String,
    pub at: DateTime<Utc>,
    pub subject: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RepoListing {
    pub files: Vec<RepoFile>,
    /// Newest first.
    pub commits: Vec<Commit>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FitCaseResult {
    #[ts(type = "string | boolean | null")]
    pub answer: serde_json::Value,
    pub confidence: f64,
    pub correct: bool,
}

/// Result of fitting a project's threshold from labeled cases.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FitResult {
    pub threshold: Option<f64>,
    pub rho: f64,
    #[ts(type = "number")]
    pub n: usize,
    #[ts(type = "number")]
    pub n_answered: usize,
    #[ts(type = "number")]
    pub n_correct: usize,
    /// Cases at or above the threshold, which Reflex would now answer alone.
    #[ts(type = "number")]
    pub n_pass: usize,
    pub pass_precision: Option<f64>,
    pub coverage: f64,
    pub applied: bool,
    pub cases: Vec<FitCaseResult>,
}

// ---- orchestrator decisions

/// A decision point of the orchestrator's task flow (`orchestrator.rs`), or a terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum DecisionNode {
    Intake,
    Ready,
    Pick,
    Launch,
    Watch,
    Verify,
    Audit,
    Land,
    Post,
    End,
    Retry,
    Review,
    Blocked,
    Failed,
    Done,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionPoint {
    pub id: DecisionNode,
    pub label: String,
    /// The question the orchestrator answers here.
    pub question: String,
    /// The condition as the code checks it, e.g. "every cfg.verify command exits 0".
    pub rule: String,
    /// Where it lives: function and config keys.
    pub source: String,
    pub terminal: bool,
}

/// One kind of `decision.<node>.<outcome>` event and the edge it takes (`from == to`: progress within a node).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionKind {
    pub kind: String,
    pub from: DecisionNode,
    pub to: DecisionNode,
    pub outcome: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionEdge {
    pub from: DecisionNode,
    pub to: DecisionNode,
    pub label: String,
}

/// The decision graph plus the config values its rules read, for the Decisions screen.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionModel {
    pub nodes: Vec<DecisionPoint>,
    pub edges: Vec<DecisionEdge>,
    pub kinds: Vec<DecisionKind>,
    #[ts(type = "number")]
    pub tick_secs: u64,
    #[ts(type = "number")]
    pub max_parallel: usize,
    #[ts(type = "number")]
    pub max_attempts: i64,
    #[ts(type = "number")]
    pub task_timeout_minutes: u64,
    pub provider_order: Vec<String>,
    pub verify: Vec<String>,
    pub post_done: Vec<String>,
    pub audit_enabled: bool,
    pub audit_required: bool,
    pub audit_chain: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionEdgeCount {
    pub from: DecisionNode,
    pub to: DecisionNode,
    #[ts(type = "number")]
    pub count: i64,
}

/// How often each edge was taken since `since` (`GET /api/decisions/edges`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionCounts {
    pub since: DateTime<Utc>,
    pub edges: Vec<DecisionEdgeCount>,
}

/// What one `POST /api/orchestrator/step` did: the decision events of that tick, oldest first.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct StepResult {
    pub decisions: Vec<Event>,
}

// ---- skills

/// Where a skill file lives: the workspace, the user's home, or an installed plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SkillSource {
    Project,
    User,
    Plugin,
}

/// A provider's context file in the workspace (CLAUDE.md, AGENTS.md, GEMINI.md…), read at session start.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ContextFile {
    pub provider: String,
    /// The file name, relative to the workspace.
    pub file: String,
    pub path: String,
    pub exists: bool,
    pub lines: u32,
    /// The first lines of prose (headings and imports skipped).
    pub summary: String,
    /// `@path` imports (Claude Code's syntax), e.g. `AGENTS.md` from a CLAUDE.md that is just `@AGENTS.md`.
    pub imports: Vec<String>,
    /// The other context files that import this one.
    pub imported_by: Vec<String>,
}

/// One provider that can load a skill, and how.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillLoad {
    pub provider: String,
    /// `native` (the provider's own skills folder), or e.g. `via AGENTS.md` when that context file points at the
    /// skill: the agent reads it only if it follows the pointer.
    pub how: String,
}

/// What the evidence on this machine says about a skill's use in this workspace.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillUsage {
    /// Times Claude loaded it (a `Skill` tool call, or `/name` typed) in this workspace's transcripts.
    pub loads: u32,
    pub loads_week: u32,
    /// Distinct agent sessions that loaded it.
    pub sessions: u32,
    pub last_used: Option<DateTime<Utc>>,
    /// MAP tasks during which it was loaded, from the session that ran each task.
    #[ts(type = "number[]")]
    pub task_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Skill {
    /// The absolute path of its SKILL.md; the key for the other skills endpoints.
    pub id: String,
    pub name: String,
    /// The name Claude's `Skill` tool and `/` menu use: `name`, `plugin:name`, or `anthropic-skills:name` for
    /// skills synced from claude.ai.
    pub invocation: String,
    pub source: SkillSource,
    pub plugin: Option<String>,
    /// Whose skills folder it is in: `claude` or `codex`.
    pub home: String,
    pub description: String,
    pub allowed_tools: Vec<String>,
    /// For display: relative to the workspace, or with `~` for the home directory.
    pub path: String,
    pub loads_in: Vec<SkillLoad>,
    /// False when a permission rule or `skillOverrides` entry in a Claude settings file turns it off here.
    pub enabled: bool,
    /// Whether the dashboard can switch it for this workspace (it writes `.claude/settings.local.json`).
    pub can_toggle: bool,
    /// Why it is off, or why it cannot be switched from here.
    pub toggle_note: Option<String>,
    /// False for `disable-model-invocation: true`: Claude runs it only when a person types `/name`.
    pub model_invocable: bool,
    /// False when nothing on this machine records its loads (Codex skills).
    pub usage_known: bool,
    pub usage: SkillUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillsOverview {
    /// The project slug; `None` is `workspace.dir`.
    pub project: Option<String>,
    pub workspace: String,
    pub contexts: Vec<ContextFile>,
    pub skills: Vec<Skill>,
    /// The transcript folder load counts were read from, when it exists.
    pub transcripts: Option<String>,
    pub transcript_files: u32,
    /// Loads that named no skill found here (bundled or since-removed skills).
    pub unmatched_loads: u32,
    pub scanned_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SkillFile {
    pub id: String,
    pub path: String,
    pub content: String,
    /// The file was longer than the limit and was cut.
    pub truncated: bool,
}

// ---- agents (custom sub-agents, backend/src/agents.rs)

/// The CLIs whose sub-agent files MAP reads and writes: Claude Code, Codex, and Antigravity (`agy`, the `gemini`
/// provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentProvider {
    Claude,
    Codex,
    Gemini,
}

/// A project's own agents (inside its folder) or the user's (in the home folder, for every project).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentScope {
    #[default]
    Project,
    User,
}

/// What an agent may do, mapped onto each CLI's own tools (Claude `Read`, agy `view_file`, Codex's shell switch…).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Capability {
    Read,
    Search,
    Edit,
    Shell,
    Web,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

/// The model per CLI; `None` inherits the session's. Claude: an alias (`opus`, `sonnet`, `haiku`, `fable`,
/// `inherit`) or a full id; Codex: any model name; agy: `inherit`, `flash` or `pro`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentModels {
    pub claude: Option<String>,
    pub codex: Option<String>,
    pub gemini: Option<String>,
}

/// One agent in MAP's own terms, rendered into each target CLI's file format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentDef {
    /// `[a-z0-9_-]`, 1 to 64 characters, starting with a letter or digit. The CLIs find agents by this name.
    pub name: String,
    /// When to use it: the parent agent reads this to decide on delegating.
    pub description: String,
    /// The system prompt (Claude, agy: the file's body; Codex: `developer_instructions`).
    pub instructions: String,
    /// `None`: every tool the CLI has (no `tools` key). Some: only these.
    pub tools: Option<Vec<Capability>>,
    #[serde(default)]
    pub models: AgentModels,
    pub effort: Option<Effort>,
    /// The CLIs to write a file for; saving removes the files of the others.
    pub targets: Vec<AgentProvider>,
}

/// Something a CLI's format cannot express exactly, or a field a save will not keep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentNote {
    pub provider: AgentProvider,
    pub text: String,
}

/// One agent file as the scanner read it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentFile {
    pub provider: AgentProvider,
    /// Relative to the workspace for project agents, `~/…` for user agents.
    pub path: String,
    pub model: Option<String>,
    /// The tool names as written in the file. Codex has no tool list: `shell_tool = false` when its shell is off.
    pub tools: Vec<String>,
    /// No `tools` key: the agent gets every tool.
    pub all_tools: bool,
    pub effort: Option<String>,
    /// Why the CLI may not load it (a missing description, unreadable TOML…).
    pub error: Option<String>,
}

/// One agent name in one scope, with the file each CLI has for it: the row of the compatibility matrix.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentRow {
    pub name: String,
    pub scope: AgentScope,
    pub description: String,
    /// In provider order; two files of one CLI with the same name are both listed (the CLI loads one).
    pub files: Vec<AgentFile>,
    /// What each present CLI's file loses against the others, and what a save from MAP would change.
    pub notes: Vec<AgentNote>,
}

/// A file that names no agent (no header, unreadable, or no `name`), so it fits no row.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentFileError {
    pub provider: AgentProvider,
    pub scope: AgentScope,
    pub path: String,
    pub error: String,
}

/// A folder the scanner reads, and where saving writes first.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentDir {
    pub provider: AgentProvider,
    pub scope: AgentScope,
    pub path: String,
    pub exists: bool,
}

/// A configured CLI MAP writes no agent files for, that loads another CLI's files itself (`[agents.also_loaded_by]`):
/// Cursor and Copilot read Claude Code's `.claude/agents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentReader {
    /// The provider's name in `map.toml` (`cursor`, `copilot`).
    pub provider: String,
    /// Whose files it loads.
    pub reads: AgentProvider,
    /// The scopes it loads them in (Copilot: project only; its own user folder is `~/.copilot/agents`).
    pub scopes: Vec<AgentScope>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentsOverview {
    /// The project slug; `None` is `workspace.dir`.
    pub project: Option<String>,
    pub workspace: String,
    pub rows: Vec<AgentRow>,
    pub errors: Vec<AgentFileError>,
    pub dirs: Vec<AgentDir>,
    /// Configured CLIs that load these files without a format of their own; the matrix shows them as extra columns.
    #[serde(default)]
    pub readers: Vec<AgentReader>,
    pub scanned_at: DateTime<Utc>,
}

/// An agent read back into MAP's terms, from its Claude file if it has one, else its agy file, else its Codex file.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentDetail {
    pub scope: AgentScope,
    pub def: AgentDef,
    /// The file `def` was read from.
    pub source: AgentProvider,
    pub files: Vec<AgentFile>,
    pub notes: Vec<AgentNote>,
}

/// One file a save writes (`content`) or removes (`content: None`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentChange {
    pub provider: AgentProvider,
    pub path: String,
    pub content: Option<String>,
    /// The file is there now: a write replaces it.
    pub exists: bool,
}

/// What saving a definition would do, or did.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentPlan {
    pub changes: Vec<AgentChange>,
    pub notes: Vec<AgentNote>,
    /// Why it cannot be saved as it is (preview only; a save fails with this message instead).
    pub error: Option<String>,
}

// ---- command centre

/// The task pipeline from `[orchestrator]` in `map.toml`, for the command centre's PIPELINE column and its
/// orchestrator card: which providers it tries, what proves a task done, and what runs after.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PipelineInfo {
    pub provider_order: Vec<String>,
    #[ts(type = "number")]
    pub max_attempts: i64,
    #[ts(type = "number")]
    pub task_timeout_minutes: u64,
    /// Shell commands run after the worker reports done; all must exit 0.
    pub verify: Vec<String>,
    pub audit_enabled: bool,
    pub audit_required: bool,
    /// Auditors tried in order; the first that gives a verdict decides.
    pub audit_chain: Vec<String>,
    /// Shell commands run after a task passes (e.g. a commit), as templates.
    pub post_done: Vec<String>,
}

// ---- project settings

/// The folder a project's agents work in. The platform works on folders on this machine in place: it never copies
/// or hosts a project's code. It may run git in that folder for you (clone, fetch, pull, push).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SourceKind {
    /// A new, empty folder created at `location`.
    #[default]
    Empty,
    /// Projects made before folders were used in place, whose code was cloned into the platform. No longer created.
    Git,
    /// A folder already on this machine, used as it is.
    Local,
    /// Only when creating: `remote` is cloned (`git clone`, not bare) into a new or empty folder at `location`, which
    /// is then stored as a `local` project with `remote` kept.
    Clone,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectSource {
    pub kind: SourceKind,
    /// The project's folder on this machine (`empty`, `local`, `clone`), or the URL a legacy `git` project was cloned
    /// from.
    #[serde(default)]
    pub location: Option<String>,
    /// For `clone`, the branch to check out (empty: the remote's default). The branch a legacy `git` project was
    /// cloned from. Unused for folders: their own checkout is what counts.
    #[serde(default)]
    pub branch: Option<String>,
    /// The repo the folder was cloned from, or last connected as `origin` from the dashboard. Informational: the
    /// folder's own `origin` is what git uses.
    #[serde(default)]
    pub remote: Option<String>,
}

/// One provider a project's tasks may use, in order of preference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectProvider {
    pub name: String,
    /// Used when the task does not pick a model; `None` is the provider's default.
    #[serde(default)]
    pub model: Option<String>,
    /// Most worker sessions of this provider running for the project at once; `None` is no limit beyond
    /// `orchestrator.max_parallel`.
    #[serde(default)]
    pub max_parallel: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectAudit {
    pub enabled: bool,
    /// The auditor; `None` uses `orchestrator.audit.chain`.
    #[serde(default)]
    pub provider: Option<String>,
}

/// Where a project's tasks run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum BranchMode {
    /// Every task works in the project folder, on whatever branch it has checked out. One task at a time is safe.
    #[default]
    Shared,
    /// Each task gets its own branch and git worktree, so several run at once; MAP lands each when it passes.
    Worktree,
}

/// What MAP does with a task's branch once it has passed verify and audit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LandMode {
    /// Rebase onto the base branch and fast-forward the base (one task at a time per project); a conflict goes back
    /// to the agent.
    #[default]
    Merge,
    /// Push the branch to `origin` and open a pull request (`gh`) or merge request (`glab`) against the base.
    PullRequest,
    /// Leave the branch for the owner to review and land from the dashboard.
    Keep,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ProjectBranches {
    pub mode: BranchMode,
    /// The branch tasks start from and land into; `None` is the branch checked out in the project folder.
    pub base: Option<String>,
    pub land: LandMode,
    /// Commands run in each new worktree before the agent starts (e.g. `bun install`).
    pub setup: Vec<String>,
    /// Untracked files or folders copied from the project folder into each new worktree (e.g. `.env`); never
    /// committed by MAP.
    pub copy: Vec<String>,
}

/// A project's own settings, stored in the platform database (not in `map.toml`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ProjectSettings {
    pub source: ProjectSource,
    /// Empty: `orchestrator.provider_order`.
    pub providers: Vec<ProjectProvider>,
    /// `None`: `orchestrator.verify`.
    pub verify: Option<Vec<String>>,
    /// `None`: `orchestrator.audit`.
    pub audit: Option<ProjectAudit>,
    /// The orchestrator starts none of the project's pending tasks while set.
    pub paused: bool,
    /// Decisions the orchestrator asks the project's Reflex first, never an LLM.
    pub reflex_gates: ReflexGates,
    /// Shared folder or a branch and worktree per task.
    pub branches: ProjectBranches,
    /// Where the project keeps its knowledge and the limits the Knowledge screen checks.
    pub knowledge: KnowledgeSettings,
    /// How many of the project's tasks run at once, and who clears tasks to share one folder.
    pub parallel: ProjectParallel,
    /// The project's own Thai words for Reflex (Thai → English, `-` for a word to skip), over the built-in glossary.
    pub thai_glossary: BTreeMap<String, String>,
    /// The always-on manager session the owner talks to about this project (`manager.rs`).
    pub manager: ManagerSettings,
    /// The project's plain environment variables (secrets live in MAP's secret store, never here). Set through
    /// `PUT /api/env`; see `env_vars.rs`.
    /// Optional in TypeScript: forms that send the settings back may leave it out (the server keeps it).
    #[ts(as = "Option<BTreeMap<String, String>>", optional)]
    pub env: BTreeMap<String, String>,
    /// The project's own hooks (`hooks.rs`), after the machine's; one with a machine hook's id replaces it. Changed
    /// through `PUT /api/hooks` only (optional here: `PATCH /api/projects/{slug}` keeps them).
    #[ts(as = "Option<Vec<Hook>>", optional)]
    pub hooks: Vec<Hook>,
    /// The project's own workflows (`workflows.rs`); one with a machine workflow's id overrides it. Changed only
    /// through `/api/workflows`, never by a settings PATCH (which may leave it out).
    #[ts(as = "Option<Vec<Workflow>>", optional)]
    pub workflows: Vec<Workflow>,
}

/// A project's manager: an interactive agent session (`<prefix>mgr-<slug>`) MAP keeps running while `enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ManagerSettings {
    pub enabled: bool,
    /// `None`: the project's first installed, signed-in provider (as the planner picks).
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// A project's parallel tasks. Changes apply at the next orchestrator tick, without a restart.
///
/// With `branches.mode = worktree` each task has its own branch, so up to `max` run at once. In the shared folder
/// (one branch) a second task only starts beside a running one when the parallel analyst (an agent) has cleared
/// both: it read the tasks and the code, and declared the paths each one touches as disjoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ProjectParallel {
    /// Most of the project's tasks at once; `None` is no project limit (only the machine-wide one).
    pub max: Option<u32>,
    /// In the shared folder, let the analyst clear independent tasks to run side by side. Off: one at a time.
    pub analyst: bool,
    /// The analyst's provider; `None` is the project's audit provider, else the first of the provider order.
    pub analyst_provider: Option<String>,
}

impl Default for ProjectParallel {
    fn default() -> Self {
        Self { max: None, analyst: true, analyst_provider: None }
    }
}

/// What the parallel analyst decided for a task: the paths it will touch, and whether it can share the folder with
/// other cleared tasks whose paths do not overlap.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskScope {
    /// Files or folders (`src/billing/`, `web/app/page.tsx`, `src/**`); `*` is the whole repo.
    pub paths: Vec<String>,
    /// Cleared to run beside other cleared tasks with disjoint paths.
    pub independent: bool,
    pub reason: String,
    /// The analyst (`analyst:<provider>`), or the agent that submitted it (`planner`, a session name).
    pub by: String,
    pub at: DateTime<Utc>,
}

/// Repeated orchestrator decisions answered by the project's Reflex (its `orchestration` corpus domain). An answer
/// below the project's threshold, an abstain or a Reflex that is not running leaves today's behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ReflexGates {
    /// Ask `intake` (ready, unclear, plan_first) before a task reaches any LLM, and hold a near-duplicate of a done
    /// task; with the parallel analyst on, also `shared_files`, whose confident "yes" runs the task alone.
    pub intake: bool,
    /// Ask `needs_audit` before the LLM audit; a confident "no" accepts the task on verification.
    pub audit: bool,
    /// Ask `tier` (light, standard, heavy) for a task without a pinned model and use `[orchestrator.tiers]`.
    pub tier: bool,
}

impl ReflexGates {
    pub fn any(&self) -> bool {
        self.intake || self.audit || self.tier
    }
}

/// One line of what creating a project did, for the dashboard to reveal in order.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CheckLine {
    pub level: Level,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectCreated {
    pub project: ProjectInfo,
    pub checks: Vec<CheckLine>,
}

/// `PATCH /api/projects/{slug}`: the project after the change, and what else the change did (a seeded corpus file).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectUpdated {
    pub project: ProjectInfo,
    pub checks: Vec<CheckLine>,
}

/// `[orchestrator.tiers.<provider>]`: the model the Reflex tier gate picks per task weight; `None` keeps the usual one.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderTiers {
    pub provider: String,
    pub light: Option<String>,
    pub standard: Option<String>,
    pub heavy: Option<String>,
}

/// The `map.toml` values a project falls back to when it does not set its own.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectDefaults {
    pub provider_order: Vec<String>,
    pub verify: Vec<String>,
    pub audit_enabled: bool,
    pub audit_chain: Vec<String>,
    #[ts(type = "number")]
    pub max_parallel: usize,
    pub post_done: Vec<String>,
    pub session_prefix: String,
    /// Providers with tier models for the Reflex tier gate.
    pub tiers: Vec<ProviderTiers>,
    /// `workspace.trust_folders`: a new project's folder is marked trusted for `trust_providers`.
    pub trust_folders: bool,
    /// Installed CLIs MAP can mark a folder trusted for (`claude`, `codex`).
    pub trust_providers: Vec<String>,
}

/// What the wizard found at a project's folder before creating the project.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceInspection {
    pub ok: bool,
    pub error: Option<String>,
    /// The branch checked out in the folder; `None` when it is not a git repo.
    pub default_branch: Option<String>,
    /// The folder's local branches.
    pub branches: Vec<String>,
    /// What is at the top of the folder (`Cargo.toml`, `package.json`, `CLAUDE.md`, …).
    pub files: Vec<String>,
    #[ts(type = "number | null")]
    pub commits: Option<u64>,
    /// For a git URL: what to do about an error (e.g. add MAP's public key to the host).
    #[serde(default)]
    pub hint: Option<String>,
    /// For a git URL: the suggested folder, `<project folder>/<repo name>`.
    #[serde(default)]
    pub folder: Option<String>,
    /// For a folder: the project folder (root) it is inside, by name.
    #[serde(default)]
    pub root: Option<String>,
}

/// How a provider signs in, from its spec in `map.toml`. Never carries a secret.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoginInfo {
    /// The command line run in the helper session; empty when the provider has no `login` command.
    pub command: String,
    /// Text typed into the helper session after it starts (e.g. `/login` for a TUI).
    pub send: Option<String>,
    /// The helper session's name.
    pub session: String,
    /// Files the CLI keeps its credentials in, `~` expanded.
    pub credentials: Vec<String>,
    /// The environment variable that can hold an API key instead.
    pub api_key_env: Option<String>,
    /// That variable is set in `workspace.env` or the server's environment.
    pub api_key_set: bool,
    /// A caution shown before starting (e.g. the login signs out the current account first).
    pub warning: Option<String>,
}

/// The helper login session as the sign-in modal sees it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoginState {
    pub session: Option<Session>,
    /// The last lines of the pane, ANSI stripped.
    pub text: String,
    pub url: Option<String>,
    /// A one-time code to enter in the browser.
    pub code: Option<String>,
    /// The CLI asks for an authorization code to be pasted back into it.
    pub wants_code: bool,
    /// The pane reports a successful sign-in.
    pub success: bool,
    pub signed_in: Option<bool>,
    /// The newest change to a credential file.
    pub credentials_updated_at: Option<DateTime<Utc>>,
    /// `starting`, `onboarding` (a first-run picker MAP answers), `ready`, `browser` (a URL or code to approve),
    /// `waiting_for_code`, `success`, `console` (the CLI waits for a person in its console) or `ended`.
    pub phase: String,
    pub hint: Option<String>,
}

// ---- usage

/// How a provider's plan usage was read on the last poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum UsageStatus {
    /// Not read yet (the first poll runs at startup).
    Pending,
    /// Read just now from the provider's account endpoint.
    Live,
    /// The newest snapshot the CLI wrote on its last run (Codex); `read_at` is when that was.
    Snapshot,
    /// The CLI is not signed in on this machine; windows are the last snapshot, if any.
    SignedOut,
    /// There is no local source for this provider's limits, so nothing is shown rather than a guess.
    NoReader,
    /// Turned off in `[usage.providers.<name>]`.
    Disabled,
    /// The reader failed; `error` says why. Windows are the last good reading, if there was one.
    Error,
}

/// One plan limit, e.g. Claude's 5-hour session or Codex's weekly limit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageWindow {
    /// Stable key within the provider (`session`, `weekly_all`, `primary`…); samples are stored under it.
    pub id: String,
    /// "Current session", "Weekly · all models".
    pub label: String,
    /// Compact label for the top-bar widget: "5h session", "7d all".
    pub short: String,
    /// "5-hour window", "7-day window".
    pub kind: String,
    /// Percent of the limit used, 0–100. `None` when unknown (e.g. the window reset after the last snapshot).
    pub used_pct: Option<f64>,
    /// Count-based limits (requests): used and cap. `None` for percent-only limits.
    pub used: Option<f64>,
    pub cap: Option<f64>,
    pub resets_at: Option<DateTime<Utc>>,
    #[ts(type = "number | null")]
    pub window_minutes: Option<u64>,
    /// Percentage points per hour over the last hour of stored readings; `None` until there are enough.
    pub rate_per_hour: Option<f64>,
    /// Estimated use at reset if the last hour's pace continues (may exceed 100).
    pub projected_pct: Option<f64>,
    /// Anything the reader wants to say about this number.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenTotals {
    /// Input tokens that were not cache reads.
    #[ts(type = "number")]
    pub input: u64,
    #[ts(type = "number")]
    pub output: u64,
    #[ts(type = "number")]
    pub cache_read: u64,
    #[ts(type = "number")]
    pub cache_write: u64,
}

impl TokenTotals {
    pub fn add(&mut self, other: &TokenTotals) {
        self.input += other.input;
        self.output += other.output;
        self.cache_read += other.cache_read;
        self.cache_write += other.cache_write;
    }

    /// What a by-project share counts: everything but cache reads, which are cheap and would drown the rest.
    pub fn weight(&self) -> u64 {
        self.input + self.output + self.cache_write
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ShareKind {
    /// A MAP project's workspace (or its other files under the data directory).
    Project,
    /// `workspace.dir`.
    Workspace,
    /// Anything else on this machine.
    Other,
}

/// Tokens a provider used in one place since `ProviderUsage::by_project_since`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectShare {
    pub kind: ShareKind,
    /// The project's slug for `project`; `None` otherwise.
    pub slug: Option<String>,
    pub name: String,
    /// Input + output + cache writes (cache reads excluded).
    #[ts(type = "number")]
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProviderUsage {
    pub provider: String,
    /// The account's plan, e.g. "Max 20x" or "Plus", when the source says.
    pub plan: Option<String>,
    /// Where the numbers come from, for "read via …".
    pub source: String,
    pub status: UsageStatus,
    pub error: Option<String>,
    /// When the numbers were measured (for a snapshot, when the CLI wrote it).
    pub read_at: Option<DateTime<Utc>>,
    pub windows: Vec<UsageWindow>,
    /// Since local midnight, from the CLI's own session logs; `None` when there is no log reader.
    pub tokens_today: Option<TokenTotals>,
    pub by_project: Vec<ProjectShare>,
    /// Start of the window `by_project` covers (the provider's shortest window).
    pub by_project_since: Option<DateTime<Utc>>,
    /// The orchestrator gives this provider no new tasks: a window is at or above `skip_pct`.
    pub skipped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UsageReport {
    /// When the last poll finished; `None` before the first.
    pub read_at: Option<DateTime<Utc>>,
    /// `usage.skip_pct`: no new tasks for a provider with a window at or above this.
    pub skip_pct: f64,
    #[ts(type = "number")]
    pub interval_secs: u64,
    pub providers: Vec<ProviderUsage>,
}

// ---- updates

/// One provider CLI's installed and newest version.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdateInfo {
    pub provider: String,
    /// From the provider's `version` command; `None` when it has none, the binary is missing, or it failed.
    pub installed: Option<String>,
    /// From the provider's `latest` source; `None` when it has none or the request failed.
    pub latest: Option<String>,
    /// `latest` is newer than `installed`.
    pub available: bool,
    /// When this provider was last checked; `None` before the first check.
    pub checked_at: Option<DateTime<Utc>>,
    /// Why the check failed or is incomplete (missing binary, timeout, unreachable source…).
    pub error: Option<String>,
    /// The `version` command as run (`{binary}` substituted); empty: none configured.
    pub version_command: Vec<String>,
    /// The `latest` URL as fetched (placeholders substituted); `None`: none configured.
    pub latest_url: Option<String>,
    /// The `update` command `POST /api/providers/{name}/update` runs; empty: none configured.
    pub update_command: Vec<String>,
    /// Set when an update from here cannot work, e.g. the CLI is installed read-only (the Docker deploy).
    pub update_blocked: Option<String>,
    /// The helper session `<session_prefix>update-<provider>` is running.
    pub updating: bool,
    /// That helper session's name.
    pub session: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct UpdatesReport {
    /// `updates.enabled`: the periodic check runs.
    pub enabled: bool,
    #[ts(type = "number")]
    pub interval_secs: u64,
    /// When the last full check finished; `None` before the first.
    pub checked_at: Option<DateTime<Utc>>,
    pub providers: Vec<UpdateInfo>,
}

// ---- reflex screen

/// Reflex's score of a calibration case the last time it was run.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CaseScore {
    #[ts(type = "string | boolean | null")]
    pub answer: serde_json::Value,
    pub confidence: f64,
    pub correct: bool,
    pub at: DateTime<Utc>,
}

/// A labeled calibration case: a message and the correct answer to one of the project's saved questions.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReflexCase {
    #[ts(type = "number")]
    pub id: i64,
    pub question_id: String,
    pub text: String,
    #[ts(type = "string | boolean")]
    pub expected: serde_json::Value,
    pub created_at: DateTime<Utc>,
    /// `None` until Reflex has scored it (Reflex was down, or the question is gone).
    pub score: Option<CaseScore>,
}

/// One lane over a summary window.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneSummary {
    #[ts(type = "number")]
    pub count: i64,
    /// Average time to answer in milliseconds; 0 without decisions.
    pub avg_ms: f64,
}

/// Decisions per lane in one hour.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HourBucket {
    pub start: DateTime<Utc>,
    #[ts(type = "number")]
    pub reflex: i64,
    #[ts(type = "number")]
    pub llm: i64,
    #[ts(type = "number")]
    pub abstain: i64,
}

/// A project's decisions over the last `hours`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionSummary {
    #[ts(type = "number")]
    pub hours: i64,
    pub since: DateTime<Utc>,
    #[ts(type = "number")]
    pub total: i64,
    pub reflex: LaneSummary,
    pub llm: LaneSummary,
    pub abstain: LaneSummary,
    #[ts(type = "number")]
    pub feedback_right: i64,
    #[ts(type = "number")]
    pub feedback_wrong: i64,
    /// One per hour, oldest first; the last one holds the current hour.
    pub buckets: Vec<HourBucket>,
}

/// Result of feedback on a logged decision.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct DecisionFeedback {
    pub decision: Decision,
    /// The calibration case the feedback added, if any.
    pub case: Option<ReflexCase>,
    /// Whether Reflex's own answer was right, as reported to its calibrator; `None` when Reflex had no answer.
    pub reflex_outcome: Option<bool>,
    /// Reflex's calibrator refitted on this report (it refits every 64 reports).
    pub refit: bool,
    pub reflex_error: Option<String>,
}

/// Result of moving corpus domains into or out of the archive folder.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ArchiveResult {
    pub moved: Vec<String>,
    #[ts(type = "number")]
    pub files: usize,
    pub commit: String,
    pub reflex: ReflexStatus,
}

// ---- Reflex savings: LLM tokens Reflex saved by answering locally (savings.rs)

/// Where a token figure comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TokenMethod {
    /// Every figure rests on usage the provider reported (the API fallback's `usage`).
    Measured,
    /// Every figure is an estimate: about 4 characters per token.
    Estimated,
    /// Some of each.
    Mixed,
}

/// Decisions and tokens of one project (or of all, in `totals`) over the report's window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SavingsRow {
    /// Empty in `totals` when it covers every project.
    pub slug: String,
    pub name: String,
    #[ts(type = "number")]
    pub decisions: i64,
    #[ts(type = "number")]
    pub reflex: i64,
    #[ts(type = "number")]
    pub llm: i64,
    #[ts(type = "number")]
    pub abstain: i64,
    /// Decisions whose state had Thai text, so Reflex was not asked: they could not save anything.
    #[ts(type = "number")]
    pub skipped_thai: i64,
    /// Every reflex-lane answer is a fallback call not made.
    #[ts(type = "number")]
    pub llm_calls_avoided: i64,
    /// What the reflex-lane answers would have cost the fallback.
    #[ts(type = "number")]
    pub tokens_saved: i64,
    /// Input plus output tokens of the llm-lane answers.
    #[ts(type = "number")]
    pub tokens_spent: i64,
    /// `100 × saved / (saved + spent)`; `null` when both are 0.
    pub saved_pct: Option<f64>,
    pub method: TokenMethod,
    /// Mean time to answer per lane, in milliseconds; `null` without decisions in that lane.
    pub avg_ms_reflex: Option<f64>,
    pub avg_ms_llm: Option<f64>,
    /// `reflex × max(0, avg_ms_llm − avg_ms_reflex)`; `null` without llm-lane decisions to compare with.
    #[ts(type = "number | null")]
    pub time_saved_ms: Option<i64>,
}

/// One UTC day of the report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SavingsDay {
    /// `YYYY-MM-DD`, in UTC.
    pub day: String,
    #[ts(type = "number")]
    pub saved: i64,
    #[ts(type = "number")]
    pub spent: i64,
    #[ts(type = "number")]
    pub reflex: i64,
    #[ts(type = "number")]
    pub llm: i64,
}

/// `GET /api/reflex/savings`: tokens Reflex saved against what the fallbacks spent, since `since`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SavingsReport {
    pub since: DateTime<Utc>,
    #[ts(type = "number")]
    pub hours: i64,
    /// How the figures are counted, in a few sentences.
    pub method_note: String,
    pub totals: SavingsRow,
    /// Every project in the report (one when it is filtered), in creation order.
    pub projects: Vec<SavingsRow>,
    /// One per UTC day the window touches, oldest first.
    pub days: Vec<SavingsDay>,
    /// `[reflex] fallback_overhead_tokens` from `map.toml`: tokens added per call of that fallback.
    #[ts(type = "Record<string, number>")]
    pub overhead_tokens: std::collections::BTreeMap<String, u64>,
}

// ---- templates: a development process laid into a project's folder (templates.rs)

/// Where a template comes from: built into the binary, or a folder in `templates.dirs` of `map.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TemplateSource {
    Builtin,
    User,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TemplateFile {
    /// Relative to the project's folder.
    pub path: String,
    /// Names of the `MAP:FILL` placeholders in the file, in order.
    pub placeholders: Vec<String>,
    /// Merged line by line into an existing file (`.gitignore`) instead of being skipped.
    pub append: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TemplateInfo {
    pub name: String,
    pub title: String,
    pub description: String,
    pub source: TemplateSource,
    /// The template's folder, for user templates.
    pub dir: Option<String>,
    /// Files whose placeholders the init agent must complete.
    pub fill: Vec<String>,
    /// Suggested verify commands for a project that uses the template.
    pub verify: Vec<String>,
    /// Every file the template lays down, rendered agents included, sorted by path.
    pub files: Vec<TemplateFile>,
}

/// What laying a template down does to one file. Existing files are never overwritten.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlanAction {
    Create,
    /// The file exists (or, for an append file, already has every line): left alone.
    Skip,
    /// The file exists; only its missing lines are added at the end.
    Append,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlannedFile {
    pub path: String,
    pub action: PlanAction,
    /// Placeholders the init agent must fill: the template's for a new file, those still in an existing one.
    pub placeholders: Vec<String>,
    /// For `append`: the lines that will be added.
    pub lines: Vec<String>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InitPlan {
    pub template: String,
    /// The project's folder.
    pub workspace: String,
    /// The folder held nothing (besides `.git`) before the template.
    pub folder_empty: bool,
    pub files: Vec<PlannedFile>,
}

/// The result of `POST /api/projects/{slug}/init`: what was written, and the init agent's task.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct InitStarted {
    pub plan: InitPlan,
    pub checks: Vec<CheckLine>,
    pub task: Task,
}

// ---- git access: MAP's SSH key, git identity and gitconfig (git_access.rs), and a project folder's sync state

/// MAP's own SSH key (`<data_dir>/git/id_ed25519`). Only the public half ever leaves the server.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitKey {
    /// The `.pub` line, `ssh-ed25519 AAAA… map@host`: what you paste into GitHub or GitLab.
    pub public_key: String,
    /// `SHA256:…`, as GitHub and GitLab show it next to a key.
    pub fingerprint: String,
    pub created_at: DateTime<Utc>,
    pub comment: String,
}

/// The author name and email MAP's gitconfig sets for agents and MAP's own commits in project folders. Not secret.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitIdentity {
    pub name: String,
    pub email: String,
}

/// One host key in MAP's `known_hosts`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnownHost {
    /// `github.com`, or `[host]:port`.
    pub host: String,
    /// `ED25519`, `ECDSA`, `RSA`.
    pub key_type: String,
    pub fingerprint: String,
    /// Shipped with MAP from the host's published keys; otherwise recorded on first use.
    pub pinned: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitAccess {
    pub key: Option<GitKey>,
    pub identity: GitIdentity,
    pub known_hosts: Vec<KnownHost>,
    /// MAP's gitconfig, exported as `GIT_CONFIG_GLOBAL` to agents and used for MAP's own git commands.
    pub gitconfig_path: String,
    /// The user's own global git config files it includes (they win over MAP's defaults).
    pub includes: Vec<String>,
    /// `[projects] path`, the project folder: where "Clone from git" puts new folders by default.
    pub projects_root: String,
    /// `ssh-keygen` and `ssh` were found on PATH.
    pub ssh_available: bool,
    /// Which SSH setup git uses (`<data_dir>/git/settings.json`).
    pub settings: GitSettings,
    /// This machine's own SSH setup, for the "system" method: public keys only, never a private one.
    pub system_ssh: SystemSsh,
}

/// Which SSH setup agents' and MAP's git commands use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SshMethod {
    /// MAP's own key in `<data_dir>/git/` with MAP's `known_hosts` (the gitconfig sets `core.sshCommand`).
    #[default]
    MapKey,
    /// The machine's own ssh: `~/.ssh` (keys, config, known_hosts) and ssh-agent; the gitconfig sets no `sshCommand`.
    System,
}

/// `<data_dir>/git/settings.json`, set with `PUT /api/git/settings`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitSettings {
    #[serde(default)]
    pub ssh: SshMethod,
}

/// A public key in `~/.ssh` (a `*.pub` file; private keys are never read).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SshPublicKey {
    /// The file name, `id_ed25519.pub`.
    pub file: String,
    pub fingerprint: String,
    /// `ED25519`, `RSA`…
    pub key_type: String,
    pub comment: String,
}

/// This machine's SSH setup as the "system" method would use it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SystemSsh {
    /// `~/.ssh` of the user MAP runs as.
    pub dir: String,
    pub dir_exists: bool,
    /// `~/.ssh/config` exists.
    pub config_exists: bool,
    /// `~/.ssh/known_hosts` exists.
    pub known_hosts_exists: bool,
    pub public_keys: Vec<SshPublicKey>,
    /// `SSH_AUTH_SOCK` is set for MAP (an ssh-agent it may reach).
    pub agent_socket: bool,
    /// MAP runs in a container (`/.dockerenv`), where `~/.ssh` is not mounted unless docker-compose.yml says so.
    pub in_container: bool,
}

/// `POST /api/git/test`: `ssh -T` to a host with MAP's key.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitTestResult {
    /// The host greeted the key (GitHub: "successfully authenticated", GitLab: "Welcome to GitLab").
    pub ok: bool,
    pub host: String,
    pub command: String,
    pub output: String,
    /// The host's keys as now recorded in MAP's `known_hosts`.
    pub host_keys: Vec<KnownHost>,
}

/// A project folder's git state (`GET /api/projects/{slug}/git`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitStatus {
    /// The folder itself is the top of a git work tree (a repo further up does not count).
    pub is_repo: bool,
    /// The branch checked out; `None` when detached or not a repo.
    pub branch: Option<String>,
    /// `origin/main`: the branch's upstream.
    pub upstream: Option<String>,
    /// `origin`'s URL, with any password in it masked.
    pub remote_url: Option<String>,
    /// Commits on the branch not on its upstream; `None` without an upstream.
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
    /// Changed, staged and untracked files.
    pub dirty: u32,
    /// When `origin` was last fetched (`FETCH_HEAD`).
    pub last_fetch: Option<DateTime<Utc>>,
    pub last_commit: Option<Commit>,
    /// Why pull, push and connecting a remote are refused right now (a task of the project is running).
    pub busy: Option<String>,
}

/// What a fetch, pull, push or remote change did.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitOpResult {
    pub ok: bool,
    /// The git commands run, one per line.
    pub command: String,
    pub output: String,
    pub status: GitStatus,
}

// ---- the Settings screen (`GET /api/settings`): read-only facts about the server, the agent CLIs' binaries and the
// git host CLIs. Never a secret: whether `api_token` or a token variable is set, never its value.

/// How the installer set MAP up (`[server] mode`, written by `deploy/install.sh --mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum InstallMode {
    /// Everything under one MAP folder: its own agent sign-ins, its own tmux server, projects in its workspace only.
    Isolated,
    /// One with the machine (a VM provisioned for MAP): the user's own CLI sign-ins and tmux server, the usual
    /// `~/.config` / `~/.local` places, projects anywhere in the home folder.
    System,
}

/// How MAP was started, as far as it can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RunMode {
    /// In a container (`/.dockerenv`): the Docker Compose deploy.
    Docker,
    /// A systemd service (`INVOCATION_ID` is set), e.g. `deploy/install.sh`'s `map.service`.
    Systemd,
    /// A launchd agent (`XPC_SERVICE_NAME` is set), e.g. `com.multi-agent-platform.map`.
    Launchd,
    /// Started by hand (`map serve`).
    Manual,
}

/// The server's settings from `map.toml`, read only.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ServerSettings {
    pub version: String,
    /// The `map.toml` this server read; `None` when it was given no file (tests).
    pub config_path: Option<String>,
    pub data_dir: String,
    pub bind: String,
    pub allowed_hosts: Vec<String>,
    /// `server.api_token` is set. Its value is never returned.
    pub api_token_set: bool,
    /// The folder holding `map.toml` (with an installer's layout, also the data dir's parent).
    pub config_dir: Option<String>,
    /// The project folder (`[projects] path`; with `roots`, the default one): projects are its sub-folders.
    pub projects_root: String,
    /// Every folder projects may be created in: the project folder, or each of `[projects] roots`.
    pub project_roots: Vec<FsRoot>,
    /// `workspace.dir`, the default workspace.
    pub workspace_dir: String,
    pub session_prefix: String,
    pub static_dir: String,
    /// `static_dir` holds the dashboard (`index.html`).
    pub static_dir_ok: bool,
    pub reflex_binary: String,
    pub reflex_version: Option<String>,
    pub run_mode: RunMode,
    /// `[server] mode`; `None` for a config written by hand or by an older installer.
    pub install_mode: Option<InstallMode>,
    /// The command that restarts MAP after editing `map.toml` for this run mode (by hand: starts it again).
    pub restart_hint: String,
    /// `workspace.tmux_socket`; `None`: tmux's default server.
    pub tmux_socket: Option<String>,
    /// How to attach to a session by hand, with `<session>` for its name (`tmux -L map attach -t '=<session>'`).
    pub attach_command: String,
}

/// `[agents_home] mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum AgentsHomeMode {
    /// The CLIs' usual folders: the same accounts and history as the owner's own `claude` / `codex` / `agy`.
    Shared,
    /// Each provider has its own folder under `dir`: MAP's agents sign in with their own accounts and keep their
    /// own history; the owner's CLIs in a terminal are unaffected.
    Isolated,
}

/// Whose logins and history MAP's agents use.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentsHome {
    pub mode: AgentsHomeMode,
    /// `agents_home.dir`, the parent of the per-provider folders (used in isolated mode only).
    pub dir: String,
}

/// An environment variable's state: set or not, and where. Never its value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnvVarState {
    pub name: String,
    /// What it is for, e.g. "token for github.com".
    pub purpose: String,
    /// Set (non-empty) in `[workspace.env]` of `map.toml`.
    pub in_workspace_env: bool,
    /// Set (non-empty) in MAP's own environment (the service's or container's), which agents inherit.
    pub in_server_env: bool,
}

/// `gh` or `glab`: on the agents' PATH, and the variables it signs in with.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GitCliTool {
    /// `gh`, `glab`.
    pub name: String,
    /// `GitHub CLI`, `GitLab CLI`.
    pub title: String,
    /// Where it is on the agents' PATH (`workspace.env` PATH, else MAP's); `None`: not found.
    pub path: Option<String>,
    pub vars: Vec<EnvVarState>,
}

/// An agent CLI's binary as configured, and where it is found.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentBinary {
    pub provider: String,
    /// `providers.<name>.binary`.
    pub binary: String,
    /// Where it resolves on the agents' PATH; `None`: not found.
    pub path: Option<String>,
    /// The CLI's state folder (sign-in, settings, history) as MAP's agents see it; `None`: not known.
    pub home: Option<String>,
    /// What MAP exports to relocate it in isolated mode (`CLAUDE_CONFIG_DIR=/…/agent-home/claude`), or in both modes
    /// for a provider with a `shared_home` (openrouter); `None` otherwise, or when the CLI has no such variable.
    pub home_env: Option<String>,
}

/// `GET /api/settings`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SettingsInfo {
    pub server: ServerSettings,
    pub git_cli: Vec<GitCliTool>,
    pub agents: Vec<AgentBinary>,
    pub agents_home: AgentsHome,
}

// ---- plans: a requirement broken into tasks by the planner agent, approved by the owner (plans.rs)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PlanStatus {
    /// Waiting for the owner: approve, reject or edit it. Nothing runs yet.
    Draft,
    /// Its tasks were created (`PlanTask::task_id`).
    Approved,
    Rejected,
    /// A newer revision replaced it before it was decided.
    Superseded,
}

impl PlanStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStatus::Draft => "draft",
            PlanStatus::Approved => "approved",
            PlanStatus::Rejected => "rejected",
            PlanStatus::Superseded => "superseded",
        }
    }
}

/// One task of a plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanTask {
    /// The planner's own id ("t1"), unique in the plan: 1–32 letters, digits, `_` or `-`.
    pub key: String,
    pub title: String,
    /// The worker's whole prompt: goal, scope and non-goals, files, acceptance criteria, how to verify.
    pub prompt: String,
    /// Keys of tasks of the same plan that must be done first.
    pub depends_on: Vec<String>,
    /// Pin the task to a provider (and model); `None` lets the orchestrator pick.
    pub provider: Option<String>,
    pub model: Option<String>,
    /// The task approving the plan created; `None` before.
    #[ts(type = "number | null")]
    pub task_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Plan {
    #[ts(type = "number")]
    pub id: i64,
    /// The project's slug; `None` is the default workspace.
    pub project: Option<String>,
    pub title: String,
    /// The requirement as the planner restated it, and anything the owner should know before approving.
    pub summary: String,
    pub status: PlanStatus,
    /// 1 for a first draft; a revision of another plan is that one's revision + 1.
    #[ts(type = "number")]
    pub revision: i64,
    /// The plan this one revises.
    #[ts(type = "number | null")]
    pub replaces: Option<i64>,
    /// The planner session that submitted it.
    pub session: Option<String>,
    /// Why the owner rejected it.
    pub reason: Option<String>,
    /// In the order submitted.
    pub tasks: Vec<PlanTask>,
    pub created_at: DateTime<Utc>,
    /// When it was approved, rejected or superseded.
    pub decided_at: Option<DateTime<Utc>>,
}

/// A task as the planner submits it, or as the owner edits it (`PATCH /api/plans/{id}`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanTaskInput {
    pub key: String,
    pub title: String,
    pub prompt: String,
    #[serde(default)]
    #[ts(optional)]
    pub depends_on: Option<Vec<String>>,
    #[serde(default)]
    #[ts(optional)]
    pub provider: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub model: Option<String>,
}

/// `POST /api/projects/{slug}/plans` (or `/api/workspace/plans`): a new draft.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanSubmit {
    pub title: String,
    #[serde(default)]
    #[ts(optional)]
    pub summary: Option<String>,
    pub tasks: Vec<PlanTaskInput>,
    /// A plan of the same project this one revises: a draft becomes `superseded`.
    #[serde(default)]
    #[ts(optional, type = "number")]
    pub replaces: Option<i64>,
    /// The submitting session (`$MAP_SESSION`); defaults to the project's planner session.
    #[serde(default)]
    #[ts(optional)]
    pub session: Option<String>,
}

/// `PATCH /api/plans/{id}`: change a draft before approving it. `tasks` replaces the whole list.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanEdit {
    #[serde(default)]
    #[ts(optional)]
    pub title: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub summary: Option<String>,
    #[serde(default)]
    #[ts(optional)]
    pub tasks: Option<Vec<PlanTaskInput>>,
}

/// `POST /api/plans/{id}/approve`: the plan, now approved, and the tasks it created in dependency order.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanApproved {
    pub plan: Plan,
    pub tasks: Vec<Task>,
}

/// The planner agent of a project (or of the default workspace): its session and where its instructions come from.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlannerInfo {
    /// `<session_prefix>plan-<slug>`, or `<session_prefix>plan` for the default workspace.
    pub session_name: String,
    /// The session, live or exited, when there is one.
    pub session: Option<Session>,
    /// `built-in` or the path of `templates.planner_agent`.
    pub instructions_source: String,
    /// The provider "Talk to the planner" uses when none is picked; `None` when none is available.
    pub default_provider: Option<String>,
}

/// `GET /api/planner/instructions`: the planner's instructions as MAP sends them, read-only.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlannerInstructions {
    /// `built-in` or the path of `templates.planner_agent`.
    pub source: String,
    pub content: String,
}

// ---- project folder: where project folders may be (`[projects] path`, or `roots`), and the folder picker (roots.rs)

/// A folder projects may be created in (`[projects] path`, or one of `roots`). `GET /api/fs/roots`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsRoot {
    pub name: String,
    /// As MAP sees it (inside the container, in Docker), `~` expanded.
    pub path: String,
    /// Display only: the same folder on the host, when the container mounts it at another path.
    pub host_path: Option<String>,
    /// The folder exists on this machine (a missing one is a warning at start, not an error).
    pub exists: bool,
    /// The folder itself may be a project's folder; otherwise only its sub-folders may.
    pub allow_root_itself: bool,
    /// "Clone from git" suggests `<path>/<repo name>` here.
    pub is_default: bool,
}

/// What a listed entry is. Only folders are listed; a symlink only when it points to a folder inside the same root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FsEntryKind {
    Dir,
    /// A symlink to a folder inside the same root.
    Symlink,
}

/// A sub-folder in a `GET /api/fs/browse` listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsEntry {
    pub name: String,
    pub kind: FsEntryKind,
    /// It has a `.git` (a repo or a worktree).
    pub is_git: bool,
    /// The slug of the MAP project whose folder this is.
    pub is_project: Option<String>,
    /// The slug of a MAP project whose folder is inside this one (projects cannot nest, so it cannot be picked).
    pub contains_project: Option<String>,
    /// It has sub-folders (or too many entries to look through).
    pub has_children: bool,
    /// Its name starts with `.`.
    pub hidden: bool,
}

/// `GET /api/fs/browse?root=&path=&hidden=`: the sub-folders of one folder inside a root. Never file contents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FsListing {
    pub root: String,
    /// The listed folder relative to the root (`""` is the root), with symlinks resolved.
    pub path: String,
    /// The parent's relative path; `None` at the root.
    pub parent: Option<String>,
    /// The listed folder as an absolute path on this machine (what a project stores).
    pub abs_path: String,
    /// The same folder on the host, when the root has a `host_path`.
    pub host_path: Option<String>,
    /// The listed folder is the root itself.
    pub is_root: bool,
    /// The slug of a MAP project whose folder is the listed folder or contains it.
    pub in_project: Option<String>,
    /// Sorted by name, case-insensitively; dot-folders only with `hidden=true`.
    pub entries: Vec<FsEntry>,
    /// More folders than the cap were found; only the first are listed.
    pub truncated: bool,
    /// Files (and other non-folders) in the listed folder: a count only.
    #[ts(type = "number")]
    pub files: u64,
    /// Dot-folders left out because `hidden` was not set.
    #[ts(type = "number")]
    pub hidden_dirs: u64,
}

/// `POST /api/projects/{slug}/worktree-estimate` (`{parallel?, copy?, setup?}`): what running N tasks of the project side by side in
/// worktrees would cost this machine, and what it trades for the speed. Rough by design: sizes are measured, RAM per
/// agent is measured from running CLIs when there are any, build memory is a rule of thumb.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorktreeEstimate {
    pub machine: MachineSpec,
    /// Tasks run at once (the request's `parallel`, else `orchestrator.max_parallel`).
    pub parallel: u32,
    /// Bytes of the files git checks out into each worktree (`git ls-tree -r -l HEAD`).
    #[ts(type = "number")]
    pub checkout_bytes: u64,
    pub tracked_files: u32,
    /// Ignored folders in the project folder that setup rebuilds per worktree (node_modules, target, .venv…), and
    /// the `copy` list.
    pub extra: Vec<SizedPath>,
    /// Disk one worktree takes: checkout plus `extra`.
    #[ts(type = "number")]
    pub per_task_disk: u64,
    /// Memory of one agent CLI, per provider the project uses.
    pub agents: Vec<AgentMemory>,
    /// Memory one task's verify commands may take while they build and test (rule of thumb from the project's files).
    #[ts(type = "number")]
    pub build_memory: u64,
    pub build_note: String,
    /// One task: the largest agent plus `build_memory`.
    #[ts(type = "number")]
    pub per_task_memory: u64,
    #[ts(type = "number")]
    pub total_disk: u64,
    #[ts(type = "number")]
    pub total_memory: u64,
    /// CPU threads the parallel verify steps want at their peak (one build per task).
    pub peak_threads: u32,
    /// Most tasks this machine should run at once, and what limits it.
    pub recommended_parallel: u32,
    pub limited_by: String,
    /// The project's pending and running tasks: how many waves they take one at a time and with `parallel` workers.
    pub open_tasks: u32,
    pub waves_serial: u32,
    pub waves_parallel: u32,
    /// What the speed costs, most serious first.
    pub drawbacks: Vec<Drawback>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct MachineSpec {
    pub cpus: u32,
    #[ts(type = "number")]
    pub memory_total: u64,
    /// `None` where MAP cannot read it.
    #[ts(type = "number | null")]
    pub memory_available: Option<u64>,
    /// Free space where worktrees are made (`<data_dir>/projects`).
    #[ts(type = "number | null")]
    pub disk_free: Option<u64>,
    pub disk_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SizedPath {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// From the `copy` list (copied) rather than rebuilt by setup.
    pub copied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentMemory {
    pub provider: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Measured from running processes (how many) rather than assumed.
    pub measured: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Drawback {
    pub level: Level,
    pub title: String,
    pub detail: String,
}

// ---- knowledge (knowledge.rs): the project's docs, their health, search, and what MAP remembers of past tasks

/// Where a project keeps its knowledge (paths relative to its folder) and the limits the health checks use. The
/// defaults follow the team-process layout: knowledge cards under `.docs/knowledge/` with an `index.md`, each card
/// listing the files it summarises in a `sources:` frontmatter list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct KnowledgeSettings {
    /// Folders indexed for search (Markdown and text files).
    pub dirs: Vec<String>,
    /// The cards' index; the folder it is in holds the cards.
    pub index: String,
    /// Rule files loaded into every agent request, with a byte budget.
    pub rulebooks: Vec<String>,
    pub card_line_cap: u32,
    pub card_line_warn: u32,
    #[ts(type = "number")]
    pub rulebook_byte_cap: u64,
    #[ts(type = "number")]
    pub rulebook_byte_warn: u64,
    /// Agent scratch folders, checked for size and for copies of the repo inside them.
    pub scratch: Vec<String>,
    #[ts(type = "number")]
    pub scratch_byte_warn: u64,
}

impl Default for KnowledgeSettings {
    fn default() -> Self {
        Self {
            dirs: vec![".docs".into(), "docs".into(), "tasks".into()],
            index: ".docs/knowledge/index.md".into(),
            rulebooks: vec!["CLAUDE.md".into(), "AGENTS.md".into()],
            card_line_cap: 200,
            card_line_warn: 170,
            rulebook_byte_cap: 60_000,
            rulebook_byte_warn: 52_000,
            scratch: vec![".scratch".into()],
            scratch_byte_warn: 2 * 1024 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum KnowledgeKind {
    /// A knowledge card (the index's folder).
    Card,
    /// Requirements, specs, client material (`requirement`, `customer`, `spec` in the path).
    Requirement,
    /// A task sheet under `tasks/`.
    Task,
    /// CLAUDE.md, AGENTS.md and the like.
    Rulebook,
    /// Any other document.
    Doc,
    /// What MAP remembers itself: a finished task's outcome, an approved plan.
    Memory,
}

/// One indexed document. `path` is relative to the project folder, or `map://tasks/<id>` / `map://plans/<id>` for
/// MAP's own memory.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeDoc {
    pub path: String,
    pub kind: KnowledgeKind,
    pub title: String,
    pub lines: u32,
    #[ts(type = "number")]
    pub bytes: u64,
    pub modified: Option<DateTime<Utc>>,
    /// A card's `sources:` frontmatter.
    pub sources: Vec<String>,
    /// For a card: the index mentions it.
    pub in_index: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum KnowledgeCheck {
    /// No index file where `knowledge.index` says.
    NoIndex,
    /// A source changed in a commit after the card's last commit.
    Stale,
    /// A card lists a source that does not exist.
    MissingSource,
    /// Over `card_line_cap` lines.
    OverCap,
    /// Over `card_line_warn` lines.
    NearCap,
    /// A card the index does not mention.
    NotInIndex,
    /// A relative Markdown link to a file that does not exist.
    BrokenLink,
    /// A rulebook over (error) or near (warning) its byte budget.
    RulebookSize,
    /// A scratch folder over `scratch_byte_warn`.
    ScratchSize,
    /// A copy of the repo (a `.git`, or the project's own docs) inside a scratch folder: grep finds stale copies.
    NestedCopy,
}

/// One finding of the health checks, with a ready task prompt that fixes it.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeIssue {
    pub level: Level,
    pub check: KnowledgeCheck,
    pub path: String,
    pub detail: String,
    /// Title and prompt for a task that fixes it; `None` when a person must decide.
    pub fix_title: Option<String>,
    pub fix_prompt: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RulebookSize {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub cap: u64,
    #[ts(type = "number")]
    pub warn: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScratchInfo {
    pub path: String,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Repo copies found inside it.
    pub copies: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KindCount {
    pub kind: KnowledgeKind,
    pub count: u32,
}

/// A search someone ran: an agent with its project token, or the owner from the dashboard.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeQuery {
    pub at: DateTime<Utc>,
    pub query: String,
    pub hits: u32,
    /// `agent` or `owner`.
    pub by: String,
}

/// `GET /api/projects/{slug}/knowledge`: everything the Knowledge screen shows for one project.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeOverview {
    pub slug: String,
    /// The project folder.
    pub root: String,
    pub settings: KnowledgeSettings,
    /// The index file, when it exists.
    pub index_path: Option<String>,
    pub indexed_at: Option<DateTime<Utc>>,
    pub counts: Vec<KindCount>,
    pub docs: Vec<KnowledgeDoc>,
    /// Most serious first.
    pub issues: Vec<KnowledgeIssue>,
    pub rulebooks: Vec<RulebookSize>,
    pub scratch: Vec<ScratchInfo>,
    /// Newest first.
    pub recent_queries: Vec<KnowledgeQuery>,
    /// Whether the project should set up its knowledge now, and what accepting does.
    pub suggestion: KnowledgeSuggestion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SuggestVerdict {
    /// No knowledge yet and the project is big or busy enough to need it: accept to queue the setup task.
    Recommended,
    /// No knowledge yet, but the project is still small: cards would cost more than they save.
    NotYet,
    /// The setup task is queued or running.
    InProgress,
    /// The index exists: knowledge is set up and in use.
    InUse,
    /// The owner said not now.
    Dismissed,
}

/// The facts the suggestion weighs.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeStats {
    /// Source files outside build, dependency and scratch folders (up to the scan limit).
    pub source_files: u32,
    #[ts(type = "number")]
    pub source_lines: u64,
    pub done_tasks: u32,
    /// Markdown and text documents already indexed (requirements, docs, task sheets).
    pub docs: u32,
    pub cards: u32,
    /// The largest rulebook (CLAUDE.md, AGENTS.md), in bytes.
    #[ts(type = "number")]
    pub rulebook_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeSuggestion {
    pub verdict: SuggestVerdict,
    /// Why, one line each.
    pub reasons: Vec<String>,
    pub stats: KnowledgeStats,
    /// What accepting does, step by step (MAP's knowledge practice).
    pub plan: Vec<String>,
    /// The setup task, once queued.
    #[ts(type = "number | null")]
    pub task: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeHit {
    pub path: String,
    pub kind: KnowledgeKind,
    pub title: String,
    /// The matching passage, matches wrapped in `[` `]`.
    pub snippet: String,
    /// Higher is better.
    pub score: f64,
}

/// `GET /api/projects/{slug}/knowledge/search?q=&kind=&limit=`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeSearch {
    pub query: String,
    pub hits: Vec<KnowledgeHit>,
    #[ts(type = "number")]
    pub took_ms: u64,
}

/// `GET /api/projects/{slug}/knowledge/doc?path=`: one indexed document's text.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeDocBody {
    pub doc: KnowledgeDoc,
    pub body: String,
}

/// How Reflex reads one text: Thai glossed into English words (`thai.rs`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ThaiPreview {
    /// What Reflex is sent.
    pub text: String,
    /// Share of the Thai characters the glossary knew, 0..1 (1 without Thai).
    pub coverage: f64,
    /// At or above `min_coverage`: Reflex is asked; below, the decision goes to the fallback as before.
    pub readable: bool,
    pub has_thai: bool,
    /// Thai runs no word matched: the words to add.
    pub unknown: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ThaiUnknown {
    pub fragment: String,
    /// In how many of the recent Thai decisions.
    pub count: u32,
}

/// The Thai side of a project's Reflex: its words, and what recent Thai decisions did not cover.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ThaiReport {
    pub builtin_words: u32,
    pub min_coverage: f64,
    pub words: BTreeMap<String, String>,
    /// Recent decisions with Thai in their state, and how many of them Reflex could read.
    pub thai_decisions: u32,
    pub readable: u32,
    /// The unknown Thai of those decisions, most frequent first.
    pub unknown: Vec<ThaiUnknown>,
}

/// The orchestrator gates' own Reflex, shared by every project (`gates.rs`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct GatesInfo {
    pub status: ReflexStatus,
    pub port: u16,
    /// The confidence from which its answers are used.
    pub threshold: Option<f64>,
    /// map.toml's `[reflex] gates_threshold`, which a reset goes back to (`None`: shadow).
    pub config_threshold: Option<f64>,
    /// The corpus: one domain per answer (`unclear/`, `plan_first/`, `light/`, `heavy/`, `shared/`, `own/`, `audit/`,
    /// `no_audit/`).
    pub files: Vec<RepoFile>,
    /// Projects with at least one gate on.
    pub projects: Vec<String>,
}


// ---- lanes: tmux sessions someone else owns, which MAP watches (lanes.rs), and Telegram (telegram.rs)

/// A lane session's state on the last tick. `RateLimited` is an idle pane parked at the CLI's usage limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum LaneState {
    Busy,
    Idle,
    /// A question or permission dialog is open; MAP never answers it.
    Waiting,
    /// Unsent text on the input line.
    PendingInput,
    RateLimited,
    /// The tmux session is gone.
    Missing,
}

/// `[lanes.clear_when]`: when MAP types `/clear` into an idle lane session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
#[ts(export)]
pub struct LaneClearWhen {
    pub enabled: bool,
    /// The pane must be unchanged this long (skipped when the agent's status file asks for the clear).
    pub idle_mins: u32,
    /// Never clear again within this long.
    pub cooldown_mins: u32,
    /// Context used (the CLI's `N% context used`) from which a clear is due.
    pub context_pct: u32,
    /// Only after the status file says the card is `done` (and is fresh).
    pub only_when_done: bool,
    /// A status file older than this is ignored.
    pub status_max_age_mins: u32,
}

impl Default for LaneClearWhen {
    fn default() -> Self {
        Self { enabled: false, idle_mins: 30, cooldown_mins: 30, context_pct: 15, only_when_done: true, status_max_age_mins: 180 }
    }
}

/// `[lanes.ask_when]`: when MAP types the lane's `status_prompt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
#[ts(export)]
pub struct LaneAskWhen {
    /// Idle with an unchanged pane this long; 0 = never for this reason.
    pub idle_stale_mins: u32,
    /// No second prompt within this long (a card reported done uses `resume_cooldown_mins`).
    pub cooldown_mins: u32,
    /// The shorter cooldown after a card is done or the usage limit reset.
    pub resume_cooldown_mins: u32,
    pub status_max_age_mins: u32,
    /// Sessions of one lane prompted per tick (1: two agents in one folder never grab the same card).
    pub max_sessions_per_round: u32,
}

impl Default for LaneAskWhen {
    fn default() -> Self {
        Self { idle_stale_mins: 30, cooldown_mins: 60, resume_cooldown_mins: 10, status_max_age_mins: 180, max_sessions_per_round: 1 }
    }
}

/// `[lanes.stall_when]`: a pane stuck where MAP cannot act (a dialog, unsent text, parked after the quota reset).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(default, deny_unknown_fields)]
#[ts(export)]
pub struct LaneStallWhen {
    pub stall_mins: u32,
    /// Alert again after this long while it stays stuck.
    pub repeat_mins: u32,
}

impl Default for LaneStallWhen {
    fn default() -> Self {
        Self { stall_mins: 20, repeat_mins: 120 }
    }
}

/// The agent's own status file (`.scratch/agent-status.json`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneStatusFile {
    #[serde(default)]
    pub card: Option<String>,
    /// `done`, `working` or `blocked`.
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub need_clear: Option<bool>,
    #[serde(default)]
    pub note: Option<String>,
    /// ISO-8601 with offset, as the agent wrote it.
    #[serde(default)]
    pub at: Option<String>,
}

/// Cards in the lane's `tasks_dir` (`todo/`, `todo-human/`, `done/`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneTasks {
    pub todo: u32,
    pub todo_human: u32,
    pub done: u32,
    /// The most recently finished card (file name without `.md`).
    pub last_done: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneGit {
    pub branch: String,
    pub sha: String,
    pub subject: String,
    /// `3 hours ago`.
    pub when: String,
    /// Uncommitted files.
    pub dirty: u32,
    /// Commits not on `origin/<branch>`; `None` without an upstream.
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneSession {
    pub name: String,
    pub state: LaneState,
    /// Why that state (`unsent text in input box: "…"`, the spinner line).
    pub why: String,
    /// A terminal is attached: MAP does not type.
    pub attached: bool,
    pub context_pct: Option<u32>,
    /// Context tokens in use, read from the Claude transcript (`None` when the pane gave the percentage).
    #[ts(type = "number | null")]
    pub context_tokens: Option<u64>,
    /// The lane's `context_window` the percentage is of (with `context_tokens`).
    #[ts(type = "number | null")]
    pub context_window: Option<u64>,
    /// `pane` (the CLI printed `N% context used`) or `transcript`.
    pub context_source: Option<String>,
    /// The CLI suggests `/clear` (`new task? … /clear`).
    pub clear_hint: bool,
    pub rate_limit_line: Option<String>,
    pub rate_limit_resets: Option<DateTime<Utc>>,
    /// Minutes the pane has not changed (volatile timers and counters ignored).
    #[ts(type = "number")]
    pub unchanged_mins: i64,
    /// The stall alert on screen now, if any.
    pub stall: Option<String>,
    /// `pause` tell in force until then.
    pub paused_until: Option<DateTime<Utc>>,
    pub last_clear: Option<DateTime<Utc>>,
    pub last_ask: Option<DateTime<Utc>>,
    /// Sub-agent rows under the input box.
    pub sub_agents: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum TellVerb {
    /// `/clear` (then the status prompt) as soon as the session is idle.
    Clear,
    /// Type the note (or the status prompt) as soon as it is idle.
    Ask,
    /// No automatic clear or prompt for `mins`.
    Pause,
}

/// A queued instruction for a lane session (`POST /api/lanes/{name}/tell`, `deploy/map-tell`); expires after 3 h.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneTell {
    pub lane: String,
    /// The tmux session; empty: whichever session of the lane is idle first.
    pub session: String,
    pub verb: TellVerb,
    pub note: String,
    pub mins: Option<u32>,
    pub at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// One thing MAP did or noticed in a lane (also a `lane.*` event).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneAction {
    #[ts(type = "number")]
    pub id: i64,
    pub at: DateTime<Utc>,
    pub session: Option<String>,
    /// `clear`, `ask`, `resume`, `stall`, `card_done`, `tell`, `rate_limited`, `watch`.
    pub kind: String,
    pub message: String,
}

/// `GET /api/lanes`: a lane's settings (map.toml, then the dashboard's changes) and what the last tick saw.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneInfo {
    pub name: String,
    /// `tmux -L <socket>`; empty: tmux's default server.
    pub socket: String,
    pub session_match: Vec<String>,
    pub path: Option<String>,
    /// Off: no notifications and no typing (screen still read). Toggled by `/watch` on Telegram too.
    pub watch: bool,
    /// Off: watch and notify only, never type.
    pub actions: bool,
    pub clear_when: LaneClearWhen,
    pub ask_when: LaneAskWhen,
    pub stall_when: LaneStallWhen,
    pub status_prompt: String,
    pub sessions: Vec<LaneSession>,
    pub status: Option<LaneStatusFile>,
    #[ts(type = "number | null")]
    pub status_age_mins: Option<i64>,
    pub tasks: Option<LaneTasks>,
    pub git: Option<LaneGit>,
    pub tells: Vec<LaneTell>,
    /// Newest first.
    pub history: Vec<LaneAction>,
    pub checked_at: Option<DateTime<Utc>>,
    /// The last tick failed for this lane (e.g. tmux not reachable).
    pub error: Option<String>,
    /// MAP-owned lanes: the project, and `manager` or `lane`. `None`: a `[[lanes]]` block (someone else's session).
    pub project: Option<String>,
    pub owned: Option<String>,
    /// A MAP-owned lane whose tmux session is gone (it is not relaunched by itself: `POST …/lanes/{name}/start`).
    pub stopped: bool,
}

/// `GET /api/lanes/{name}/screen`: the tail of the last capture, ANSI stripped.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneScreen {
    pub lane: String,
    pub session: Option<String>,
    pub text: String,
    pub at: Option<DateTime<Utc>>,
}

/// `POST /api/lanes/import-watcher`: what the agent-watcher's `config.json` filled in.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LaneImport {
    /// Lanes whose settings were filled from a watcher target.
    pub updated: Vec<String>,
    /// Targets with no matching lane, and why.
    pub skipped: Vec<String>,
    #[ts(type = "number | null")]
    pub chat_id: Option<i64>,
    #[ts(type = "number[]")]
    pub allowed_user_ids: Vec<i64>,
}

/// `GET /api/projects/{slug}/manager`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ManagerInfo {
    pub project: String,
    pub session_name: String,
    pub settings: ManagerSettings,
    /// What `provider: null` starts on now.
    pub default_provider: Option<String>,
    /// `off`, `starting`, `running`, `restarting`, `stopped` (gave up after repeated failures) or `failed`.
    pub state: String,
    /// Why it is stopped or failed.
    pub warning: Option<String>,
    pub session: Option<Session>,
    /// `built-in` or `templates.manager_agent`'s path.
    pub instructions_source: String,
    /// The provider resumes the conversation after a restart (`resume_args`).
    pub resumes: bool,
}

/// A persistent lane MAP opened in a project (`POST /api/projects/{slug}/lanes`), kept in the database.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct OwnedLaneRecord {
    /// The lane's and the tmux session's name: `<prefix>lane-<slug>-<name>`.
    pub lane: String,
    pub project: String,
    /// The short name given when opening it.
    pub name: String,
    pub provider: String,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub status_prompt: String,
    pub clear_when: Option<LaneClearWhen>,
    /// Who opened it: `owner` or `manager`.
    pub opened_by: String,
    pub created_at: DateTime<Utc>,
}

/// `POST /api/projects/{slug}/lanes`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[serde(deny_unknown_fields)]
#[ts(export)]
pub struct OpenLane {
    /// 1–24 letters, digits or `-`.
    pub name: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    /// The first prompt typed into the new session.
    pub prompt: Option<String>,
    pub clear_when: Option<LaneClearWhen>,
    pub status_prompt: Option<String>,
}

/// `GET /api/projects/{slug}/overview`: what a project's manager (or the owner) needs to answer "how is it going".
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectOverview {
    pub project: ProjectInfo,
    pub orchestrator: OrchestratorStatus,
    /// Task counts by status.
    pub counts: BTreeMap<String, u32>,
    /// Open tasks (not done or cancelled), newest first, at most 50.
    pub open_tasks: Vec<Task>,
    /// The latest 10 done tasks.
    pub recent_done: Vec<Task>,
    /// Draft plans waiting for approval.
    pub draft_plans: Vec<Plan>,
    pub sessions: Vec<Session>,
    pub lanes: Vec<LaneInfo>,
    pub manager: ManagerInfo,
}

/// `GET /api/telegram`. The token is never returned: only whether it is set.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TelegramInfo {
    pub enabled: bool,
    pub token_set: bool,
    /// The variable the token is read from (and `env_file`, when set).
    pub token_env: String,
    pub env_file: Option<String>,
    #[ts(type = "number")]
    pub chat_id: i64,
    #[ts(type = "number[]")]
    pub allowed_user_ids: Vec<i64>,
    /// This MAP runs the getUpdates listener (`/watch`, `/status`).
    pub commands: bool,
    /// The listener is connected (not backing off).
    pub polling: bool,
    pub last_send_ok: Option<bool>,
    /// Redacted: never contains the token.
    pub last_error: Option<String>,
    pub last_sent_at: Option<DateTime<Utc>>,
    pub last_report_at: Option<DateTime<Utc>>,
    pub report_minute: u32,
}

// ---- done tasks page

/// `GET /api/tasks/page`: one page of tasks (the Done tasks screen), newest change first.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskPage {
    pub items: Vec<TaskPageItem>,
    /// Tasks matching the filter, across all pages.
    #[ts(type = "number")]
    pub total: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TaskPageItem {
    pub task: Task,
    /// When its first worker session started (`None`: no session was ever started for it). With the task's
    /// `updated_at` (when it finished, for a done task) it gives the run's duration.
    pub started_at: Option<DateTime<Utc>>,
}

// ---- Knowledge graph (knowledge_graph.rs): docs, the code they cite, and the links between them

/// A node of `GET /api/projects/{slug}/knowledge/graph`: a document or a code file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeGraphNode {
    /// `doc:<path>` or `code:<path>`.
    pub id: String,
    /// `doc` or `code`.
    pub kind: String,
    pub label: String,
    pub path: String,
    /// The document's kind (cards, requirements, …); `None` for code.
    pub doc_kind: Option<KnowledgeKind>,
    /// The domain (cluster) it belongs to: `KnowledgeGraphDomain::id`.
    pub domain: String,
    /// A document's worst health finding.
    pub level: Option<Level>,
    /// `ok`, `stale` (a document whose sources changed after it, or a code file such a document cites) or `missing`
    /// (a cited file that does not exist).
    pub state: String,
    pub lines: u32,
    pub degree: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeGraphEdge {
    pub from: String,
    pub to: String,
    /// `source` (frontmatter `sources:`), `mention` (a code path in the text), `link` (a Markdown link to a doc).
    pub kind: String,
    /// `ok`, `stale` or `missing`.
    pub state: String,
}

/// A cluster: the code folder most of its members point at (`api/billing`), or a document folder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeGraphDomain {
    pub id: String,
    pub name: String,
    /// A generated one-line summary (counts and the main documents; no LLM).
    pub summary: String,
    pub docs: u32,
    pub code: u32,
    /// Members with a warning or an error.
    pub flagged: u32,
}

/// `GET /api/projects/{slug}/knowledge/graph`: built from the index on every request (no LLM): documents, the code
/// files they cite, the links between them, clustered by the code folders they point at.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct KnowledgeGraph {
    pub slug: String,
    pub domains: Vec<KnowledgeGraphDomain>,
    pub nodes: Vec<KnowledgeGraphNode>,
    pub edges: Vec<KnowledgeGraphEdge>,
    /// Documents in the index; `nodes` keeps the most useful ones when there are more than the cap.
    pub total_docs: u32,
    pub truncated: bool,
}

// ---- Configure screen: where each setting comes from

/// Where a Configure item's value comes from: the machine (every project), the project alone, or a project value
/// that replaces the machine one with the same key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConfigSource {
    #[default]
    Machine,
    Project,
    Override,
}

/// One variable of the Configure screen's Env & secrets tab, as `GET /api/env` returns it. A secret's value is never
/// returned: only that it is `set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnvVar {
    pub key: String,
    /// The value of a plain variable; always `null` for a secret.
    pub value: Option<String>,
    pub secret: bool,
    /// The variable has a value (a secret's is stored in MAP's secret store).
    pub set: bool,
    pub source: ConfigSource,
}

/// `GET /api/env[?project=<slug>]`: the machine's variables, merged with the project's when one is given (a project
/// variable with a machine variable's key replaces it: `override`). Every agent session, hook and verify/audit
/// command of the project gets these, the machine's first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnvList {
    pub project: Option<String>,
    pub vars: Vec<EnvVar>,
    /// Where plain values of this scope live (`database: runtime_settings` or `database: project settings`).
    pub values_at: String,
    /// Where this scope's secrets live (a file under the server's data folder, mode 0600).
    pub secrets_at: String,
}

/// One variable in `PUT /api/env`. `value: null` keeps a secret's stored value (or turns a plain variable's value
/// into a secret); a string replaces it. A secret made plain needs a new value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnvVarInput {
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub secret: bool,
}

/// `PUT /api/env`: replaces one scope's list (the machine's without `project`). A variable left out is deleted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnvUpdate {
    #[serde(default)]
    pub project: Option<String>,
    pub vars: Vec<EnvVarInput>,
}

/// `DELETE /api/env`: removes one variable from a scope (the machine's without `project`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EnvDelete {
    #[serde(default)]
    pub project: Option<String>,
    pub key: String,
}

/// Which files a Configure call reads and writes: the provider CLIs' own (machine) files, or a project's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConfigScope {
    Machine,
    Project,
}

/// The settings dialect a provider CLI reads (`claude` and `openrouter` both run Claude Code).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CliKind {
    Claude,
    Codex,
    Gemini,
    /// A CLI whose permission and MCP files MAP does not know.
    Other,
}

/// One column of the Permissions matrix and one CLI an MCP server can be given to: a provider MAP runs
/// (`orchestrator.provider_order`, then the audit chain).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConfigCli {
    /// The provider's name in `map.toml`.
    pub name: String,
    pub kind: CliKind,
    /// What this CLI can and cannot do on this tab (Codex: no per-tool rules; its approval policy).
    pub note: Option<String>,
}

/// A file a Configure tab reads (and writes) for one provider and scope.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConfigFile {
    pub provider: String,
    pub scope: ConfigScope,
    pub path: String,
    pub exists: bool,
    /// MAP edits it (false: only read, e.g. Claude's shared `.claude/settings.json`).
    pub writable: bool,
    /// Why it could not be read (invalid JSON or TOML); MAP does not write it then.
    pub error: Option<String>,
}

// ---- Configure → Permissions: tool rules × CLI

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum PermValue {
    Allow,
    Ask,
    Deny,
    /// The CLI has no setting for this rule (Codex: none at all); read only.
    Unsupported,
}

/// One rule for one CLI. In project scope it is the value the CLI will use: the stricter of the project's and the
/// machine's (Claude Code and Gemini both merge the lists, and a deny anywhere wins).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermCell {
    pub provider: String,
    pub value: PermValue,
    /// Which file the value comes from; `None`: no file sets it (the CLI's default, `value`) or unsupported.
    pub source: Option<ConfigSource>,
    /// Partly set, a project value the machine one outweighs, agy having no "ask" list, …
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermRow {
    /// `Read / Grep`, `Bash(git *)`, `mcp__github__*`, …
    pub rule: String,
    pub description: String,
    /// The MCP server of an `mcp__<name>__*` row.
    pub mcp_server: Option<String>,
    pub cells: Vec<PermCell>,
}

/// `GET /api/permissions?project=`: no project is the machine scope.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermissionsView {
    pub project: Option<String>,
    pub scope: ConfigScope,
    pub providers: Vec<ConfigCli>,
    pub rows: Vec<PermRow>,
    pub files: Vec<ConfigFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermChange {
    pub rule: String,
    pub provider: String,
    /// `None`: clear it from this scope's file (inherit the machine value, or the CLI's default).
    pub value: Option<PermValue>,
}

/// `PUT /api/permissions`; returns the fresh [`PermissionsView`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PermissionsUpdate {
    #[serde(default)]
    pub project: Option<String>,
    pub cells: Vec<PermChange>,
}

// ---- Configure → MCP servers

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum McpTransport {
    Stdio,
    /// Streamable HTTP (an SSE-only server reads as this too, with a note).
    Http,
}

/// An environment variable or HTTP header of a server: its name and whether it has a value. Values are never
/// returned: they are often tokens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpKey {
    pub key: String,
    pub set: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum McpState {
    NotTested,
    Connected,
    Failed,
}

/// The last `POST /api/mcp/test` of a server in this scope (kept in memory until MAP restarts).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpStatus {
    pub state: McpState,
    /// `connected · 12 tools`, `failed: <reason>`, `not tested`.
    pub label: String,
    pub tools: Option<u32>,
    pub tested_at: Option<DateTime<Utc>>,
}

/// One MCP server across the CLIs: the same name in several CLIs' files is one server.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpServer {
    pub name: String,
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env_keys: Vec<McpKey>,
    pub header_keys: Vec<McpKey>,
    /// The CLIs that get it (in project scope: from the project's file or inherited from the machine's).
    pub providers: Vec<String>,
    pub source: ConfigSource,
    pub status: McpStatus,
    /// The CLIs' definitions differ, an SSE server, …
    pub notes: Vec<String>,
}

/// `GET /api/mcp?project=`: no project is the machine scope.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpView {
    pub project: Option<String>,
    pub scope: ConfigScope,
    /// The CLIs a server can be given to.
    pub providers: Vec<ConfigCli>,
    pub servers: Vec<McpServer>,
    pub files: Vec<ConfigFile>,
}

/// An environment variable or header to write. `value: None` keeps the value the server already has.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpValueInput {
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpServerInput {
    pub name: String,
    pub transport: McpTransport,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
    /// The whole list: a variable left out is removed.
    #[serde(default)]
    pub env: Vec<McpValueInput>,
    #[serde(default)]
    pub headers: Vec<McpValueInput>,
}

/// `PUT /api/mcp`: write the server into the scope's file of each CLI in `providers` and remove it from the scope's
/// files of the other CLIs; returns the fresh [`McpView`].
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpSave {
    #[serde(default)]
    pub project: Option<String>,
    pub server: McpServerInput,
    pub providers: Vec<String>,
}

/// `DELETE /api/mcp` (removes it from every file of the scope) and `POST /api/mcp/test`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpNameRequest {
    #[serde(default)]
    pub project: Option<String>,
    pub name: String,
}

/// `POST /api/mcp/test`: `initialize`, then `tools/list`, within 15 s.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct McpTestResult {
    pub ok: bool,
    pub tools: u32,
    /// The first 30.
    pub tool_names: Vec<String>,
    pub error: Option<String>,
    #[ts(type = "number")]
    pub duration_ms: u64,
}

// ---- Configure → Rules (rules.rs): the context files agents read at session start

/// Whether one provider reads a rule file, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RuleReader {
    pub provider: String,
    pub reads: bool,
    /// `own file` (its root file, read by name), `via CLAUDE.md` (an `@path` import line in that root file, or a file
    /// it imports), or empty when it does not read it.
    pub how: String,
    /// `PUT /api/rules/readers` cannot change this provider: the file is its own root file, or the provider has no
    /// import syntax (Codex reads only its AGENTS.md).
    pub fixed: bool,
    /// Why it is fixed, or what a change would edit.
    pub note: String,
}

/// One rule file: a provider's root context file (`CLAUDE.md`, `AGENTS.md`, `GEMINI.md`…), a file one of them
/// imports, a rule file MAP created in the project, or a provider's own machine-wide file in its home folder.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RuleFile {
    /// Relative to the project folder for project files; `~/…` (or the absolute path) for machine files. The key
    /// for `PUT`/`DELETE /api/rules` and `PUT /api/rules/readers`.
    pub path: String,
    pub abs_path: String,
    pub source: ConfigSource,
    pub exists: bool,
    pub lines: u32,
    #[ts(type = "number")]
    pub bytes: u64,
    /// Providers that read it at session start (directly, or through an `@path` import in their root file).
    pub readers: Vec<String>,
    /// Every provider with a context file, reading it or not, with whether the readers call can change it.
    pub reader_options: Vec<RuleReader>,
    /// The `@path` imports it contains, as written (`./` dropped).
    pub imports: Vec<String>,
    /// The providers whose root file this is (read by name at session start).
    pub root_of: Vec<String>,
    /// The contents, at most `max_bytes` (see `truncated`).
    pub body: String,
    pub truncated: bool,
}

/// `GET /api/rules`: project files first (with `project`), then the machine files.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RulesOverview {
    pub project: Option<String>,
    /// The project folder (empty in machine scope).
    pub folder: String,
    pub files: Vec<RuleFile>,
    /// Largest body returned or accepted, in bytes.
    #[ts(type = "number")]
    pub max_bytes: u64,
}

/// `PUT /api/rules`: create or replace a rule file. Without `project` only a machine file (its `path` from the
/// listing) can be written.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RuleWrite {
    #[serde(default)]
    pub project: Option<String>,
    pub path: String,
    pub body: String,
}

/// `DELETE /api/rules`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RuleDelete {
    #[serde(default)]
    pub project: Option<String>,
    pub path: String,
}

/// `PUT /api/rules/readers`: exactly these providers read the file afterwards, by adding or removing an `@<path>`
/// import line in each provider's root file. Fixed readers (see [`RuleReader::fixed`]) must stay as they are.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct RuleReaders {
    #[serde(default)]
    pub project: Option<String>,
    pub path: String,
    pub readers: Vec<String>,
}

// ---- Configure → Hooks (hooks.rs): commands MAP runs around a task

/// When a hook runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum HookEvent {
    /// Before a task's agent session starts, in the task's folder (its worktree when it has one).
    PreLaunch,
    /// After verify passes, before the audit.
    PostVerify,
    /// After the task is accepted, after `[orchestrator] post_done`.
    PostDone,
    /// When a task ends failed. Never changes the task.
    OnFail,
    /// When the watcher marks a task's session stalled (once per stall). Never changes the task.
    OnStall,
}

impl HookEvent {
    pub const ALL: [HookEvent; 5] = [HookEvent::PreLaunch, HookEvent::PostVerify, HookEvent::PostDone, HookEvent::OnFail, HookEvent::OnStall];

    pub fn as_str(self) -> &'static str {
        match self {
            HookEvent::PreLaunch => "pre_launch",
            HookEvent::PostVerify => "post_verify",
            HookEvent::PostDone => "post_done",
            HookEvent::OnFail => "on_fail",
            HookEvent::OnStall => "on_stall",
        }
    }

    /// Whether a blocking failure stops the task (`pre_launch`, `post_verify`, `post_done`).
    pub fn can_block(self) -> bool {
        matches!(self, HookEvent::PreLaunch | HookEvent::PostVerify | HookEvent::PostDone)
    }
}

/// A shell command MAP runs at `event` (`sh -c`, in the task's folder, with `MAP_EVENT`, `MAP_TASK_ID`,
/// `MAP_TASK_TITLE`, `MAP_PROJECT`, `MAP_SESSION`, `MAP_BRANCH` and `[workspace.env]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Hook {
    /// Empty on `PUT` for a new hook: MAP makes one. A project hook with a machine hook's id replaces it there.
    #[serde(default)]
    pub id: String,
    pub event: HookEvent,
    pub command: String,
    /// A failure (non-zero exit or timeout) stops the task (failed, a person decides); otherwise it is only logged.
    /// Ignored for `on_fail` and `on_stall`.
    #[serde(default)]
    pub blocking: bool,
    /// 1 to 3600.
    #[serde(default = "default_hook_timeout")]
    pub timeout_secs: u32,
    #[serde(default = "default_hook_enabled")]
    pub enabled: bool,
}

fn default_hook_timeout() -> u32 {
    300
}

fn default_hook_enabled() -> bool {
    true
}

/// A hook as the Configure screen lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HookItem {
    pub id: String,
    pub event: HookEvent,
    pub command: String,
    pub blocking: bool,
    pub timeout_secs: u32,
    pub enabled: bool,
    pub source: ConfigSource,
    /// From `[orchestrator] post_done` in map.toml (id `toml:post_done:<n>`): shown, run as before, not editable here.
    pub read_only: bool,
}

/// One hook event and what it means, for the screen's groups.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HookEventInfo {
    pub event: HookEvent,
    pub description: String,
    /// Whether `blocking` means anything for it.
    pub can_block: bool,
}

/// `GET /api/hooks` (and the answer to `PUT`): machine hooks, plus the project's with `project` (an override in
/// place of the machine hook it replaces), in run order.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HooksOverview {
    pub project: Option<String>,
    pub hooks: Vec<HookItem>,
    pub events: Vec<HookEventInfo>,
}

/// `PUT /api/hooks`: replaces the scope's list (the machine's without `project`). `toml:` hooks are read-only and
/// skipped.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HooksWrite {
    #[serde(default)]
    pub project: Option<String>,
    pub hooks: Vec<Hook>,
}

/// `POST /api/hooks/test`: run a command now in the project folder (or `workspace.dir`).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HookTest {
    #[serde(default)]
    pub project: Option<String>,
    pub command: String,
    #[serde(default = "default_hook_timeout")]
    pub timeout_secs: u32,
}

/// How one run of a command went.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct HookRun {
    /// `None` when it timed out or was killed by a signal.
    pub exit_code: Option<i32>,
    #[ts(type = "number")]
    pub duration_ms: u64,
    /// The last 20 lines of stdout and stderr together, secrets masked.
    pub output_tail: String,
    pub timed_out: bool,
}

impl HookRun {
    pub fn ok(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }
}

// ---- Configure › Workflows (workflows.rs): a task's steps as a graph the orchestrator walks

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkflowNodeKind {
    /// Where the task enters (exactly one): when it is ready (dependencies done, a slot free).
    Trigger,
    /// A worker session with an agent (`AgentDef`) or a provider; its `next` edge when it reports done.
    #[default]
    Agent,
    /// A command in the task's folder: exit 0 follows `pass`, else `fail` while attempts last.
    Verify,
    /// An audit by the named auditor: its verdict text follows `pass`, else `fail`.
    Audit,
    /// A command with a timeout; a blocking failure follows `fail` (or fails the task).
    Hook,
    /// The task ends done (then landing and `post_done`) or failed.
    End,
}

impl WorkflowNodeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Trigger => "trigger",
            Self::Agent => "agent",
            Self::Verify => "verify",
            Self::Audit => "audit",
            Self::Hook => "hook",
            Self::End => "end",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkflowEdgeKind {
    /// The way on after success. Out of a verify, audit or hook it means the same as `pass`.
    #[default]
    Next,
    /// Out of a verify, audit or hook that passed. Out of a trigger or an agent it is read as `next`.
    Pass,
    /// Out of an agent (it failed, exited or timed out), a verify, an audit, or a blocking hook that failed.
    Fail,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum WorkflowOutcome {
    #[default]
    Done,
    Failed,
}

/// One step of a workflow. Only the fields of its kind are read.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct WorkflowNode {
    /// `[A-Za-z0-9_-]`, 1 to 40 characters, unique in the workflow.
    pub id: String,
    pub kind: WorkflowNodeKind,
    pub label: String,
    /// Position on the editor's canvas.
    pub x: f64,
    pub y: f64,
    /// agent, audit: an `AgentDef` name in scope (the project's or the user's agents) or a provider name.
    pub agent: Option<String>,
    /// agent: text put before the task's prompt, after the agent's own instructions.
    pub prompt: Option<String>,
    /// verify, hook: run with `sh -c` in the task's folder.
    pub command: Option<String>,
    /// verify (1–10, required), audit (default `orchestrator.max_attempts`): runs after which the fail edge is no
    /// longer taken and the task goes to its failed end.
    pub max_attempts: Option<u32>,
    /// audit: the text that means pass (default `VERDICT: PASS`).
    pub verdict: Option<String>,
    /// hook: when it is meant to run (`pre_launch`, `post_verify`, `post_done`, `on_fail`, `on_stall`). Its place in
    /// the graph decides when it really runs; this is a label.
    pub event: Option<String>,
    /// hook: a failure follows the fail edge (or fails the task); not blocking, it is logged and the task goes on.
    /// Default true.
    pub blocking: Option<bool>,
    /// hook: default 600.
    #[ts(type = "number | null")]
    pub timeout_secs: Option<u64>,
    /// end: default done.
    pub outcome: Option<WorkflowOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub kind: WorkflowEdgeKind,
}

/// Which tasks a workflow takes when they do not name one.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct WorkflowTrigger {
    /// Takes every task in its scope that no keyword workflow took.
    pub default: bool,
    /// Words matched in the task's title and prompt, case-insensitively, as whole words (`bug`, `fix`).
    pub keywords: Vec<String>,
    /// Globs of the paths the task is about; shown only, not matched yet.
    pub paths: Vec<String>,
}

/// A task's steps as a graph: Configure › Workflows. Machine workflows apply to every project; a project's own one
/// with the same id overrides it (`source: override`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct Workflow {
    /// `[A-Za-z0-9_-]`, 1 to 40 characters.
    pub id: String,
    pub name: String,
    pub description: String,
    pub trigger: WorkflowTrigger,
    pub nodes: Vec<WorkflowNode>,
    pub edges: Vec<WorkflowEdge>,
    /// Set by the server when listing; ignored on save.
    pub source: ConfigSource,
}

/// `GET /api/workflows?project=`: the machine's workflows merged with the project's.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowList {
    pub project: Option<String>,
    pub workflows: Vec<Workflow>,
}

/// One node's numbers over the last runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowNodeStats {
    pub node: String,
    /// Runs (tasks) that reached it.
    pub runs: u32,
    /// Times it ran (a retry loop runs a node more than once per task).
    pub visits: u32,
    /// Average time from the node's start to its outcome.
    #[ts(type = "number | null")]
    pub avg_ms: Option<i64>,
    /// Share of its finished visits that failed, 0–100.
    pub fail_pct: Option<f64>,
}

/// One task that ran the workflow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowRun {
    #[ts(type = "number")]
    pub task_id: i64,
    pub title: String,
    pub status: TaskStatus,
    /// From its first workflow step to its last event.
    #[ts(type = "number | null")]
    pub duration_ms: Option<i64>,
    /// What failed on the way (verify output, audit reasons), each shortened.
    pub failures: Vec<String>,
    pub at: DateTime<Utc>,
}

/// `GET /api/workflows/{id}/stats?project=`: per node over the last 30 runs of tasks that used it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct WorkflowStats {
    pub workflow: String,
    pub project: Option<String>,
    pub runs: u32,
    pub nodes: Vec<WorkflowNodeStats>,
    /// Newest first.
    pub recent: Vec<WorkflowRun>,
}

// ---- Configure › "Ask an agent to do it" (configure_ask.rs)

/// The Configure tab an agent is asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConfigTab {
    Agents,
    Rules,
    Skills,
    Perms,
    Hooks,
    Verify,
    Workflows,
}

/// The language of the config an agent writes (rules, role prompts, labels). English unless the owner chose Thai.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConfigLanguage {
    #[default]
    En,
    Th,
}

/// `POST /api/configure/ask`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConfigAskRequest {
    pub tab: ConfigTab,
    #[serde(default)]
    pub project: Option<String>,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<Effort>,
    /// May be empty for `workflows` with `from_runs`.
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub language: ConfigLanguage,
    /// The tab's current config as the dashboard has it (sent to the agent, shortened when large).
    #[serde(default)]
    pub context: serde_json::Value,
    /// workflows: the workflow the ops are for.
    #[serde(default)]
    pub workflow_id: Option<String>,
    /// workflows: also send the per-node stats and outcomes of its last 30 runs.
    #[serde(default)]
    pub from_runs: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ConfigOpKind {
    Add,
    Change,
    Remove,
}

/// One proposed change, shown as a `+` / `~` / `−` line the owner checks. The dashboard applies the checked ones to
/// its tab and saves through that tab's own API; the backend never writes config from a proposal.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConfigOp {
    pub op: ConfigOpKind,
    /// What it changes: `agent:<name>`, `rule:<file>`, `skill:<name>`, `perm:<rule>:<agent>`, `hook:<event>`,
    /// `verify.cmds`, `verify.max_attempts`, `audit.enabled`, `audit.auditor`, `audit.verdict`,
    /// `workflow.node:<id>`, `workflow.edge`, `workflow.trigger`.
    pub target: String,
    /// The line the owner reads.
    pub text: String,
    /// The change itself; its shape per tab is in `configure_ask.rs` (`schema`). Workflows: the editor's op,
    /// `{"op": "add", "after", "before", "node", "fail_to"?}`, `{"op": "insert_before", "target", "node"}`,
    /// `{"op": "remove", "id", "bridge"?: [from, to, kind]}`, `{"op": "change", "id", "key", "to"}`.
    pub value: serde_json::Value,
}

/// What the agent proposed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ConfigAskResult {
    pub summary: String,
    /// Why: facts the agent read, then a line for each proposal MAP dropped as invalid.
    pub evidence: Vec<String>,
    pub ops: Vec<ConfigOp>,
    pub provider: String,
    pub model: String,
    pub language: ConfigLanguage,
    #[ts(type = "number")]
    pub duration_ms: i64,
}

// ---- activity graph (trace.rs): who made what, and what each agent used

/// One thing an agent did, read from its CLI's transcript (aggregated: the same command head `×count`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TraceItem {
    /// `command`, `skill`, `knowledge`, `subagent` or `tool`.
    pub kind: String,
    /// The command's head (`cargo test`, `git commit`), the skill's or sub-agent's name, the knowledge card's path,
    /// the tool's name.
    pub name: String,
    /// The latest invocation in full, secrets masked, at most 300 characters.
    pub detail: String,
    pub count: u32,
    pub first_at: Option<DateTime<Utc>>,
    pub last_at: Option<DateTime<Utc>>,
    /// The latest few exact invocations (masked), newest last.
    pub samples: Vec<String>,
    /// The last lines each of `samples` printed (masked; empty when the transcript has no result for it), same order.
    #[serde(default)]
    pub outputs: Vec<String>,
}

/// `GET /api/sessions/{name}/trace`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SessionTrace {
    pub session: String,
    pub provider: String,
    /// The transcript read (`None`: none found, or the provider has no trace reader).
    pub file: Option<String>,
    pub items: Vec<TraceItem>,
}

/// A node of `GET /api/projects/{slug}/activity`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityNode {
    pub id: String,
    /// `manager`, `planner`, `owner`, `orchestrator` (a task's pick, with the gate verdicts), `task`, `agent`,
    /// `step` (verify, audit, land, review, done, failed), `subagent`, `command`, `skill`, `knowledge`, `tool`.
    pub kind: String,
    pub label: String,
    pub detail: Option<String>,
    pub count: u32,
    /// `running`, `ok`, `fail`, `waiting`, or the task's / session's state.
    pub state: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
    #[ts(type = "number | null")]
    pub task_id: Option<i64>,
    pub session: Option<String>,
    /// The agent node a usage node (command, skill, knowledge, sub-agent, tool) hangs off.
    pub parent: Option<String>,
    /// Column, left to right (creators 0, tasks 1, then each hand-off).
    pub layer: u32,
    /// Row: one per task in order; shared nodes (creators, sessions without a task) use their own rows.
    pub row: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub last_at: Option<DateTime<Utc>>,
    /// Exact invocations of a command node (masked).
    pub samples: Vec<String>,
    /// The tail of what each of `samples` printed (masked; empty when unknown), same order.
    pub outputs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityEdge {
    pub from: String,
    pub to: String,
    /// `created`, `picked`, `assigned`, `handed`, `retry`, `unblocks`, `spawned`, `called`, `used`, `read`.
    pub kind: String,
    pub label: Option<String>,
    pub count: u32,
    pub last_at: Option<DateTime<Utc>>,
    /// The hand-off in progress now (the task is still on it).
    pub live: bool,
}

/// `GET /api/projects/{slug}/activity`: the project's task flow and what each agent used, ready to draw.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ActivityGraph {
    pub project: String,
    pub since: Option<DateTime<Utc>>,
    pub generated_at: DateTime<Utc>,
    pub nodes: Vec<ActivityNode>,
    pub edges: Vec<ActivityEdge>,
}
