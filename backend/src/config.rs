//! `map.toml`: every path, session name and command the platform uses comes from here, never from code.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub watcher: WatcherConfig,
    #[serde(default)]
    pub orchestrator: OrchestratorConfig,
    #[serde(default)]
    pub reflex: ReflexConfig,
    #[serde(default)]
    pub skills: SkillsConfig,
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub usage: UsageConfig,
    #[serde(default)]
    pub updates: UpdatesConfig,
    #[serde(default)]
    pub templates: TemplatesConfig,
    #[serde(default)]
    pub projects: ProjectsConfig,
    #[serde(default)]
    pub agents_home: AgentsHomeConfig,
    /// Overrides merged onto the built-in `claude`, `codex` and `gemini` specs; new names add providers.
    #[serde(default, rename = "providers")]
    provider_overrides: BTreeMap<String, ProviderOverride>,
    #[serde(skip)]
    pub providers: BTreeMap<String, ProviderSpec>,
    /// The file this config was loaded from (absolute); `None` for a config parsed from text.
    #[serde(skip)]
    pub path: Option<PathBuf>,
    /// Problems found while loading that do not stop MAP (e.g. a project root that does not exist yet); `serve`
    /// and `check` log them.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ServerConfig {
    /// Loopback by default. Put an authenticating proxy in front before binding anything wider.
    pub bind: String,
    /// When set, every `/api` request needs `Authorization: Bearer <token>` (or `?token=` for the WebSocket).
    pub api_token: Option<String>,
    /// The exported Next.js dashboard (`frontend/out`). Missing directory: API only.
    pub static_dir: PathBuf,
    /// SQLite database and per-run files (prompts, launch scripts, result files, final panes).
    pub data_dir: PathBuf,
    /// Host names accepted in the `Host` header besides loopback (e.g. your tunnel's domain). Blocks DNS rebinding.
    pub allowed_hosts: Vec<String>,
    /// Browser origins allowed to open `/api/ws` besides the dashboard's own (the `next dev` server by default).
    pub allowed_origins: Vec<String>,
    /// How the installer set this machine up (`isolated` or `system`); informational, shown in Settings.
    pub mode: Option<crate::model::InstallMode>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1:4400".into(),
            api_token: None,
            static_dir: "frontend/out".into(),
            data_dir: ".map".into(),
            allowed_hosts: vec![],
            allowed_origins: vec!["http://localhost:3000".into(), "http://127.0.0.1:3000".into()],
            mode: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceConfig {
    /// The repository agents work in.
    pub dir: PathBuf,
    /// Only tmux sessions with this prefix are managed; the platform never touches anything else.
    #[serde(default = "default_prefix")]
    pub session_prefix: String,
    /// Exported in every agent's launch script and verify command (e.g. a PATH that finds the agent CLIs).
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Before it starts an agent in a folder (`dir`, a project's folder), MAP marks the folder trusted in the CLI's
    /// own config (each provider's `trust`), so the CLI does not stop at its "trust this folder?" question. `false`:
    /// MAP leaves those files alone and you answer the question yourself.
    #[serde(default = "default_true")]
    pub trust_folders: bool,
    /// A private tmux server (`tmux -L <name>`) for every session MAP runs; unset: tmux's default server. MAP always
    /// drops `TMUX` and `TMUX_PANE` from its tmux commands, so a MAP started inside tmux never targets that server by
    /// accident.
    #[serde(default)]
    pub tmux_socket: Option<String>,
}

fn default_prefix() -> String {
    "map-".into()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WatcherConfig {
    pub interval_secs: u64,
    pub stall_minutes: u64,
    pub wake_cooldown_secs: u64,
    /// Used when a rate-limit message carries no reset time.
    pub default_rate_limit_wait_secs: u64,
    pub wake_prompt: String,
    /// Only the bottom of the pane is scanned, so an old error in the scrollback does not re-trigger.
    pub scan_lines: usize,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            interval_secs: 5,
            stall_minutes: 45,
            wake_cooldown_secs: 60,
            default_rate_limit_wait_secs: 300,
            wake_prompt: "The rate limit has reset. Check where you stopped and continue the task.".into(),
            scan_lines: 40,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct OrchestratorConfig {
    pub autostart: bool,
    pub max_parallel: usize,
    /// A task that fails verification is retried with the failure appended to its prompt, up to this many runs.
    pub max_attempts: i64,
    pub task_timeout_minutes: u64,
    /// Providers tried in order for tasks that do not pin one.
    pub provider_order: Vec<String>,
    /// How long a provider is skipped after a quota or auth failure.
    pub provider_cooldown_minutes: u64,
    /// Shell commands run in the workspace after the agent reports done; all must exit 0.
    pub verify: Vec<String>,
    /// Shell commands run after a task passes (e.g. a commit). `{id}` and `{title}` are substituted, shell-quoted.
    pub post_done: Vec<String>,
    pub audit: AuditConfig,
    /// Models for the project Reflex tier gate (`[orchestrator.tiers.<provider>]`). Built in for `claude` only; a
    /// provider without a table, or a tier left out, keeps the model it would have used anyway.
    pub tiers: BTreeMap<String, TierModels>,
}

/// One provider's model per task weight, for the Reflex tier gate.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TierModels {
    pub light: Option<String>,
    pub standard: Option<String>,
    pub heavy: Option<String>,
}

impl TierModels {
    /// The model for `tier` (`light`, `standard` or `heavy`).
    pub fn get(&self, tier: &str) -> Option<&str> {
        match tier {
            "light" => self.light.as_deref(),
            "standard" => self.standard.as_deref(),
            "heavy" => self.heavy.as_deref(),
            _ => None,
        }
        .filter(|m| !m.is_empty())
    }

    pub fn is_empty(&self) -> bool {
        ["light", "standard", "heavy"].iter().all(|t| self.get(t).is_none())
    }
}

/// Tier models used when `map.toml` has no `[orchestrator.tiers.claude]`.
fn builtin_claude_tiers() -> TierModels {
    TierModels { light: Some("haiku".into()), standard: Some("sonnet".into()), heavy: Some("opus".into()) }
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            autostart: false,
            max_parallel: 1,
            max_attempts: 2,
            task_timeout_minutes: 240,
            provider_order: vec!["claude".into(), "codex".into(), "gemini".into()],
            provider_cooldown_minutes: 30,
            verify: vec![],
            post_done: vec![],
            audit: AuditConfig::default(),
            tiers: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AuditConfig {
    pub enabled: bool,
    /// When every auditor is unavailable: `false` accepts the task on verification alone, `true` fails it.
    pub required: bool,
    pub chain: Vec<String>,
    pub timeout_secs: u64,
}

impl Default for AuditConfig {
    fn default() -> Self {
        Self { enabled: false, required: false, chain: vec!["gemini".into(), "claude".into(), "codex".into()], timeout_secs: 600 }
    }
}

/// Projects' decision engines: one `reflex` process per project, fed by its corpus repo.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ReflexConfig {
    /// The Reflex binary (https://reflex.gist.rs). Missing binary: projects still work, decisions go to the fallback.
    pub binary: String,
    /// Each project's Reflex listens on loopback at the first free port from here.
    pub port_base: u16,
    /// Environment variable holding the API key for projects whose fallback is `api` (read from MAP's environment or
    /// `[workspace.env]` for each call; never stored or shown).
    pub api_key_env: String,
    /// Model for the `api` fallback when a project does not set one.
    pub api_model: String,
    /// The `api` fallback's wire format: `anthropic` (Messages API, `x-api-key`) or `openai` (Chat Completions,
    /// `Authorization: Bearer`), which OpenRouter and other gateways speak.
    pub api_format: ApiFormat,
    /// The endpoint the `api` fallback posts to; unset: `https://api.anthropic.com/v1/messages` for `anthropic`,
    /// `https://api.openai.com/v1/chat/completions` for `openai`. For OpenRouter:
    /// `https://openrouter.ai/api/v1/chat/completions`.
    pub api_url: Option<String>,
    /// Tokens added to every call of a fallback (`api`, or a provider name) in the savings report, both saved and
    /// spent: a headless CLI's own system prompt, which its output does not report. Missing: 0.
    pub fallback_overhead_tokens: BTreeMap<String, u64>,
    /// The orchestrator gates' own Reflex (one for every project, `<data_dir>/gates/corpus.git`); unset: the port
    /// just below `port_base`.
    pub gates_port: Option<u16>,
    /// The confidence from which the gates Reflex's answer is used; below it a gate does what it did before. Unset
    /// (the default): shadow, every gate answer is logged on the projects' Reflex screens but none is acted on, until
    /// a threshold is set from the confidences of answers that were right. Set live in Settings → Reflex gates.
    pub gates_threshold: Option<f64>,
}

impl Default for ReflexConfig {
    fn default() -> Self {
        Self {
            binary: "reflex".into(),
            port_base: 7400,
            api_key_env: "ANTHROPIC_API_KEY".into(),
            api_model: "claude-haiku-5-5".into(),
            api_format: ApiFormat::Anthropic,
            api_url: None,
            fallback_overhead_tokens: BTreeMap::new(),
            gates_port: None,
            gates_threshold: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiFormat {
    #[default]
    Anthropic,
    Openai,
}

impl ReflexConfig {
    /// Where the `api` fallback posts.
    pub fn api_endpoint(&self) -> &str {
        match (&self.api_url, self.api_format) {
            (Some(url), _) => url,
            (None, ApiFormat::Anthropic) => "https://api.anthropic.com/v1/messages",
            (None, ApiFormat::Openai) => "https://api.openai.com/v1/chat/completions",
        }
    }

    /// The `api` fallback's name on the dashboard: "Anthropic API", "OpenRouter API", "OpenAI-compatible API".
    pub fn api_name(&self) -> String {
        let host = self.api_endpoint().split("://").nth(1).and_then(|r| r.split(['/', ':']).next()).unwrap_or("").to_ascii_lowercase();
        let known = [("anthropic.com", "Anthropic API"), ("openrouter.ai", "OpenRouter API"), ("openai.com", "OpenAI API")];
        match known.iter().find(|(h, _)| host == *h || host.ends_with(&format!(".{h}"))) {
            Some((_, name)) => (*name).into(),
            None if self.api_format == ApiFormat::Openai => format!("OpenAI-compatible API ({host})"),
            None => format!("Anthropic-compatible API ({host})"),
        }
    }
}

/// Whose logins and history MAP's agents use (`[agents_home]`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentsHomeMode {
    /// The CLIs' usual folders (`~/.claude`, `~/.codex`, `~/.gemini`): the same accounts and history as your own
    /// `claude` / `codex` / `agy` in a terminal.
    #[default]
    Shared,
    /// Each provider gets `<dir>/<provider>`, exported in its `home_env` to every CLI MAP starts: MAP's agents sign in
    /// once with their own accounts, and keep their own history. Your own CLIs in a terminal are unaffected.
    Isolated,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsHomeConfig {
    pub mode: AgentsHomeMode,
    /// The parent of the per-provider folders in `isolated` mode. Default `<folder of map.toml>/agent-home`
    /// (`<data_dir>/agent-home` for a config without a file). `~/` is `$HOME`; relative paths resolve against the
    /// config's folder.
    dir: Option<PathBuf>,
    #[serde(skip)]
    resolved: PathBuf,
}

impl AgentsHomeConfig {
    /// The resolved parent folder (used in `isolated` mode only).
    pub fn dir(&self) -> &Path {
        &self.resolved
    }

    pub fn isolated(&self) -> bool {
        self.mode == AgentsHomeMode::Isolated
    }
}

/// Where the Skills screen looks for skills, context files and evidence of their use. Everything here is read only,
/// except `.claude/settings.local.json` inside a workspace, which enabling or disabling a skill writes.
/// `~` expands to `$HOME`; `*_project_dirs` are relative to each workspace.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SkillsConfig {
    /// Context file each provider reads at session start, relative to the workspace.
    pub context_files: BTreeMap<String, String>,
    /// Skill folders inside a workspace that Claude Code loads (`<dir>/<name>/SKILL.md`).
    pub claude_project_dirs: Vec<PathBuf>,
    /// Skill folders inside a workspace that Codex loads.
    pub codex_project_dirs: Vec<PathBuf>,
    /// Other CLIs that load skills natively from some of those workspace folders, per provider (Cursor and Copilot
    /// read `.claude/skills` and `.agents/skills`; `openrouter` is Claude Code).
    pub also_loaded_by: BTreeMap<String, Vec<PathBuf>>,
    /// Personal Claude Code skills, searched recursively (claude.ai-synced skills live under `synced/`).
    pub claude_user_dirs: Vec<PathBuf>,
    /// Claude Code plugins: every folder with `.claude-plugin/plugin.json` below these is a plugin, and its
    /// `skills/` folder holds its skills. Marketplace catalogs count only for plugins `enabledPlugins` turns on.
    pub claude_plugin_dirs: Vec<PathBuf>,
    /// Personal Codex skills.
    pub codex_user_dirs: Vec<PathBuf>,
    /// Claude Code's user settings: `enabledPlugins` and permission rules that disable skills everywhere.
    pub claude_settings: PathBuf,
    /// Claude Code's session transcripts (`<dir>/<workspace path with non-alphanumerics as ->/*.jsonl`): the
    /// evidence for load counts.
    pub claude_transcripts: PathBuf,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        let context_files = [("claude", "CLAUDE.md"), ("codex", "AGENTS.md"), ("gemini", "GEMINI.md"), ("copilot", ".github/copilot-instructions.md")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Self {
            context_files,
            claude_project_dirs: vec![".claude/skills".into()],
            codex_project_dirs: vec![".agents/skills".into()],
            also_loaded_by: [
                ("cursor", vec![".claude/skills", ".agents/skills"]),
                ("copilot", vec![".claude/skills", ".agents/skills"]),
                ("openrouter", vec![".claude/skills"]),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.into_iter().map(PathBuf::from).collect()))
            .collect(),
            claude_user_dirs: vec!["~/.claude/skills".into()],
            claude_plugin_dirs: vec!["~/.claude/plugins".into()],
            codex_user_dirs: vec!["~/.codex/skills".into()],
            claude_settings: "~/.claude/settings.json".into(),
            claude_transcripts: "~/.claude/projects".into(),
        }
    }
}

/// Where the Agents screen finds and writes custom sub-agents, per CLI. MAP writes `<name>.md` (Claude, agy) or
/// `<name>.toml` (Codex) in the first folder of a list, and never outside these folders. `~` expands to `$HOME`;
/// `*_project_dirs` are relative to each workspace.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentsConfig {
    /// Claude Code: Markdown with a YAML header, searched recursively.
    pub claude_project_dirs: Vec<PathBuf>,
    /// Codex: one TOML file per agent.
    pub codex_project_dirs: Vec<PathBuf>,
    /// Antigravity (`agy`, the `gemini` provider): `<name>.md` or `<name>/agent.md`.
    pub gemini_project_dirs: Vec<PathBuf>,
    pub claude_user_dirs: Vec<PathBuf>,
    pub codex_user_dirs: Vec<PathBuf>,
    pub gemini_user_dirs: Vec<PathBuf>,
    /// CLIs MAP writes no files for that load another CLI's agents natively, with the scopes they load them in:
    /// Cursor and GitHub Copilot read Claude Code's `.claude/agents` in a workspace. Shown on the Agents screen only.
    pub also_loaded_by: BTreeMap<String, AlsoLoaded>,
}

/// `[agents.also_loaded_by.<provider>]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AlsoLoaded {
    pub reads: crate::model::AgentProvider,
    #[serde(default = "project_scope")]
    pub scopes: Vec<crate::model::AgentScope>,
}

fn project_scope() -> Vec<crate::model::AgentScope> {
    vec![crate::model::AgentScope::Project]
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            claude_project_dirs: vec![".claude/agents".into()],
            codex_project_dirs: vec![".codex/agents".into()],
            gemini_project_dirs: vec![".agents/agents".into()],
            claude_user_dirs: vec!["~/.claude/agents".into()],
            codex_user_dirs: vec!["~/.codex/agents".into()],
            gemini_user_dirs: vec!["~/.gemini/config/agents".into()],
            also_loaded_by: ["cursor", "copilot"]
                .into_iter()
                .map(|p| (p.to_string(), AlsoLoaded { reads: crate::model::AgentProvider::Claude, scopes: project_scope() }))
                .collect(),
        }
    }
}

/// Plan limits per provider account (`backend/src/usage.rs`), and the line past which the orchestrator gives a
/// provider no new tasks. Paths may start with `~/`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UsageConfig {
    pub enabled: bool,
    pub interval_secs: u64,
    /// No new tasks for a provider once any of its windows is at or above this percentage.
    pub skip_pct: f64,
    /// Claude Code's sign-in (`claudeAiOauth`). Its token is only ever sent to api.anthropic.com.
    pub claude_credentials: PathBuf,
    /// Claude Code's transcripts, for tokens per day and per project.
    pub claude_projects: PathBuf,
    /// Codex's rollout logs: rate-limit snapshots and token counts.
    pub codex_sessions: PathBuf,
    /// Codex's sign-in file; only its presence is checked.
    pub codex_auth: PathBuf,
    /// Per provider: turn the reader off, or pick one (`claude`, `codex`, `openrouter`, `none`) for a provider with another name.
    pub providers: BTreeMap<String, UsageProviderConfig>,
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            interval_secs: 300,
            skip_pct: 90.0,
            claude_credentials: "~/.claude/.credentials.json".into(),
            claude_projects: "~/.claude/projects".into(),
            codex_sessions: "~/.codex/sessions".into(),
            codex_auth: "~/.codex/auth.json".into(),
            providers: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UsageProviderConfig {
    pub enabled: bool,
    pub reader: Option<String>,
}

impl Default for UsageProviderConfig {
    fn default() -> Self {
        Self { enabled: true, reader: None }
    }
}

/// Update checks for the agent CLIs (`backend/src/updates.rs`): each provider's `version` command against the
/// version its `latest` source publishes.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdatesConfig {
    pub enabled: bool,
    /// At least 300.
    pub interval_secs: u64,
    /// Limit for each version command and each request for the latest version.
    pub timeout_secs: u64,
}

impl Default for UpdatesConfig {
    fn default() -> Self {
        Self { enabled: true, interval_secs: 21600, timeout_secs: 15 }
    }
}

/// Project templates (`backend/src/templates.rs`): folders with a `template.toml`, next to the built-in ones, and
/// the init agent's instructions. Paths may start with `~/`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TemplatesConfig {
    /// Each folder's subfolders that hold a `template.toml` are templates. Missing folders are ignored.
    pub dirs: Vec<PathBuf>,
    /// The init agent's instructions; unset: the ones built into the binary (`backend/templates/init-agent.md`).
    pub init_agent: Option<PathBuf>,
    /// The planner agent's instructions; unset: the ones built into the binary (`backend/templates/planner-agent.md`).
    pub planner_agent: Option<PathBuf>,
}

impl Default for TemplatesConfig {
    fn default() -> Self {
        Self { dirs: vec!["~/.config/map/templates".into()], init_agent: None, planner_agent: None }
    }
}

/// The project folder (`path`): every project's folder must be a sub-folder of it, the dashboard browses only inside
/// it, and "Clone from git" suggests `<path>/<repo name>`. Also how long MAP waits for git on the network.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ProjectsConfig {
    /// The project folder. `~/` is `$HOME`; a relative path resolves against the config's folder. `root` is its older
    /// name. Default `~/project`.
    #[serde(alias = "root")]
    path: Option<PathBuf>,
    /// Advanced, instead of `path`: several named project folders (`roots.rs`). Each project's folder must be inside
    /// one of them. After `Config::parse` the list is never empty (`path` becomes a single root); read it with
    /// [`ProjectsConfig::roots`].
    roots: Option<Vec<ProjectRoot>>,
    /// With `roots`: the one "Clone from git" suggests `<root>/<repo name>` in, by name. Unset: the first.
    pub default_root: Option<String>,
    /// Timeout for git commands MAP runs over the network (clone, ls-remote, fetch, pull, push).
    pub git_timeout_secs: u64,
    #[serde(skip)]
    default_index: usize,
}

/// One allowed parent of project folders (`roots = [{ name, path }]` or `[[projects.roots]]`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectRoot {
    /// Shown on the dashboard's root chips and used in `GET /api/fs/browse?root=<name>`.
    pub name: String,
    /// The folder as MAP sees it (inside the container, in Docker). Absolute, or `~/…`.
    pub path: PathBuf,
    /// Display only: the same folder as seen on the host, when MAP runs in a container that mounts it elsewhere.
    #[serde(default)]
    pub host_path: Option<PathBuf>,
    /// The root folder itself may be a project's folder; by default only its sub-folders may.
    #[serde(default)]
    pub allow_root_itself: bool,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self { path: None, roots: None, default_root: None, git_timeout_secs: 300, default_index: 0 }
    }
}

impl ProjectsConfig {
    /// The allowed roots, in `map.toml`'s order (never empty once parsed).
    pub fn roots(&self) -> &[ProjectRoot] {
        self.roots.as_deref().unwrap_or_default()
    }

    /// The root "Clone from git" suggests a folder in (`default_root`, else the first).
    pub fn default_root(&self) -> &ProjectRoot {
        &self.roots()[self.default_index]
    }

    /// Resolves `path` (or `roots`) and checks them against each other and the data dir. Problems that should not
    /// stop MAP (a project folder that does not exist yet, the data dir inside it) become `warnings`.
    fn resolve(&mut self, base: &Path, data_dir: &Path, warnings: &mut Vec<String>) -> Result<()> {
        let roots = match (self.roots.take(), self.path.take()) {
            (Some(_), Some(_)) => bail!("[projects]: set `path` (one project folder) or `roots` (several), not both"),
            (Some(roots), None) => {
                if roots.is_empty() {
                    bail!("projects.roots needs at least one folder");
                }
                let mut out = Vec::with_capacity(roots.len());
                for mut r in roots {
                    r.name = r.name.trim().to_string();
                    if r.name.is_empty() || r.name.chars().count() > 40 || r.name.chars().any(char::is_control) {
                        bail!("projects.roots: every root needs a name of 1–40 characters (got `{}`)", r.name);
                    }
                    let path = expand_home(&r.path);
                    if !path.is_absolute() {
                        bail!("projects.roots `{}`: path `{}` must be absolute (or ~/…)", r.name, r.path.display());
                    }
                    r.path = path;
                    r.host_path = r.host_path.filter(|h| !h.as_os_str().is_empty());
                    if let Some(h) = &r.host_path
                        && !h.is_absolute()
                    {
                        bail!("projects.roots `{}`: host_path `{}` must be absolute", r.name, h.display());
                    }
                    out.push(r);
                }
                out
            }
            (None, root) => {
                // One project folder. Relative paths resolve against the config's folder, as `root` always did.
                if self.default_root.is_some() {
                    bail!("projects.default_root only applies to `roots`");
                }
                let path = expand_home(&root.unwrap_or_else(|| "~/project".into()));
                let path = if path.is_absolute() { path } else { base.join(path) };
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| "root".into());
                vec![ProjectRoot { name, path, host_path: None, allow_root_itself: false }]
            }
        };
        let data = crate::roots::normalize(data_dir);
        let data_real = data_dir.canonicalize().ok();
        for (i, r) in roots.iter().enumerate() {
            if r.path.components().any(|c| c == std::path::Component::ParentDir) {
                bail!("projects.roots `{}`: path `{}` must not contain `..`", r.name, r.path.display());
            }
            let path = crate::roots::normalize(&r.path);
            for other in &roots[..i] {
                if other.name.eq_ignore_ascii_case(&r.name) {
                    bail!("projects.roots: the name `{}` is used twice", r.name);
                }
                let o = crate::roots::normalize(&other.path);
                if path.starts_with(&o) || o.starts_with(&path) {
                    bail!("projects.roots `{}` ({}) and `{}` ({}) overlap: list each folder once", other.name, o.display(), r.name, path.display());
                }
            }
            let real = r.path.canonicalize().ok();
            let both = |f: fn(&Path, &Path) -> bool| f(&path, &data) || matches!((&real, &data_real), (Some(p), Some(d)) if f(p, d));
            if both(|p, d| p.starts_with(d)) {
                bail!("projects.roots `{}` ({}) is MAP's data dir or inside it ({})", r.name, path.display(), data.display());
            }
            if both(|p, d| d.starts_with(p)) {
                warnings.push(format!(
                    "project folder `{}` ({}) contains MAP's data dir {}: that folder is never listed or accepted as a project folder",
                    r.name,
                    path.display(),
                    data.display()
                ));
            }
            match &real {
                Some(p) if p.is_dir() => {}
                Some(_) => warnings.push(format!("project folder `{}` ({}) is not a folder: nothing can be picked in it", r.name, path.display())),
                None => warnings.push(format!("project folder `{}` ({}) does not exist yet: create it to pick folders in it", r.name, path.display())),
            }
        }
        self.default_index = match self.default_root.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => roots.iter().position(|r| r.name == name).with_context(|| {
                format!("projects.default_root `{name}` is not one of the roots ({})", roots.iter().map(|r| r.name.as_str()).collect::<Vec<_>>().join(", "))
            })?,
            None => 0,
        };
        self.default_root = Some(roots[self.default_index].name.clone());
        self.roots = Some(roots);
        Ok(())
    }
}

/// `~` or `~/…` becomes `$HOME/…`; other paths are unchanged.
pub fn expand_home(p: &Path) -> PathBuf {
    let Ok(rest) = p.strip_prefix("~") else { return p.to_path_buf() };
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(rest),
        None => p.to_path_buf(),
    }
}

/// How the initial prompt reaches an interactive agent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    /// Appended as the last command-line argument.
    Arg,
    /// Typed into the pane after `startup_secs`.
    #[default]
    Send,
}

/// A coding-agent CLI. Command templates substitute `{binary}`, `{model}`, `{session}` and (headless only) `{prompt}`.
#[derive(Debug, Clone, Default)]
pub struct ProviderSpec {
    pub binary: String,
    pub models: Vec<String>,
    pub default_model: String,
    pub interactive: Vec<String>,
    pub headless: Vec<String>,
    pub extra_args: Vec<String>,
    pub prompt_mode: PromptMode,
    pub startup_secs: u64,
    /// Pane text meaning the agent is waiting at its prompt.
    pub idle_markers: Vec<String>,
    /// Pane text meaning the agent is busy even if the pane did not change.
    pub working_markers: Vec<String>,
    pub auth_markers: Vec<String>,
    /// Pane text of the CLI's "trust this folder?" question (state `needs_trust`). Matched with runs of whitespace
    /// squashed, so a phrase the pane wraps onto two lines still counts.
    pub trust_markers: Vec<String>,
    /// Labels of the option in that question that trusts the folder; `POST /api/sessions/{name}/trust` moves the
    /// selection there and presses Enter.
    pub trust_accept: Vec<String>,
    /// Where the CLI records the folders it trusts; `None`: MAP cannot mark one and relies on `trust_markers`.
    pub trust: Option<TrustSpec>,
    /// Sign-in command run in a helper tmux session (`{binary}` substituted); empty: no web sign-in.
    pub login: Vec<String>,
    /// Text typed into the helper session after `startup_secs` (for CLIs that sign in from their TUI).
    pub login_send: Option<String>,
    /// Shown before a sign-in starts, e.g. when the CLI signs out the current account first.
    pub login_warning: Option<String>,
    /// Files the CLI keeps its credentials in (`~/` is the home directory). Only their existence and whether
    /// they parse are checked; empty: the sign-in state is unknown.
    pub credentials: Vec<String>,
    /// The environment variable that holds an API key instead of a sign-in.
    pub api_key_env: Option<String>,
    /// Prints the installed version (`{binary}` substituted); empty: the version is unknown.
    pub version: Vec<String>,
    /// Where the newest release's version is published; `None`: unknown.
    pub latest: Option<LatestSource>,
    /// Updates the CLI, run in a helper tmux session `<session_prefix>update-<name>`; empty: not from the dashboard.
    pub update: Vec<String>,
    /// The variable that relocates the CLI's state (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`; `HOME` for a CLI without
    /// one). In `[agents_home] mode = "isolated"` MAP sets it to `<agents_home.dir>/<isolated_dir>` for every launch.
    pub home_env: Option<String>,
    /// The CLI's state folder when `home_env` is not set (`~/.claude`). Paths MAP reads under it (credentials,
    /// skills, transcripts, usage logs, settings) follow the folder the CLI really uses ([`Config::provider_home`]).
    pub default_home: Option<String>,
    /// The state folder inside `home_env`'s folder: empty for `CLAUDE_CONFIG_DIR`, `.gemini` for `HOME`.
    pub home_subdir: Option<String>,
    /// The folder's name under `agents_home.dir`; default: the provider's name.
    pub isolated_dir: Option<String>,
    /// `VAR=~/path` exported next to `home_env` in isolated mode, so tools the agent runs keep their real folders
    /// when `home_env` is `HOME` (cargo, rustup, go). `~` is MAP's own `$HOME`; a value in MAP's environment wins.
    pub home_keep: Vec<String>,
    /// The folder `home_env` points at in shared mode too (`~/` is the home directory), for a provider that runs
    /// another provider's CLI and must never use that one's sign-in (`openrouter` runs `claude` with
    /// `CLAUDE_CONFIG_DIR=~/.claude-openrouter`). `None`: shared mode leaves `home_env` alone.
    pub shared_home: Option<String>,
    /// Variables exported for every run of the provider, after `[workspace.env]` (so they win over it). A value that is
    /// exactly `$NAME` is a reference: launch scripts say `export VAR="$NAME"`, so a secret's value is never written
    /// to disk, and commands MAP runs itself read `NAME` from `[workspace.env]` or MAP's environment.
    pub env: BTreeMap<String, String>,
}

/// The variable a provider `env` value refers to (`$OPENROUTER_API_KEY` → `OPENROUTER_API_KEY`); `None` for a literal.
pub fn env_ref(value: &str) -> Option<&str> {
    value.strip_prefix('$').filter(|v| valid_var(v))
}

/// Where a CLI records that its user trusts a folder (`backend/src/trust.rs` writes it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustSpec {
    pub format: TrustFormat,
    /// The file, unless `dir_env` is set; `~/` is the home directory.
    pub file: String,
    /// A variable naming the CLI's config folder (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`). When it is set in
    /// `[workspace.env]` (which wins) or in MAP's own environment, the file is `<that folder>/<dir_file>`.
    pub dir_env: Option<String>,
    pub dir_file: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustFormat {
    /// Claude Code's global config (`.claude.json`): `projects["<folder>"].hasTrustDialogAccepted = true`.
    ClaudeJson,
    /// Codex's `config.toml`: `[projects."<folder>"] trust_level = "trusted"`.
    CodexToml,
    /// `claude_json` plus `hasCompletedOnboarding = true` when the key is missing, for a Claude Code config folder
    /// that only MAP uses (the `openrouter` provider's): its first run would otherwise stop at the theme picker.
    ClaudeJsonOnboarded,
    /// A JSON config with a `trusted_folders` array of paths (GitHub Copilot CLI's `config.json`).
    TrustedFoldersJson,
}

/// `[providers.<name>.trust]`: each field overrides the built-in one; `file = ""` turns marking off.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct TrustOverride {
    format: Option<TrustFormat>,
    file: Option<String>,
    dir_env: Option<String>,
    dir_file: Option<String>,
}

impl TrustOverride {
    fn apply(self, base: Option<TrustSpec>, name: &str) -> Result<Option<TrustSpec>> {
        if self.file.as_deref() == Some("") {
            return Ok(None);
        }
        let merged = match base {
            Some(b) => TrustSpec {
                format: self.format.unwrap_or(b.format),
                file: self.file.unwrap_or(b.file),
                dir_env: optional(self.dir_env, b.dir_env),
                dir_file: optional(self.dir_file, b.dir_file),
            },
            None => {
                let (Some(format), Some(file)) = (self.format, self.file) else {
                    bail!("providers.{name}.trust needs `format` and `file`");
                };
                TrustSpec { format, file, dir_env: optional(self.dir_env, None), dir_file: optional(self.dir_file, None) }
            }
        };
        if merged.dir_env.is_some() != merged.dir_file.is_some() {
            bail!("providers.{name}.trust: set both `dir_env` and `dir_file`, or neither");
        }
        Ok(Some(merged))
    }
}

/// A URL that answers with the newest version, as plain text or inside JSON. Fetched without credentials.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatestSource {
    /// `{os}` (linux, darwin, windows), `{arch}` (amd64, arm64) and `{channel}` are substituted. `""` in an
    /// override clears the source.
    pub url: String,
    /// Dotted path to the version in a JSON body (`version`, `dist-tags.latest`); absent: the body is the version.
    #[serde(default)]
    pub json: Option<String>,
    /// Where `{channel}` comes from.
    #[serde(default)]
    pub channel: Option<ChannelSource>,
}

/// A release channel read from one key of the CLI's own JSON settings file (only that key is used).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelSource {
    /// `~/` is the home directory.
    pub file: String,
    /// Dotted path inside the file.
    pub key: String,
    /// When the file or the key is missing.
    pub default: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderOverride {
    binary: Option<String>,
    models: Option<Vec<String>>,
    default_model: Option<String>,
    interactive: Option<Vec<String>>,
    headless: Option<Vec<String>>,
    extra_args: Option<Vec<String>>,
    prompt_mode: Option<PromptMode>,
    startup_secs: Option<u64>,
    idle_markers: Option<Vec<String>>,
    working_markers: Option<Vec<String>>,
    auth_markers: Option<Vec<String>>,
    trust_markers: Option<Vec<String>>,
    trust_accept: Option<Vec<String>>,
    trust: Option<TrustOverride>,
    login: Option<Vec<String>>,
    login_send: Option<String>,
    login_warning: Option<String>,
    credentials: Option<Vec<String>>,
    api_key_env: Option<String>,
    version: Option<Vec<String>>,
    latest: Option<LatestSource>,
    update: Option<Vec<String>>,
    home_env: Option<String>,
    default_home: Option<String>,
    home_subdir: Option<String>,
    isolated_dir: Option<String>,
    home_keep: Option<Vec<String>>,
    shared_home: Option<String>,
    env: Option<BTreeMap<String, String>>,
}

/// An optional override: missing keeps the built-in value, `""` clears it.
fn optional(over: Option<String>, base: Option<String>) -> Option<String> {
    match over {
        Some(s) if s.is_empty() => None,
        Some(s) => Some(s),
        None => base,
    }
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

pub fn builtin_providers() -> BTreeMap<String, ProviderSpec> {
    let mut map = BTreeMap::new();
    map.insert(
        "claude".into(),
        ProviderSpec {
            binary: "claude".into(),
            models: strings(&["opus", "sonnet", "haiku", "fable"]),
            default_model: "sonnet".into(),
            interactive: strings(&["{binary}", "--name", "{session}", "--model", "{model}"]),
            headless: strings(&["{binary}", "-p", "{prompt}", "--model", "{model}"]),
            extra_args: vec![],
            prompt_mode: PromptMode::Arg,
            startup_secs: 4,
            idle_markers: strings(&["? for shortcuts"]),
            working_markers: strings(&["esc to interrupt"]),
            auth_markers: strings(&["Login expired", "Please run /login", "needs you to sign in again", "OAuth session expired", "Failed to authenticate"]),
            // Claude Code 2.1: "Quick safety check: Is this a project you created or one you trust? …", options
            // "No, exit" (selected first) and "Yes, I trust this folder".
            trust_markers: strings(&["Is this a project you created or one you trust", "Yes, I trust this folder", "Do you trust the files in this folder"]),
            trust_accept: strings(&["Yes, I trust this folder"]),
            trust: Some(TrustSpec {
                format: TrustFormat::ClaudeJson,
                file: "~/.claude.json".into(),
                dir_env: Some("CLAUDE_CONFIG_DIR".into()),
                dir_file: Some(".claude.json".into()),
            }),
            login: strings(&["{binary}", "auth", "login"]),
            login_send: None,
            login_warning: None,
            credentials: strings(&["~/.claude/.credentials.json"]),
            api_key_env: Some("ANTHROPIC_API_KEY".into()),
            version: strings(&["{binary}", "--version"]),
            // The channel Claude Code itself follows (`autoUpdatesChannel`: `latest` or `stable`).
            latest: Some(LatestSource {
                url: "https://downloads.claude.ai/claude-code-releases/{channel}".into(),
                json: None,
                channel: Some(ChannelSource { file: "~/.claude/settings.json".into(), key: "autoUpdatesChannel".into(), default: "latest".into() }),
            }),
            update: strings(&["{binary}", "update"]),
            home_env: Some("CLAUDE_CONFIG_DIR".into()),
            default_home: Some("~/.claude".into()),
            home_subdir: None,
            isolated_dir: None,
            home_keep: vec![],
            shared_home: None,
            env: BTreeMap::new(),
        },
    );
    map.insert(
        "codex".into(),
        ProviderSpec {
            binary: "codex".into(),
            models: strings(&["gpt-6-sol", "gpt-6-astra", "gpt-6-luna"]),
            default_model: "gpt-6-sol".into(),
            interactive: strings(&["{binary}", "-m", "{model}"]),
            headless: strings(&["{binary}", "exec", "-m", "{model}", "{prompt}"]),
            extra_args: vec![],
            prompt_mode: PromptMode::Arg,
            startup_secs: 4,
            idle_markers: strings(&["? for shortcuts"]),
            working_markers: strings(&["esc to interrupt", "Working"]),
            auth_markers: strings(&["Not logged in", "sign in with ChatGPT"]),
            // codex-cli 0.159: "Trust this folder? Codex can read, edit, and run files here, …" with "Trust and
            // continue" / "Open restricted"; older releases asked "Do you trust the contents of this directory?".
            trust_markers: strings(&["Trust this folder? Codex can", "Trust and continue", "Do you trust the contents of this directory", "allow Codex to work in this folder"]),
            trust_accept: strings(&["Trust and continue", "Yes, continue", "Yes, allow Codex to work in this folder"]),
            trust: Some(TrustSpec {
                format: TrustFormat::CodexToml,
                file: "~/.codex/config.toml".into(),
                dir_env: Some("CODEX_HOME".into()),
                dir_file: Some("config.toml".into()),
            }),
            login: strings(&["{binary}", "login", "--device-auth"]),
            login_send: None,
            // Seen with codex-cli 0.159: the device flow deletes ~/.codex/auth.json as soon as it starts.
            login_warning: Some("codex login signs out the current account as soon as it starts, before you approve the new one.".into()),
            credentials: strings(&["~/.codex/auth.json"]),
            api_key_env: Some("OPENAI_API_KEY".into()),
            version: strings(&["{binary}", "--version"]),
            latest: Some(LatestSource { url: "https://registry.npmjs.org/@openai/codex/latest".into(), json: Some("version".into()), channel: None }),
            update: strings(&["{binary}", "update"]),
            home_env: Some("CODEX_HOME".into()),
            default_home: Some("~/.codex".into()),
            home_subdir: None,
            isolated_dir: None,
            home_keep: vec![],
            shared_home: None,
            env: BTreeMap::new(),
        },
    );
    map.insert(
        "gemini".into(),
        ProviderSpec {
            binary: "agy".into(),
            models: strings(&["gemini-3.8-pro-high", "gemini-3.8-flash-high"]),
            default_model: "gemini-3.8-flash-high".into(),
            interactive: strings(&["{binary}", "--model", "{model}"]),
            headless: strings(&["{binary}", "-p", "{prompt}", "--model", "{model}"]),
            extra_args: vec![],
            prompt_mode: PromptMode::Send,
            startup_secs: 6,
            idle_markers: strings(&["Type your message"]),
            working_markers: strings(&["esc to cancel"]),
            auth_markers: strings(&["Login required", "Please sign in"]),
            // agy 1.3: "Do you trust the contents of this project?" / "Yes, I trust this folder". It keeps the answer
            // in `trustedWorkspaces` of ~/.gemini/antigravity-cli/settings.json, in a format MAP has not confirmed,
            // so MAP does not write it: the question shows as `needs_trust`.
            trust_markers: strings(&["Do you trust the contents of this project", "Yes, I trust this folder"]),
            trust_accept: strings(&["Yes, I trust this folder"]),
            trust: None,
            // agy signs in from its TUI (`/login`); it may keep the token in the system keyring instead of this file.
            login: strings(&["{binary}"]),
            login_send: Some("/login".into()),
            login_warning: None,
            credentials: strings(&["~/.gemini/jetski-standalone-oauth-token"]),
            api_key_env: Some("GEMINI_API_KEY".into()),
            version: strings(&["{binary}", "--version"]),
            latest: Some(LatestSource {
                url: "https://antigravity-cli-auto-updater-974169037036.us-central1.run.app/manifests/{os}_{arch}.json".into(),
                json: Some("version".into()),
                channel: None,
            }),
            update: strings(&["{binary}", "update"]),
            // agy 1.3 has no variable for its state folder (~/.gemini: sign-in token, settings, conversations), so
            // isolated mode gives it a HOME of its own. It may also keep a token in the system keyring, which HOME
            // does not move.
            home_env: Some("HOME".into()),
            default_home: Some("~/.gemini".into()),
            home_subdir: Some(".gemini".into()),
            isolated_dir: Some("gemini-home".into()),
            home_keep: strings(&["CARGO_HOME=~/.cargo", "RUSTUP_HOME=~/.rustup", "GOPATH=~/go"]),
            shared_home: None,
            env: BTreeMap::new(),
        },
    );
    // Cursor CLI (https://cursor.com/docs/cli/overview). Installed as `agent` and `cursor-agent`; the longer name is
    // the one no other tool uses. None of the pane strings below were captured from a real pane: they come from
    // Cursor's docs and forum, and a `[providers.cursor]` override fixes them.
    map.insert(
        "cursor".into(),
        ProviderSpec {
            binary: "cursor-agent".into(),
            // `cursor-agent --list-models` prints the ids this account can use; add them to `models` in map.toml.
            models: strings(&["auto"]),
            default_model: "auto".into(),
            interactive: strings(&["{binary}", "--model", "{model}"]),
            // Print mode proposes edits only (no --force); --trust: print mode never asks about the folder.
            headless: strings(&["{binary}", "-p", "--output-format", "text", "--trust", "--model", "{model}", "{prompt}"]),
            extra_args: vec![],
            prompt_mode: PromptMode::Arg,
            startup_secs: 5,
            idle_markers: strings(&["Plan, search, build anything"]),
            working_markers: vec![],
            auth_markers: strings(&["Not logged in", "Authentication required", "Please log in", "run `agent login`", "run `cursor-agent login`"]),
            // "Workspace Trust Required … Do you want to trust this workspace?" with "Trust this workspace". Where the
            // CLI keeps trusted workspaces is not documented, so MAP does not write it: the question shows as
            // `needs_trust`.
            trust_markers: strings(&["Workspace Trust Required", "Do you want to trust this workspace", "Trust this workspace"]),
            trust_accept: strings(&["Trust this workspace"]),
            trust: None,
            // A browser sign-in; NO_OPEN_BROWSER prints the URL instead of opening a browser on the server.
            login: strings(&["env", "NO_OPEN_BROWSER=1", "{binary}", "login"]),
            login_send: None,
            login_warning: None,
            // Where the sign-in is kept is not documented (a keyring where there is one): the state is unknown.
            credentials: vec![],
            api_key_env: Some("CURSOR_API_KEY".into()),
            version: strings(&["{binary}", "--version"]),
            latest: None,
            update: strings(&["{binary}", "update"]),
            // CURSOR_CONFIG_DIR moves ~/.cursor/cli-config.json, but not provably the sign-in, so isolated mode gives
            // the CLI a HOME of its own, as for agy.
            home_env: Some("HOME".into()),
            default_home: Some("~/.cursor".into()),
            home_subdir: Some(".cursor".into()),
            isolated_dir: Some("cursor-home".into()),
            home_keep: strings(&["CARGO_HOME=~/.cargo", "RUSTUP_HOME=~/.rustup", "GOPATH=~/go"]),
            shared_home: None,
            env: BTreeMap::new(),
        },
    );
    // GitHub Copilot CLI (https://docs.github.com/en/copilot/how-tos/copilot-cli), npm @github/copilot.
    map.insert(
        "copilot".into(),
        ProviderSpec {
            binary: "copilot".into(),
            models: strings(&["auto", "claude-opus-5.5", "claude-sonnet-4.6", "claude-haiku-4.5", "gpt-6-astra", "gpt-5.6-luna", "gpt-5.4-mini"]),
            default_model: "auto".into(),
            interactive: strings(&["{binary}", "--model", "{model}"]),
            // -s: the answer only. Without --allow-all-tools a headless run uses no tool that needs approval.
            headless: strings(&["{binary}", "-p", "{prompt}", "--model", "{model}", "-s", "--no-ask-user"]),
            extra_args: vec![],
            // The first prompt is `-i <prompt>`, which a session without one cannot leave dangling: typed in instead.
            prompt_mode: PromptMode::Send,
            startup_secs: 6,
            idle_markers: vec![],
            working_markers: strings(&["Esc to cancel"]),
            auth_markers: strings(&["/login to authenticate", "Not authenticated", "No authentication information found", "Please run /login"]),
            // "… You should only proceed if you trust the files in this location." with "1. Yes", "2. Yes, and
            // remember this folder for future sessions", "3. No (Esc)".
            trust_markers: strings(&["only proceed if you trust the files in this location", "Do you trust the files in this folder"]),
            trust_accept: strings(&["Yes, and remember this folder"]),
            trust: Some(TrustSpec {
                format: TrustFormat::TrustedFoldersJson,
                file: "~/.copilot/config.json".into(),
                dir_env: Some("COPILOT_HOME".into()),
                dir_file: Some("config.json".into()),
            }),
            login: strings(&["{binary}", "login", "--device-code"]),
            login_send: None,
            login_warning: None,
            // The token is in the system keychain (or, without one, in config.json next to other state), so a file
            // cannot tell: the state is unknown unless the variable below is set.
            credentials: vec![],
            api_key_env: Some("COPILOT_GITHUB_TOKEN".into()),
            version: strings(&["{binary}", "--version"]),
            latest: Some(LatestSource { url: "https://registry.npmjs.org/@github/copilot/latest".into(), json: Some("version".into()), channel: None }),
            update: strings(&["{binary}", "update"]),
            home_env: Some("COPILOT_HOME".into()),
            default_home: Some("~/.copilot".into()),
            home_subdir: None,
            isolated_dir: None,
            home_keep: vec![],
            shared_home: None,
            env: BTreeMap::new(),
        },
    );
    // OpenRouter (https://openrouter.ai/docs/guides/guides/coding-agents): Claude Code against OpenRouter's
    // Anthropic-compatible endpoint, signed in by the owner's OPENROUTER_API_KEY. Its own Claude config folder, in
    // shared mode too, so it never uses (or signs out) the `claude` provider's account.
    let mut openrouter = map["claude"].clone();
    openrouter.models = strings(&[
        "~anthropic/claude-sonnet-latest",
        "~anthropic/claude-opus-latest",
        "~anthropic/claude-haiku-latest",
        "anthropic/claude-sonnet-5.5",
        "anthropic/claude-opus-5.5",
        "openai/gpt-6.1-sol",
        "google/gemini-4-argon",
        "moonshotai/kimi-k3",
    ]);
    openrouter.default_model = "~anthropic/claude-sonnet-latest".into();
    openrouter.auth_markers.extend(strings(&["Invalid API key", "No auth credentials found", "User not found"]));
    openrouter.trust = Some(TrustSpec {
        format: TrustFormat::ClaudeJsonOnboarded,
        file: "~/.claude-openrouter/.claude.json".into(),
        dir_env: Some("CLAUDE_CONFIG_DIR".into()),
        dir_file: Some(".claude.json".into()),
    });
    // No sign-in: the key is the sign-in.
    openrouter.login = vec![];
    openrouter.credentials = vec![];
    openrouter.api_key_env = Some("OPENROUTER_API_KEY".into());
    // The same `claude` binary: the claude provider announces and runs its updates.
    openrouter.latest = None;
    openrouter.update = vec![];
    openrouter.default_home = Some("~/.claude-openrouter".into());
    openrouter.shared_home = Some("~/.claude-openrouter".into());
    openrouter.env = [
        ("ANTHROPIC_BASE_URL", "https://openrouter.ai/api"),
        ("ANTHROPIC_AUTH_TOKEN", "$OPENROUTER_API_KEY"),
        // Must be set and empty, or Claude Code may send an Anthropic key to OpenRouter.
        ("ANTHROPIC_API_KEY", ""),
        // The models behind Claude Code's own aliases (sub-agents, background calls).
        ("ANTHROPIC_DEFAULT_OPUS_MODEL", "~anthropic/claude-opus-latest"),
        ("ANTHROPIC_DEFAULT_SONNET_MODEL", "~anthropic/claude-sonnet-latest"),
        ("ANTHROPIC_DEFAULT_HAIKU_MODEL", "~anthropic/claude-haiku-latest"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    map.insert("openrouter".into(), openrouter);

    map
}

impl ProviderOverride {
    fn apply(self, base: Option<ProviderSpec>, name: &str) -> Result<ProviderSpec> {
        let base = match base {
            Some(base) => base,
            None => {
                let (Some(binary), Some(interactive)) = (self.binary.clone(), self.interactive.clone()) else {
                    bail!("provider `{name}` is not built in, so it needs `binary` and `interactive`");
                };
                let model = self.default_model.clone().unwrap_or_default();
                ProviderSpec {
                    binary,
                    models: if model.is_empty() { vec![] } else { vec![model.clone()] },
                    default_model: model,
                    interactive,
                    startup_secs: 4,
                    ..Default::default()
                }
            }
        };
        Ok(ProviderSpec {
            binary: self.binary.unwrap_or(base.binary),
            models: self.models.unwrap_or(base.models),
            default_model: self.default_model.unwrap_or(base.default_model),
            interactive: self.interactive.unwrap_or(base.interactive),
            headless: self.headless.unwrap_or(base.headless),
            extra_args: self.extra_args.unwrap_or(base.extra_args),
            prompt_mode: self.prompt_mode.unwrap_or(base.prompt_mode),
            startup_secs: self.startup_secs.unwrap_or(base.startup_secs),
            idle_markers: self.idle_markers.unwrap_or(base.idle_markers),
            working_markers: self.working_markers.unwrap_or(base.working_markers),
            auth_markers: self.auth_markers.unwrap_or(base.auth_markers),
            trust_markers: self.trust_markers.unwrap_or(base.trust_markers),
            trust_accept: self.trust_accept.unwrap_or(base.trust_accept),
            trust: match self.trust {
                Some(t) => t.apply(base.trust, name)?,
                None => base.trust,
            },
            login: self.login.unwrap_or(base.login),
            login_send: optional(self.login_send, base.login_send),
            login_warning: optional(self.login_warning, base.login_warning),
            credentials: self.credentials.unwrap_or(base.credentials),
            api_key_env: optional(self.api_key_env, base.api_key_env),
            version: self.version.unwrap_or(base.version),
            latest: match self.latest {
                Some(l) if l.url.is_empty() => None,
                Some(l) => Some(l),
                None => base.latest,
            },
            update: self.update.unwrap_or(base.update),
            home_env: optional(self.home_env, base.home_env),
            default_home: optional(self.default_home, base.default_home),
            home_subdir: optional(self.home_subdir, base.home_subdir),
            isolated_dir: optional(self.isolated_dir, base.isolated_dir),
            home_keep: self.home_keep.unwrap_or(base.home_keep),
            shared_home: optional(self.shared_home, base.shared_home),
            env: self.env.unwrap_or(base.env),
        })
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new("."));
        let mut config = Self::parse_in(&text, base, Some(base))?;
        config.path = Some(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()));
        Ok(config)
    }

    /// Relative paths in the file resolve against `base` (the config file's directory).
    #[cfg(test)]
    pub fn parse(text: &str, base: &Path) -> Result<Self> {
        Self::parse_in(text, base, None)
    }

    /// [`Config::parse`]; `config_dir` is the folder of the file it came from, where `agents_home.dir` defaults to.
    fn parse_in(text: &str, base: &Path, config_dir: Option<&Path>) -> Result<Self> {
        let mut config: Config = toml::from_str(text).context("parsing config")?;
        let mut providers = builtin_providers();
        for (name, over) in std::mem::take(&mut config.provider_overrides) {
            let spec = over.apply(providers.remove(&name), &name)?;
            providers.insert(name, spec);
        }
        config.providers = providers;

        let resolve = |p: &Path| if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
        config.server.static_dir = resolve(&config.server.static_dir);
        config.server.data_dir = resolve(&config.server.data_dir);
        config.workspace.dir = resolve(&config.workspace.dir);
        let home = |p: &Path| resolve(&expand_home(p));
        let skills = &mut config.skills;
        for dir in skills.claude_user_dirs.iter_mut().chain(skills.claude_plugin_dirs.iter_mut()).chain(skills.codex_user_dirs.iter_mut()) {
            *dir = home(dir);
        }
        skills.claude_settings = home(&skills.claude_settings);
        skills.claude_transcripts = home(&skills.claude_transcripts);
        if let Some(dir) = skills.claude_project_dirs.iter().chain(&skills.codex_project_dirs).chain(skills.also_loaded_by.values().flatten()).find(|d| d.is_absolute() || d.components().any(|c| c == std::path::Component::ParentDir)) {
            bail!("skills project dir `{}` must be relative to the workspace and stay inside it", dir.display());
        }
        let agents = &mut config.agents;
        for dir in agents.claude_user_dirs.iter_mut().chain(agents.codex_user_dirs.iter_mut()).chain(agents.gemini_user_dirs.iter_mut()) {
            *dir = home(dir);
        }
        let project_dirs = [&agents.claude_project_dirs, &agents.codex_project_dirs, &agents.gemini_project_dirs];
        if let Some(dir) = project_dirs.iter().flat_map(|d| d.iter()).find(|d| d.is_absolute() || d.as_os_str().is_empty() || d.components().any(|c| c == std::path::Component::ParentDir)) {
            bail!("agents project dir `{}` must be relative to the workspace and stay inside it", dir.display());
        }
        config.reflex.api_url = config.reflex.api_url.take().map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
        if let Some(url) = &config.reflex.api_url
            && !(url.starts_with("https://") || url.starts_with("http://"))
        {
            bail!("reflex.api_url must start with https:// or http://");
        }
        if !valid_var(&config.reflex.api_key_env) {
            bail!("reflex.api_key_env `{}` is not a valid variable name", config.reflex.api_key_env);
        }
        if config.reflex.binary.contains('/') {
            config.reflex.binary = resolve(Path::new(&config.reflex.binary)).to_string_lossy().into_owned();
        }

        let templates = &mut config.templates;
        for dir in &mut templates.dirs {
            *dir = home(dir);
        }
        if let Some(path) = &mut templates.init_agent {
            *path = home(path);
        }
        if let Some(path) = &mut templates.planner_agent {
            *path = home(path);
        }

        let mut warnings = vec![];
        config.projects.resolve(base, &config.server.data_dir, &mut warnings)?;
        config.warnings.extend(warnings);
        config.projects.git_timeout_secs = config.projects.git_timeout_secs.clamp(5, 3600);

        let usage = &mut config.usage;
        for p in [&mut usage.claude_credentials, &mut usage.claude_projects, &mut usage.codex_sessions, &mut usage.codex_auth] {
            *p = resolve(&expand_home(p));
        }

        config.agents_home.resolved = match config.agents_home.dir.take() {
            Some(dir) => home(&dir),
            None => match config_dir {
                // `map.toml` given without a folder: its parent is "", the working directory.
                Some(dir) => {
                    let dir = if dir.as_os_str().is_empty() { Path::new(".") } else { dir };
                    std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf()).join("agent-home")
                }
                None => config.server.data_dir.join("agent-home"),
            },
        };
        for (name, spec) in &config.providers {
            if let Some(var) = &spec.home_env
                && !valid_var(var)
            {
                bail!("providers.{name}.home_env `{var}` is not a valid variable name");
            }
            if let Some(dir) = &spec.isolated_dir
                && !matches!(Path::new(dir).components().collect::<Vec<_>>().as_slice(), [std::path::Component::Normal(_)])
            {
                bail!("providers.{name}.isolated_dir `{dir}` must be one folder name");
            }
            if let Some(keep) = spec.home_keep.iter().find(|k| !k.split_once('=').is_some_and(|(k, v)| valid_var(k) && !v.is_empty())) {
                bail!("providers.{name}.home_keep `{keep}` must be VAR=value");
            }
            if let Some(k) = spec.env.keys().find(|k| !valid_var(k)) {
                bail!("providers.{name}.env key `{k}` is not a valid variable name");
            }
            if let Some((k, v)) = spec.env.iter().find(|(_, v)| v.starts_with('$') && env_ref(v).is_none()) {
                bail!("providers.{name}.env.{k} = `{v}`: a reference is `$NAME` and nothing else");
            }
            if let Some((k, _)) = spec.env.iter().find(|(_, v)| v.contains('\0')) {
                bail!("providers.{name}.env.{k} contains a NUL byte");
            }
            if config.agents_home.isolated() && spec.home_env.is_none() {
                config.warnings.push(format!("provider `{name}` has no home_env, so in isolated mode it still uses its usual folders and sign-in"));
            }
        }
        config.rebase_provider_paths();
        if let Some(sock) = config.workspace.tmux_socket.take().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) {
            if !sock.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) || sock.starts_with('.') {
                bail!("workspace.tmux_socket `{sock}` may use only letters, digits, -, _ and . (it is a name, not a path)");
            }
            config.workspace.tmux_socket = Some(sock);
        }
        let usage = &mut config.usage;
        if !(usage.skip_pct > 0.0 && usage.skip_pct <= 100.0) {
            bail!("usage.skip_pct must be above 0 and at most 100");
        }
        usage.interval_secs = usage.interval_secs.max(30);
        for (name, p) in &usage.providers {
            if !config.providers.contains_key(name) {
                bail!("unknown provider `{name}` in [usage.providers]");
            }
            if let Some(r) = &p.reader
                && !matches!(r.as_str(), "claude" | "codex" | "openrouter" | "none")
            {
                bail!("usage.providers.{name}.reader must be `claude`, `codex`, `openrouter` or `none`");
            }
        }

        config.updates.interval_secs = config.updates.interval_secs.max(300);
        config.updates.timeout_secs = config.updates.timeout_secs.clamp(1, 300);
        for (name, spec) in &config.providers {
            if let Some(l) = &spec.latest
                && !(l.url.starts_with("https://") || l.url.starts_with("http://"))
            {
                bail!("providers.{name}.latest.url must start with https:// or http://");
            }
        }

        if config.server.api_token.as_deref() == Some("") {
            config.server.api_token = None;
        }
        if !config.workspace.session_prefix.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || config.workspace.session_prefix.is_empty() {
            bail!("workspace.session_prefix must be non-empty and use only letters, digits, - or _");
        }
        if let Some(key) = config.workspace.env.keys().find(|k| !valid_var(k)) {
            bail!("workspace.env key `{key}` is not a valid variable name");
        }
        if config.orchestrator.max_parallel == 0 {
            bail!("orchestrator.max_parallel must be at least 1");
        }
        for name in config.orchestrator.provider_order.iter().chain(&config.orchestrator.audit.chain) {
            if !config.providers.contains_key(name) {
                bail!("unknown provider `{name}` in orchestrator settings");
            }
        }
        let tiers = &mut config.orchestrator.tiers;
        for (name, t) in tiers.iter() {
            let spec = config.providers.get(name).with_context(|| format!("unknown provider `{name}` in [orchestrator.tiers]"))?;
            for (tier, model) in [("light", &t.light), ("standard", &t.standard), ("heavy", &t.heavy)] {
                if let Some(m) = model.as_deref().filter(|m| !m.is_empty())
                    && !spec.models.is_empty()
                    && !spec.models.iter().any(|x| x == m)
                {
                    bail!("orchestrator.tiers.{name}.{tier}: model `{m}` is not in {name}'s models ({})", spec.models.join(", "));
                }
            }
        }
        // The built-in claude tiers, without any model a changed `models` list no longer has.
        if !tiers.contains_key("claude")
            && let Some(spec) = config.providers.get("claude")
        {
            let keep = |m: Option<String>| m.filter(|m| spec.models.is_empty() || spec.models.contains(m));
            let b = builtin_claude_tiers();
            tiers.insert("claude".into(), TierModels { light: keep(b.light), standard: keep(b.standard), heavy: keep(b.heavy) });
        }
        Ok(config)
    }

    /// The folder `providers.<name>.home_env` points the CLI at: a value in `[workspace.env]` wins, then the
    /// isolated folder (`[agents_home] mode = "isolated"`), then MAP's own environment. `None`: the variable is not
    /// set, so the CLI uses its `default_home`. A provider with a `shared_home` always gets its own folder (the
    /// isolated one, else `shared_home`), whatever `[workspace.env]` says, so it never shares another's sign-in.
    pub fn provider_home_var(&self, name: &str) -> Option<PathBuf> {
        let spec = self.providers.get(name)?;
        let var = spec.home_env.as_deref()?;
        if let Some(home) = &spec.shared_home {
            return Some(if self.agents_home.isolated() { self.isolated_folder(name, spec) } else { expand_home(Path::new(home)) });
        }
        if let Some(v) = self.workspace.env.get(var).filter(|v| !v.is_empty()) {
            return Some(expand_home(Path::new(v)));
        }
        if self.agents_home.isolated() {
            return Some(self.isolated_folder(name, spec));
        }
        std::env::var_os(var).filter(|v| !v.is_empty()).map(|v| expand_home(Path::new(&v)))
    }

    /// The `home_env` assignment MAP makes for every run of the provider, if any (`CLAUDE_CONFIG_DIR=/…`): the
    /// isolated folder, or a `shared_home`. For Settings → Agent CLIs.
    pub fn provider_home_export(&self, name: &str) -> Option<(String, String)> {
        let spec = self.providers.get(name)?;
        let var = spec.home_env.as_deref()?;
        if spec.shared_home.is_some() {
            return self.provider_home_var(name).map(|d| (var.to_string(), d.display().to_string()));
        }
        self.provider_env(name).into_iter().find(|(k, _)| k == var)
    }

    /// The CLI's state folder as the CLI will see it (`~/.claude`, `<agents_home.dir>/claude`,
    /// `<agents_home.dir>/gemini-home/.gemini`); `None` for a provider with neither `home_env` nor `default_home`.
    /// Every path MAP reads under a provider's `default_home` follows this folder ([`Config::parse`] moves them).
    pub fn provider_home(&self, name: &str) -> Option<PathBuf> {
        let spec = self.providers.get(name)?;
        match self.provider_home_var(name) {
            Some(dir) => Some(match spec.home_subdir.as_deref().filter(|s| !s.is_empty()) {
                Some(sub) => dir.join(sub),
                None => dir,
            }),
            None => spec.default_home.as_deref().map(|h| expand_home(Path::new(h))),
        }
    }

    /// What every launch of the provider exports before `[workspace.env]`: in isolated mode its `home_env` (unless
    /// `[workspace.env]` sets that variable itself) and its `home_keep`; nothing in shared mode. A provider with a
    /// `shared_home` gets its folder after `[workspace.env]` instead ([`Config::provider_fixed_env`]).
    pub fn provider_env(&self, name: &str) -> Vec<(String, String)> {
        let Some(spec) = self.providers.get(name) else { return vec![] };
        let Some(var) = spec.home_env.as_deref() else { return vec![] };
        if spec.shared_home.is_some() || !self.agents_home.isolated() || self.workspace.env.get(var).is_some_and(|v| !v.is_empty()) {
            return vec![];
        }
        let mut env = vec![(var.to_string(), self.isolated_folder(name, spec).display().to_string())];
        for (k, v) in spec.home_keep.iter().filter_map(|k| k.split_once('=')) {
            if self.workspace.env.contains_key(k) {
                continue;
            }
            let v = std::env::var(k).ok().filter(|v| !v.is_empty()).unwrap_or_else(|| expand_home(Path::new(v)).display().to_string());
            env.push((k.to_string(), v));
        }
        env
    }

    /// A variable's value as the commands MAP runs itself see it: `[workspace.env]` first, then MAP's environment.
    pub fn env_value(&self, var: &str) -> Option<String> {
        self.workspace.env.get(var).cloned().filter(|v| !v.is_empty()).or_else(|| std::env::var(var).ok().filter(|v| !v.is_empty()))
    }

    /// The provider's `env` with every `$NAME` reference read from [`Config::env_value`] (missing: empty), for the
    /// commands MAP runs itself (headless fallback, audit, version check), after `[workspace.env]`. Never written
    /// anywhere: launch scripts get [`Config::provider_script_env`] instead.
    pub fn provider_fixed_env(&self, name: &str) -> Vec<(String, String)> {
        let Some(spec) = self.providers.get(name) else { return vec![] };
        let mut env: Vec<(String, String)> = self.shared_home_export(name).into_iter().collect();
        env.extend(spec.env.iter().map(|(k, v)| (k.clone(), match env_ref(v) { Some(r) => self.env_value(r).unwrap_or_default(), None => v.clone() })));
        env
    }

    /// A `shared_home` provider's `home_env` and folder, exported after `[workspace.env]` so it wins.
    fn shared_home_export(&self, name: &str) -> Option<(String, String)> {
        let spec = self.providers.get(name)?;
        spec.shared_home.as_ref()?;
        Some((spec.home_env.clone()?, self.provider_home_var(name)?.display().to_string()))
    }

    /// The provider's `env` as launch-script lines, after `[workspace.env]`: a literal is shell-quoted, a `$NAME`
    /// reference stays a reference (`export ANTHROPIC_AUTH_TOKEN="$OPENROUTER_API_KEY"`), so its value never reaches
    /// the script on disk. A reference that is empty when the script runs prints a warning in the pane.
    pub fn provider_script_env(&self, name: &str) -> Vec<String> {
        let Some(spec) = self.providers.get(name) else { return vec![] };
        let mut lines: Vec<String> = self.shared_home_export(name).map(|(k, v)| format!("export {k}={}", shlex::try_quote(&v).map(|q| q.into_owned()).unwrap_or_else(|_| "''".into()))).into_iter().collect();
        for (k, v) in &spec.env {
            match env_ref(v) {
                Some(r) => {
                    lines.push(format!("[ -n \"${{{r}:-}}\" ] || echo \"map: {r} is not set in this session's environment ({k} needs it)\" >&2"));
                    lines.push(format!("export {k}=\"${r}\""));
                }
                None => lines.push(format!("export {k}={}", shlex::try_quote(v).map(|q| q.into_owned()).unwrap_or_else(|_| "''".into()))),
            }
        }
        lines
    }

    fn isolated_folder(&self, name: &str, spec: &ProviderSpec) -> PathBuf {
        self.agents_home.dir().join(spec.isolated_dir.as_deref().unwrap_or(name))
    }

    /// In isolated mode, creates `agents_home.dir` and each provider's folders (mode 0700) where missing, and returns
    /// the provider folders. Nothing is copied into them: each provider is signed in there once.
    pub fn create_agent_homes(&self) -> Result<Vec<PathBuf>> {
        if !self.agents_home.isolated() {
            return Ok(vec![]);
        }
        let mut dirs = vec![self.agents_home.dir().to_path_buf()];
        for name in self.providers.keys() {
            for dir in [self.provider_home_var(name), self.provider_home(name)].into_iter().flatten() {
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }
        for dir in &dirs {
            if dir.is_dir() {
                continue;
            }
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).with_context(|| format!("chmod 700 {}", dir.display()))?;
            }
        }
        Ok(dirs.split_off(1))
    }

    /// Moves every configured path under a provider's `default_home` (credentials, skills and agents folders,
    /// transcripts, usage logs, the release-channel file) to the same place under [`Config::provider_home`]. A no-op
    /// in shared mode unless the CLI's `home_env` is set.
    fn rebase_provider_paths(&mut self) {
        let moves: Vec<(PathBuf, PathBuf)> = self
            .providers
            .iter()
            .filter_map(|(name, spec)| {
                let from = expand_home(Path::new(spec.default_home.as_deref()?));
                let to = self.provider_home(name)?;
                (from != to).then_some((from, to))
            })
            .collect();
        if moves.is_empty() {
            return;
        }
        let rebase = |p: &mut PathBuf| {
            if let Some((from, to)) = moves.iter().find(|(from, _)| p.starts_with(from)) {
                let rest = p.strip_prefix(from).expect("starts_with").to_path_buf();
                *p = if rest.as_os_str().is_empty() { to.clone() } else { to.join(rest) };
            }
        };
        let s = &mut self.skills;
        for p in s.claude_user_dirs.iter_mut().chain(s.claude_plugin_dirs.iter_mut()).chain(s.codex_user_dirs.iter_mut()) {
            rebase(p);
        }
        rebase(&mut s.claude_settings);
        rebase(&mut s.claude_transcripts);
        let a = &mut self.agents;
        for p in a.claude_user_dirs.iter_mut().chain(a.codex_user_dirs.iter_mut()).chain(a.gemini_user_dirs.iter_mut()) {
            rebase(p);
        }
        let u = &mut self.usage;
        for p in [&mut u.claude_credentials, &mut u.claude_projects, &mut u.codex_sessions, &mut u.codex_auth] {
            rebase(p);
        }
        let rebase_str = |s: &mut String| {
            let before = expand_home(Path::new(s.as_str()));
            let mut p = before.clone();
            rebase(&mut p);
            if p != before {
                *s = p.display().to_string();
            }
        };
        for spec in self.providers.values_mut() {
            spec.credentials.iter_mut().for_each(rebase_str);
            if let Some(ch) = spec.latest.as_mut().and_then(|l| l.channel.as_mut()) {
                rebase_str(&mut ch.file);
            }
        }
    }
}

/// For tests whose outcome depends on which CLIs are installed: Cursor, Copilot and OpenRouter (the `claude`
/// binary) as not installed, whatever this machine's PATH has.
#[cfg(test)]
pub const NO_NEW_CLIS: &str =
    "[providers.cursor]\nbinary = \"/nonexistent/cursor-agent\"\n[providers.copilot]\nbinary = \"/nonexistent/copilot\"\n[providers.openrouter]\nbinary = \"/nonexistent/claude\"\n";

/// A shell variable name: letters, digits and `_`, not starting with a digit.
fn valid_var(k: &str) -> bool {
    !k.is_empty() && k.chars().enumerate().all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_config_parses() {
        let c = Config::parse(include_str!("../../map.example.toml"), Path::new("/srv/map")).unwrap();
        assert_eq!(c.server.static_dir, PathBuf::from("/srv/map/frontend/out"));
        assert!(c.orchestrator.audit.enabled);
        assert_eq!(c.reflex.port_base, 7400);
        assert!(c.reflex.fallback_overhead_tokens.is_empty());
        assert_eq!(c.skills.context_files["codex"], "AGENTS.md");
        assert_eq!(c.usage.skip_pct, 90.0);
        assert_eq!(c.templates.dirs.len(), 1);
    }

    #[test]
    fn fallback_overhead_parses() {
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[reflex]\nfallback_overhead_tokens = { claude = 1200, api = 0 }\n", Path::new("/")).unwrap();
        assert_eq!(c.reflex.fallback_overhead_tokens.get("claude"), Some(&1200));
        assert_eq!(c.reflex.fallback_overhead_tokens.get("codex"), None);
    }

    #[test]
    fn template_paths_expand_home() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[templates]\ndirs = [\"~/t\", \"rel\"]\ninit_agent = \"agent.md\"\nplanner_agent = \"~/planner.md\"\n", Path::new("/etc/map")).unwrap();
        assert_eq!(c.templates.dirs, vec![home.join("t"), PathBuf::from("/etc/map/rel")]);
        assert_eq!(c.templates.init_agent, Some(PathBuf::from("/etc/map/agent.md")));
        assert_eq!(c.templates.planner_agent, Some(home.join("planner.md")));
        let c = Config::parse("[workspace]\ndir = \"/r\"\n", Path::new("/")).unwrap();
        assert_eq!(c.templates.dirs, vec![home.join(".config/map/templates")]);
    }

    fn projects_parse(projects: &str) -> Result<Config> {
        Config::parse(&format!("[workspace]\ndir = \"/r\"\n[server]\ndata_dir = \"/var/map/data\"\n[projects]\n{projects}\n"), Path::new("/etc/map"))
    }

    #[test]
    fn project_folder_path_root_and_back_compat() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        // Nothing set: ~/project, one root named after its last component.
        let c = projects_parse("").unwrap();
        let root = c.projects.default_root();
        assert_eq!((root.name.as_str(), &root.path, root.allow_root_itself, c.projects.git_timeout_secs), ("project", &home.join("project"), false, 300));
        // `path`, and the older `root` as the same key; relative paths resolve against the config's folder.
        let c = projects_parse(r#"path = "~/MAP/workspace""#).unwrap();
        assert_eq!((c.projects.roots().len(), &c.projects.default_root().path, c.projects.default_root().name.as_str()), (1, &home.join("MAP/workspace"), "workspace"));
        let c = projects_parse("root = \"/srv/code\"\ngit_timeout_secs = 1").unwrap();
        assert_eq!((&c.projects.default_root().path, c.projects.git_timeout_secs), (&PathBuf::from("/srv/code"), 5));
        assert_eq!(projects_parse(r#"path = "code""#).unwrap().projects.default_root().path, PathBuf::from("/etc/map/code"));
        assert!(projects_parse("path = \"/a\"\nroot = \"/b\"").is_err(), "path and root are one key");
        // A missing folder is a warning, not an error.
        let c = projects_parse(r#"path = "/nonexistent/map-projects""#).unwrap();
        assert!(c.warnings.iter().any(|w| w.contains("does not exist yet")), "{:?}", c.warnings);
        let docker = Config::parse(include_str!("../../deploy/map.docker.toml"), Path::new("/config")).unwrap();
        assert!(docker.projects.default_root().path.is_absolute());
    }

    #[test]
    fn project_roots_are_validated() {
        let c = projects_parse(
            r#"roots = [{ name = "Projects", path = "/srv/p" }, { name = "Docker", path = "/projects", host_path = "/home/me/project", allow_root_itself = true }]
default_root = "Docker""#,
        )
        .unwrap();
        assert_eq!(c.projects.roots().len(), 2);
        let d = c.projects.default_root();
        assert_eq!((d.name.as_str(), d.host_path.as_deref(), d.allow_root_itself), ("Docker", Some(Path::new("/home/me/project")), true));
        assert_eq!(projects_parse(r#"roots = [{ name = "A", path = "/a" }]"#).unwrap().projects.default_root().name, "A", "the first by default");
        for bad in [
            "roots = []",
            r#"roots = [{ name = " ", path = "/a" }]"#,
            r#"roots = [{ name = "A", path = "rel" }]"#,
            r#"roots = [{ name = "A", path = "/a/../b" }]"#,
            r#"roots = [{ name = "A", path = "/a" }, { name = "a", path = "/b" }]"#,
            r#"roots = [{ name = "A", path = "/a" }, { name = "B", path = "/a/b" }]"#,
            r#"roots = [{ name = "A", path = "/a", host_path = "rel" }]"#,
            r#"roots = [{ name = "A", path = "/var/map/data/p" }]"#,
            r#"roots = [{ name = "A", path = "/var/map/data" }]"#,
            "roots = [{ name = \"A\", path = \"/a\" }]\ndefault_root = \"B\"",
            "roots = [{ name = \"A\", path = \"/a\" }]\npath = \"/b\"",
            "path = \"/a\"\ndefault_root = \"a\"",
            r#"roots = [{ name = "A", path = "/a", extra = 1 }]"#,
        ] {
            assert!(projects_parse(bad).is_err(), "{bad}");
        }
        // MAP's data dir inside the project folder: allowed, with a warning (it is never listed or accepted).
        let c = projects_parse(r#"path = "/var/map""#).unwrap();
        assert!(c.warnings.iter().any(|w| w.contains("contains MAP's data dir")), "{:?}", c.warnings);
    }

    #[test]
    fn skills_paths_expand_home() {
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[skills]\nclaude_user_dirs = [\"~/sk\", \"rel\"]\n", Path::new("/etc/map")).unwrap();
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(c.skills.claude_user_dirs, vec![home.join("sk"), PathBuf::from("/etc/map/rel")]);
        assert_eq!(c.skills.claude_transcripts, home.join(".claude/projects"));
        assert!(Config::parse("[workspace]\ndir = \"/r\"\n[skills]\nclaude_project_dirs = [\"../x\"]\n", Path::new("/")).is_err());
        // Agents: the same rules, with defaults for every CLI.
        let c = Config::parse("[workspace]\ndir = \"/r\"\n", Path::new("/")).unwrap();
        assert_eq!(c.agents.gemini_user_dirs, vec![home.join(".gemini/config/agents")]);
        assert_eq!(c.agents.codex_project_dirs, vec![PathBuf::from(".codex/agents")]);
        assert!(Config::parse("[workspace]\ndir = \"/r\"\n[agents]\ngemini_project_dirs = [\"/abs\"]\n", Path::new("/")).is_err());
        assert!(Config::parse("[workspace]\ndir = \"/r\"\n[agents]\nclaude_project_dirs = [\"a/../../b\"]\n", Path::new("/")).is_err());
    }

    #[test]
    fn usage_paths_expand_home_and_settings_are_checked() {
        let home = std::env::var("HOME").unwrap();
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[usage]\ncodex_sessions = \"logs/codex\"\n", Path::new("/etc/map")).unwrap();
        assert_eq!(c.usage.claude_credentials, PathBuf::from(home).join(".claude/.credentials.json"));
        assert_eq!(c.usage.codex_sessions, PathBuf::from("/etc/map/logs/codex"));
        for bad in ["skip_pct = 0", "skip_pct = 120", "[usage.providers.nope]\nenabled = false", "[usage.providers.codex]\nreader = \"x\""] {
            let text = format!("[workspace]\ndir = \"/r\"\n[usage]\n{bad}\n");
            assert!(Config::parse(&text, Path::new("/")).is_err(), "{bad}");
        }
    }

    #[test]
    fn minimal_config_uses_builtins_and_resolves_paths() {
        let c = Config::parse("[workspace]\ndir = \"repo\"\n", Path::new("/etc/map")).unwrap();
        assert_eq!(c.workspace.dir, PathBuf::from("/etc/map/repo"));
        assert_eq!(c.server.bind, "127.0.0.1:4400");
        assert!(c.providers.contains_key("claude") && c.providers.contains_key("codex") && c.providers.contains_key("gemini"));
    }

    #[test]
    fn override_merges_onto_builtin() {
        let c = Config::parse(
            "[workspace]\ndir = \"/r\"\n[providers.claude]\nextra_args = [\"--dangerously-skip-permissions\"]\n",
            Path::new("/"),
        )
        .unwrap();
        let claude = &c.providers["claude"];
        assert_eq!(claude.extra_args, vec!["--dangerously-skip-permissions"]);
        assert_eq!(claude.binary, "claude");
    }

    #[test]
    fn login_settings_override_and_clear() {
        let c = Config::parse(
            "[workspace]\ndir = \"/r\"\n[providers.gemini]\nlogin_send = \"\"\ncredentials = [\"~/x.json\"]\n[providers.codex]\nlogin = [\"{binary}\", \"login\"]\n",
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(c.providers["gemini"].login_send, None);
        assert_eq!(c.providers["gemini"].credentials, vec!["~/x.json"]);
        assert_eq!(c.providers["codex"].login, vec!["{binary}", "login"]);
        assert!(c.providers["codex"].login_warning.is_some());
        assert_eq!(c.providers["claude"].api_key_env.as_deref(), Some("ANTHROPIC_API_KEY"));
    }

    #[test]
    fn update_settings_default_override_and_clear() {
        let c = Config::parse("[workspace]\ndir = \"/r\"\n", Path::new("/")).unwrap();
        assert_eq!(c.providers["codex"].version, vec!["{binary}", "--version"]);
        assert_eq!(c.providers["codex"].latest.as_ref().unwrap().json.as_deref(), Some("version"));
        assert!(c.providers["gemini"].latest.as_ref().unwrap().url.contains("{os}_{arch}"));
        assert_eq!(c.providers["claude"].update, vec!["{binary}", "update"]);
        assert_eq!((c.updates.enabled, c.updates.interval_secs), (true, 21600));
        let c = Config::parse(
            "[workspace]\ndir = \"/r\"\n[updates]\ninterval_secs = 5\n[providers.claude]\nlatest = { url = \"\" }\nupdate = []\n[providers.codex]\nlatest = { url = \"https://example.com/v.txt\" }\n[providers.aider]\nbinary = \"aider\"\ninteractive = [\"{binary}\"]\n",
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(c.updates.interval_secs, 300);
        assert_eq!(c.providers["claude"].latest, None);
        assert!(c.providers["claude"].update.is_empty());
        assert_eq!(c.providers["codex"].latest.as_ref().unwrap().json, None);
        assert!(c.providers["aider"].version.is_empty() && c.providers["aider"].latest.is_none());
        let bad = "[workspace]\ndir = \"/r\"\n[providers.codex]\nlatest = { url = \"file:///etc/passwd\" }\n";
        assert!(Config::parse(bad, Path::new("/")).is_err());
    }

    #[test]
    fn new_provider_needs_binary_and_command() {
        let err = Config::parse("[workspace]\ndir = \"/r\"\n[providers.aider]\nbinary = \"aider\"\n", Path::new("/")).unwrap_err();
        assert!(format!("{err:#}").contains("needs `binary` and `interactive`"));
    }

    #[test]
    fn tiers_default_for_claude_only_and_are_checked() {
        let c = Config::parse("[workspace]\ndir = \"/r\"\n", Path::new("/")).unwrap();
        let claude = &c.orchestrator.tiers["claude"];
        assert_eq!((claude.get("light"), claude.get("standard"), claude.get("heavy")), (Some("haiku"), Some("sonnet"), Some("opus")));
        assert!(!c.orchestrator.tiers.contains_key("codex") && !c.orchestrator.tiers.contains_key("gemini"));
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[orchestrator.tiers.codex]\nlight = \"gpt-6-luna\"\n", Path::new("/")).unwrap();
        assert_eq!(c.orchestrator.tiers["codex"].get("light"), Some("gpt-6-luna"));
        assert_eq!(c.orchestrator.tiers["codex"].get("heavy"), None);
        assert_eq!(c.orchestrator.tiers["claude"].get("light"), Some("haiku"), "claude keeps its built-in tiers");
        // A configured claude table replaces the built-in one.
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[orchestrator.tiers.claude]\nheavy = \"fable\"\n", Path::new("/")).unwrap();
        assert_eq!((c.orchestrator.tiers["claude"].get("light"), c.orchestrator.tiers["claude"].get("heavy")), (None, Some("fable")));
        let err = Config::parse("[workspace]\ndir = \"/r\"\n[orchestrator.tiers.codex]\nlight = \"haiku\"\n", Path::new("/")).unwrap_err();
        assert!(format!("{err:#}").contains("not in codex's models"), "{err:#}");
        assert!(Config::parse("[workspace]\ndir = \"/r\"\n[orchestrator.tiers.nope]\nlight = \"x\"\n", Path::new("/")).is_err());
        // Built-in tiers drop models a changed list no longer has.
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[providers.claude]\nmodels = [\"sonnet\"]\n", Path::new("/")).unwrap();
        assert_eq!((c.orchestrator.tiers["claude"].get("light"), c.orchestrator.tiers["claude"].get("standard")), (None, Some("sonnet")));
    }

    #[test]
    fn agent_homes_follow_the_mode_and_the_provider() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        let unset = |v: &str| std::env::var_os(v).is_none_or(|v| v.is_empty());
        // Shared (the default): the CLIs' usual folders, nothing exported, every path as before.
        let shared = Config::parse("[workspace]\ndir = \"/r\"\n[server]\ndata_dir = \"/var/map\"\n", Path::new("/etc/map")).unwrap();
        assert!(!shared.agents_home.isolated());
        assert_eq!(shared.agents_home.dir(), Path::new("/var/map/agent-home"), "no config file: under the data dir");
        if unset("CLAUDE_CONFIG_DIR") && unset("CODEX_HOME") {
            assert_eq!(shared.provider_home("claude"), Some(home.join(".claude")));
            assert_eq!(shared.provider_home("codex"), Some(home.join(".codex")));
            assert_eq!(shared.usage.codex_auth, home.join(".codex/auth.json"));
            assert_eq!(shared.providers["claude"].credentials, ["~/.claude/.credentials.json"]);
        }
        assert_eq!(shared.provider_home("gemini"), Some(home.join(".gemini")));
        for p in ["claude", "codex", "gemini"] {
            assert!(shared.provider_env(p).is_empty(), "{p}");
        }

        // Isolated: <dir>/<provider>, exported in each CLI's own variable; agy gets a HOME of its own.
        let text = "[workspace]\ndir = \"/r\"\n[agents_home]\nmode = \"isolated\"\ndir = \"/srv/ah\"\n[providers.aider]\nbinary = \"aider\"\ninteractive = [\"aider\"]\n";
        let c = Config::parse(text, Path::new("/etc/map")).unwrap();
        let ah = PathBuf::from("/srv/ah");
        assert_eq!(c.provider_home("claude"), Some(ah.join("claude")));
        assert_eq!(c.provider_home("codex"), Some(ah.join("codex")));
        assert_eq!(c.provider_home("gemini"), Some(ah.join("gemini-home/.gemini")));
        assert_eq!(c.provider_home("aider"), None);
        assert_eq!(c.provider_env("claude"), [("CLAUDE_CONFIG_DIR".to_string(), "/srv/ah/claude".to_string())]);
        assert_eq!(c.provider_env("codex"), [("CODEX_HOME".to_string(), "/srv/ah/codex".to_string())]);
        let agy = c.provider_env("gemini");
        assert_eq!(agy[0], ("HOME".to_string(), "/srv/ah/gemini-home".to_string()));
        let cargo = std::env::var("CARGO_HOME").unwrap_or_else(|_| home.join(".cargo").display().to_string());
        assert!(agy.contains(&("CARGO_HOME".to_string(), cargo)), "tools keep their real folders: {agy:?}");
        assert!(c.provider_env("aider").is_empty());
        assert!(c.warnings.iter().any(|w| w.contains("aider")), "{:?}", c.warnings);
        // Every path MAP reads under ~/.claude, ~/.codex or ~/.gemini moved with it; others did not.
        assert_eq!(c.providers["claude"].credentials, ["/srv/ah/claude/.credentials.json"]);
        assert_eq!(c.providers["codex"].credentials, ["/srv/ah/codex/auth.json"]);
        assert_eq!(c.providers["gemini"].credentials, ["/srv/ah/gemini-home/.gemini/jetski-standalone-oauth-token"]);
        assert_eq!(c.providers["claude"].latest.as_ref().unwrap().channel.as_ref().unwrap().file, "/srv/ah/claude/settings.json");
        assert_eq!(c.usage.claude_credentials, ah.join("claude/.credentials.json"));
        assert_eq!(c.usage.claude_projects, ah.join("claude/projects"));
        assert_eq!(c.usage.codex_sessions, ah.join("codex/sessions"));
        assert_eq!(c.usage.codex_auth, ah.join("codex/auth.json"));
        assert_eq!(c.skills.claude_user_dirs, [ah.join("claude/skills")]);
        assert_eq!(c.skills.claude_plugin_dirs, [ah.join("claude/plugins")]);
        assert_eq!(c.skills.codex_user_dirs, [ah.join("codex/skills")]);
        assert_eq!(c.skills.claude_settings, ah.join("claude/settings.json"));
        assert_eq!(c.skills.claude_transcripts, ah.join("claude/projects"));
        assert_eq!(c.agents.claude_user_dirs, [ah.join("claude/agents")]);
        assert_eq!(c.agents.codex_user_dirs, [ah.join("codex/agents")]);
        assert_eq!(c.agents.gemini_user_dirs, [ah.join("gemini-home/.gemini/config/agents")]);
        assert_eq!(c.templates.dirs, [home.join(".config/map/templates")]);

        // A variable set in [workspace.env] wins over the isolated folder (and is exported from there, not twice).
        let c = Config::parse(&format!("{text}[workspace.env]\nCODEX_HOME = \"/opt/codex\"\n"), Path::new("/")).unwrap();
        assert_eq!(c.provider_home("codex"), Some(PathBuf::from("/opt/codex")));
        assert!(c.provider_env("codex").is_empty());
        assert_eq!(c.usage.codex_auth, PathBuf::from("/opt/codex/auth.json"));

        for bad in [
            "[agents_home]\nmode = \"private\"",
            "[providers.codex]\nhome_env = \"1X\"",
            "[providers.codex]\nisolated_dir = \"../x\"",
            "[providers.gemini]\nhome_keep = [\"CARGO_HOME\"]",
            "[workspace]\ndir = \"/r\"\ntmux_socket = \"/tmp/x\"",
        ] {
            let text = if bad.starts_with("[workspace]") { bad.to_string() } else { format!("[workspace]\ndir = \"/r\"\n{bad}\n") };
            assert!(Config::parse(&text, Path::new("/")).is_err(), "{bad}");
        }
        let c = Config::parse("[workspace]\ndir = \"/r\"\ntmux_socket = \" map-dev \"\n", Path::new("/")).unwrap();
        assert_eq!(c.workspace.tmux_socket.as_deref(), Some("map-dev"));
        assert_eq!(Config::parse("[workspace]\ndir = \"/r\"\ntmux_socket = \"\"\n", Path::new("/")).unwrap().workspace.tmux_socket, None);
    }

    #[test]
    fn agent_home_defaults_next_to_map_toml_and_is_created_private() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("map.toml");
        std::fs::write(&file, format!("[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[agents_home]\nmode = \"isolated\"\n", dir.path().display())).unwrap();
        let c = Config::load(&file).unwrap();
        let ah = dir.path().join("agent-home");
        assert_eq!(c.agents_home.dir(), ah);
        let made = c.create_agent_homes().unwrap();
        for sub in ["claude", "codex", "gemini-home", "gemini-home/.gemini"] {
            assert!(made.contains(&ah.join(sub)), "{sub}: {made:?}");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for d in [ah.clone(), ah.join("claude"), ah.join("gemini-home/.gemini")] {
                assert_eq!(std::fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700, "{}", d.display());
            }
        }
        // Shared mode creates nothing.
        let shared = Config::parse(&format!("[workspace]\ndir = \"/r\"\n[server]\ndata_dir = \"{}/s\"\n", dir.path().display()), Path::new("/")).unwrap();
        assert!(shared.create_agent_homes().unwrap().is_empty());
        assert!(!dir.path().join("s").exists());
    }

    #[test]
    fn cursor_copilot_and_openrouter_are_built_in_but_not_in_the_order() {
        let c = Config::parse("[workspace]\ndir = \"/r\"\n", Path::new("/")).unwrap();
        assert_eq!(c.orchestrator.provider_order, ["claude", "codex", "gemini"], "new providers are opt-in");
        let cursor = &c.providers["cursor"];
        assert_eq!((cursor.binary.as_str(), cursor.default_model.as_str(), cursor.prompt_mode), ("cursor-agent", "auto", PromptMode::Arg));
        assert!(cursor.headless.windows(2).any(|w| w == ["-p", "--output-format"]) && cursor.headless.contains(&"--trust".to_string()));
        assert!(cursor.trust.is_none(), "where cursor keeps trusted folders is undocumented");
        assert_eq!(cursor.api_key_env.as_deref(), Some("CURSOR_API_KEY"));
        assert!(cursor.credentials.is_empty());

        let copilot = &c.providers["copilot"];
        assert_eq!((copilot.binary.as_str(), copilot.prompt_mode), ("copilot", PromptMode::Send));
        assert_eq!(copilot.headless[..3], ["{binary}", "-p", "{prompt}"]);
        assert_eq!(copilot.trust.as_ref().unwrap().format, TrustFormat::TrustedFoldersJson);
        assert_eq!(copilot.home_env.as_deref(), Some("COPILOT_HOME"));
        assert_eq!(copilot.latest.as_ref().unwrap().url, "https://registry.npmjs.org/@github/copilot/latest");
        assert!(copilot.models.contains(&copilot.default_model));

        let or = &c.providers["openrouter"];
        assert_eq!(or.binary, "claude");
        assert!(or.login.is_empty() && or.credentials.is_empty() && or.update.is_empty() && or.latest.is_none());
        assert_eq!(or.api_key_env.as_deref(), Some("OPENROUTER_API_KEY"));
        assert_eq!(or.env["ANTHROPIC_BASE_URL"], "https://openrouter.ai/api");
        assert_eq!(or.env["ANTHROPIC_AUTH_TOKEN"], "$OPENROUTER_API_KEY");
        assert_eq!(or.env["ANTHROPIC_API_KEY"], "");
        assert!(or.models.contains(&or.default_model));
        assert_eq!(or.trust.as_ref().unwrap().format, TrustFormat::ClaudeJsonOnboarded);
        // Opting in is one line.
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[orchestrator]\nprovider_order = [\"claude\", \"openrouter\", \"copilot\", \"cursor\"]\n", Path::new("/")).unwrap();
        assert_eq!(c.orchestrator.provider_order.len(), 4);
        // The trust formats by their map.toml names.
        let c = Config::parse(
            "[workspace]\ndir = \"/r\"\n[providers.codex.trust]\nformat = \"trusted_folders_json\"\n[providers.gemini.trust]\nformat = \"claude_json_onboarded\"\nfile = \"~/.x.json\"\n",
            Path::new("/"),
        )
        .unwrap();
        assert_eq!(c.providers["codex"].trust.as_ref().unwrap().format, TrustFormat::TrustedFoldersJson);
        assert_eq!(c.providers["gemini"].trust.as_ref().unwrap().format, TrustFormat::ClaudeJsonOnboarded);
    }

    #[test]
    fn openrouter_never_shares_the_claude_providers_folder() {
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        // Shared mode, even with CLAUDE_CONFIG_DIR in [workspace.env] for the claude provider.
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[workspace.env]\nCLAUDE_CONFIG_DIR = \"/opt/claude\"\n", Path::new("/")).unwrap();
        assert_eq!(c.provider_home("claude"), Some(PathBuf::from("/opt/claude")));
        assert_eq!(c.provider_home("openrouter"), Some(home.join(".claude-openrouter")));
        assert!(c.provider_env("openrouter").is_empty(), "exported after [workspace.env] instead");
        let fixed = c.provider_fixed_env("openrouter");
        assert_eq!(fixed[0], ("CLAUDE_CONFIG_DIR".to_string(), home.join(".claude-openrouter").display().to_string()));
        assert_eq!(c.provider_home_export("openrouter"), Some(("CLAUDE_CONFIG_DIR".to_string(), home.join(".claude-openrouter").display().to_string())));
        assert_eq!(c.provider_home_export("claude"), None, "shared mode exports nothing for claude");
        // Isolated: its own folder under agents_home, apart from claude's.
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[agents_home]\nmode = \"isolated\"\ndir = \"/srv/ah\"\n", Path::new("/")).unwrap();
        assert_eq!(c.provider_home("openrouter"), Some(PathBuf::from("/srv/ah/openrouter")));
        assert_eq!(c.provider_home("claude"), Some(PathBuf::from("/srv/ah/claude")));
        assert_eq!(c.provider_home("copilot"), Some(PathBuf::from("/srv/ah/copilot")));
        assert_eq!(c.provider_env("copilot"), [("COPILOT_HOME".to_string(), "/srv/ah/copilot".to_string())]);
        assert_eq!(c.provider_home("cursor"), Some(PathBuf::from("/srv/ah/cursor-home/.cursor")));
        assert_eq!(c.provider_env("cursor")[0], ("HOME".to_string(), "/srv/ah/cursor-home".to_string()));
        assert!(c.provider_env("cursor").iter().any(|(k, _)| k == "CARGO_HOME"));
        let dirs = c.provider_script_env("openrouter");
        assert_eq!(dirs[0], "export CLAUDE_CONFIG_DIR=/srv/ah/openrouter");
    }

    #[test]
    fn provider_env_references_secrets_and_resolves_them_only_in_memory() {
        let c = Config::parse(
            "[workspace]\ndir = \"/r\"\n[workspace.env]\nMY_KEY = \"sk-or-SECRET-123\"\n[providers.openrouter]\nenv = { ANTHROPIC_AUTH_TOKEN = \"$MY_KEY\", ANTHROPIC_API_KEY = \"\", X_LITERAL = \"it's $x\" }\n",
            Path::new("/"),
        )
        .unwrap();
        let script = c.provider_script_env("openrouter").join("\n");
        assert!(script.contains("export ANTHROPIC_AUTH_TOKEN=\"$MY_KEY\""), "{script}");
        assert!(script.contains("[ -n \"${MY_KEY:-}\" ] || echo"), "{script}");
        assert!(script.contains("export ANTHROPIC_API_KEY=''"), "{script}");
        assert!(script.contains("export X_LITERAL=\"it's \"'$x'"), "a literal `$` is quoted, not expanded: {script}");
        assert!(!script.contains("SECRET"), "the value never reaches the script: {script}");
        let fixed: BTreeMap<String, String> = c.provider_fixed_env("openrouter").into_iter().collect();
        assert_eq!(fixed["ANTHROPIC_AUTH_TOKEN"], "sk-or-SECRET-123");
        assert_eq!(fixed["ANTHROPIC_API_KEY"], "");
        assert_eq!(env_ref("$OPENROUTER_API_KEY"), Some("OPENROUTER_API_KEY"));
        assert_eq!(env_ref("${X}"), None);
        assert_eq!(env_ref("plain"), None);
        for bad in ["env = { \"1X\" = \"a\" }", "env = { A = \"$1X\" }", "env = { A = \"$X-Y\" }"] {
            assert!(Config::parse(&format!("[workspace]\ndir = \"/r\"\n[providers.openrouter]\n{bad}\n"), Path::new("/")).is_err(), "{bad}");
        }
    }

    #[test]
    fn api_fallback_format_url_and_name() {
        let c = Config::parse("[workspace]\ndir = \"/r\"\n", Path::new("/")).unwrap();
        assert_eq!((c.reflex.api_format, c.reflex.api_endpoint(), c.reflex.api_name().as_str()), (ApiFormat::Anthropic, "https://api.anthropic.com/v1/messages", "Anthropic API"));
        let c = Config::parse(
            "[workspace]\ndir = \"/r\"\n[reflex]\napi_format = \"openai\"\napi_url = \"https://openrouter.ai/api/v1/chat/completions\"\napi_key_env = \"OPENROUTER_API_KEY\"\napi_model = \"~anthropic/claude-haiku-latest\"\n",
            Path::new("/"),
        )
        .unwrap();
        assert_eq!((c.reflex.api_endpoint(), c.reflex.api_name().as_str()), ("https://openrouter.ai/api/v1/chat/completions", "OpenRouter API"));
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[reflex]\napi_format = \"openai\"\napi_url = \"http://127.0.0.1:8080/v1/chat/completions\"\n", Path::new("/")).unwrap();
        assert_eq!(c.reflex.api_name(), "OpenAI-compatible API (127.0.0.1)");
        assert_eq!(Config::parse("[workspace]\ndir = \"/r\"\n[reflex]\napi_format = \"openai\"\n", Path::new("/")).unwrap().reflex.api_endpoint(), "https://api.openai.com/v1/chat/completions");
        for bad in ["api_url = \"ftp://x\"", "api_format = \"grpc\"", "api_key_env = \"MY-KEY\""] {
            assert!(Config::parse(&format!("[workspace]\ndir = \"/r\"\n[reflex]\n{bad}\n"), Path::new("/")).is_err(), "{bad}");
        }
    }

    #[test]
    fn rejects_unknown_provider_in_order() {
        let err = Config::parse("[workspace]\ndir = \"/r\"\n[orchestrator]\nprovider_order = [\"nope\"]\n", Path::new("/")).unwrap_err();
        assert!(format!("{err:#}").contains("unknown provider `nope`"));
    }
}
