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
