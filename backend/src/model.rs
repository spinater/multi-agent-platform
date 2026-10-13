//! Types shared by the store, the API and the frontend. `cargo test` exports them to
//! `frontend/lib/generated/` through ts-rs, so the dashboard never drifts from the server.

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
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
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
    #[ts(type = "number")]
    pub max_parallel: usize,
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
}

/// Messages pushed to dashboard clients over `/api/ws`.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
#[ts(export)]
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
    Project { project: ProjectInfo },
    ProjectRemoved { slug: String },
    /// The client fell behind and missed messages; reload `/api/overview`.
    Resync,
    /// A new usage reading (every `usage.interval_secs`, or after `POST /api/usage/refresh`).
    Usage { report: UsageReport },
    /// A provider's sign-in, availability or cooldown changed (checked every watcher tick), e.g. after a login.
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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
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
    Ready,
    Pick,
    Launch,
    Watch,
    Verify,
    Audit,
    Post,
    End,
    Retry,
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

/// The folder a project's agents work in. The platform works on folders on this machine in place: it never copies,
/// clones or hosts a project's code.
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
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectSource {
    pub kind: SourceKind,
    /// The project's folder on this machine (`empty`, `local`), or the URL a legacy `git` project was cloned from.
    #[serde(default)]
    pub location: Option<String>,
    /// The branch a legacy `git` project was cloned from; unused for folders.
    #[serde(default)]
    pub branch: Option<String>,
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
