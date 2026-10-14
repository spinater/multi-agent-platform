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
    /// `workspace.tmux_socket`: the private tmux server (`tmux -L <name>`) agents run on; `None`: tmux's default.
    pub tmux_socket: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AgentsOverview {
    /// The project slug; `None` is `workspace.dir`.
    pub project: Option<String>,
    pub workspace: String,
    pub rows: Vec<AgentRow>,
    pub errors: Vec<AgentFileError>,
    pub dirs: Vec<AgentDir>,
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
}

/// Repeated orchestrator decisions answered by the project's Reflex (its `orchestration` corpus domain). An answer
/// below the project's threshold, an abstain or a Reflex that is not running leaves today's behaviour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(default)]
#[ts(export)]
pub struct ReflexGates {
    /// Ask `needs_audit` before the LLM audit; a confident "no" accepts the task on verification.
    pub audit: bool,
    /// Ask `tier` (light, standard, heavy) for a task without a pinned model and use `[orchestrator.tiers]`.
    pub tier: bool,
}

impl ReflexGates {
    pub fn any(&self) -> bool {
        self.audit || self.tier
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
    /// What MAP exports to relocate it in isolated mode (`CLAUDE_CONFIG_DIR=/…/agent-home/claude`); `None` in shared
    /// mode, or when the CLI has no such variable.
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
