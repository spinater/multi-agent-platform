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
    /// Overrides merged onto the built-in `claude`, `codex` and `gemini` specs; new names add providers.
    #[serde(default, rename = "providers")]
    provider_overrides: BTreeMap<String, ProviderOverride>,
    #[serde(skip)]
    pub providers: BTreeMap<String, ProviderSpec>,
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
}

fn default_prefix() -> String {
    "map-".into()
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
    /// Environment variable holding the Anthropic API key for projects whose fallback is `api`.
    pub api_key_env: String,
    /// Model for the `api` fallback when a project does not set one.
    pub api_model: String,
}

impl Default for ReflexConfig {
    fn default() -> Self {
        Self { binary: "reflex".into(), port_base: 7400, api_key_env: "ANTHROPIC_API_KEY".into(), api_model: "claude-haiku-5-5".into() }
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
        let context_files = [("claude", "CLAUDE.md"), ("codex", "AGENTS.md"), ("gemini", "GEMINI.md")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        Self {
            context_files,
            claude_project_dirs: vec![".claude/skills".into()],
            codex_project_dirs: vec![".agents/skills".into()],
            claude_user_dirs: vec!["~/.claude/skills".into()],
            claude_plugin_dirs: vec!["~/.claude/plugins".into()],
            codex_user_dirs: vec!["~/.codex/skills".into()],
            claude_settings: "~/.claude/settings.json".into(),
            claude_transcripts: "~/.claude/projects".into(),
        }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptMode {
    /// Appended as the last command-line argument.
    Arg,
    /// Typed into the pane after `startup_secs`.
    Send,
}

/// A coding-agent CLI. Command templates substitute `{binary}`, `{model}`, `{session}` and (headless only) `{prompt}`.
#[derive(Debug, Clone)]
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
    login: Option<Vec<String>>,
    login_send: Option<String>,
    login_warning: Option<String>,
    credentials: Option<Vec<String>>,
    api_key_env: Option<String>,
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
            login: strings(&["{binary}", "auth", "login"]),
            login_send: None,
            login_warning: None,
            credentials: strings(&["~/.claude/.credentials.json"]),
            api_key_env: Some("ANTHROPIC_API_KEY".into()),
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
            login: strings(&["{binary}", "login", "--device-auth"]),
            login_send: None,
            // Seen with codex-cli 0.159: the device flow deletes ~/.codex/auth.json as soon as it starts.
            login_warning: Some("codex login signs out the current account as soon as it starts, before you approve the new one.".into()),
            credentials: strings(&["~/.codex/auth.json"]),
            api_key_env: Some("OPENAI_API_KEY".into()),
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
            // agy signs in from its TUI (`/login`); it may keep the token in the system keyring instead of this file.
            login: strings(&["{binary}"]),
            login_send: Some("/login".into()),
            login_warning: None,
            credentials: strings(&["~/.gemini/jetski-standalone-oauth-token"]),
            api_key_env: Some("GEMINI_API_KEY".into()),
        },
    );
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
                    headless: vec![],
                    extra_args: vec![],
                    prompt_mode: PromptMode::Send,
                    startup_secs: 4,
                    idle_markers: vec![],
                    working_markers: vec![],
                    auth_markers: vec![],
                    login: vec![],
                    login_send: None,
                    login_warning: None,
                    credentials: vec![],
                    api_key_env: None,
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
            login: self.login.unwrap_or(base.login),
            login_send: optional(self.login_send, base.login_send),
            login_warning: optional(self.login_warning, base.login_warning),
            credentials: self.credentials.unwrap_or(base.credentials),
            api_key_env: optional(self.api_key_env, base.api_key_env),
        })
    }
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new("."));
        Self::parse(&text, base)
    }

    /// Relative paths in the file resolve against `base` (the config file's directory).
    pub fn parse(text: &str, base: &Path) -> Result<Self> {
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
        if let Some(dir) = skills.claude_project_dirs.iter().chain(&skills.codex_project_dirs).find(|d| d.is_absolute() || d.components().any(|c| c == std::path::Component::ParentDir)) {
            bail!("skills project dir `{}` must be relative to the workspace and stay inside it", dir.display());
        }
        if config.reflex.binary.contains('/') {
            config.reflex.binary = resolve(Path::new(&config.reflex.binary)).to_string_lossy().into_owned();
        }

        if config.server.api_token.as_deref() == Some("") {
            config.server.api_token = None;
        }
        if !config.workspace.session_prefix.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || config.workspace.session_prefix.is_empty() {
            bail!("workspace.session_prefix must be non-empty and use only letters, digits, - or _");
        }
        if let Some(key) = config.workspace.env.keys().find(|k| !k.chars().enumerate().all(|(i, c)| c == '_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit()))) {
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
        Ok(config)
    }
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
        assert_eq!(c.skills.context_files["codex"], "AGENTS.md");
    }

    #[test]
    fn skills_paths_expand_home() {
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[skills]\nclaude_user_dirs = [\"~/sk\", \"rel\"]\n", Path::new("/etc/map")).unwrap();
        let home = PathBuf::from(std::env::var_os("HOME").unwrap());
        assert_eq!(c.skills.claude_user_dirs, vec![home.join("sk"), PathBuf::from("/etc/map/rel")]);
        assert_eq!(c.skills.claude_transcripts, home.join(".claude/projects"));
        assert!(Config::parse("[workspace]\ndir = \"/r\"\n[skills]\nclaude_project_dirs = [\"../x\"]\n", Path::new("/")).is_err());
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
    fn new_provider_needs_binary_and_command() {
        let err = Config::parse("[workspace]\ndir = \"/r\"\n[providers.aider]\nbinary = \"aider\"\n", Path::new("/")).unwrap_err();
        assert!(format!("{err:#}").contains("needs `binary` and `interactive`"));
    }

    #[test]
    fn rejects_unknown_provider_in_order() {
        let err = Config::parse("[workspace]\ndir = \"/r\"\n[orchestrator]\nprovider_order = [\"nope\"]\n", Path::new("/")).unwrap_err();
        assert!(format!("{err:#}").contains("unknown provider `nope`"));
    }
}
