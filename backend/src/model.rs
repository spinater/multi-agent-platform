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
    /// The checkout of the code repo that the project's agents work in.
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

// ---- project settings

/// Where a project's code repo came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SourceKind {
    /// A new repo with the platform's seed files.
    #[default]
    Empty,
    /// `git clone --bare` of a URL.
    Git,
    /// `git clone --bare` of a git repo on this machine.
    Local,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ProjectSource {
    pub kind: SourceKind,
    /// The git URL (`git`) or the path of the repo on this machine (`local`).
    #[serde(default)]
    pub location: Option<String>,
    /// The branch imported as the platform repo's `main`; `None` is the source's default branch.
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

/// What the wizard found at a project source before importing it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SourceInspection {
    pub ok: bool,
    pub error: Option<String>,
    pub default_branch: Option<String>,
    pub branches: Vec<String>,
    /// Marker files at the top of a local repo's branch (`Cargo.toml`, `package.json`, `CLAUDE.md`, …).
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
