//! The Agents screen: custom sub-agents for the three CLIs MAP drives, in a workspace and in the home folder.
//!
//! - **Claude Code**: Markdown with a YAML header (`name`, `description`, `tools`, `model`, `effort`…), the body is
//!   the system prompt. `<workspace>/.claude/agents/**/*.md`, `~/.claude/agents/`.
//! - **Codex**: one TOML file per agent (`name`, `description`, `developer_instructions`, `model`,
//!   `model_reasoning_effort`, `[features] shell_tool`). `<workspace>/.codex/agents/*.toml`, `~/.codex/agents/`.
//! - **agy** (the `gemini` provider): Markdown like Claude's, with its own tool names and `model` of `inherit`,
//!   `flash` or `pro`. `<workspace>/.agents/agents/<name>.md` or `<name>/agent.md`, `~/.gemini/config/agents/`.
//!
//! Every CLI finds an agent by its `name`, not its file name, so the scanner groups files by name: one row per
//! agent and scope, with the file each CLI has. [`AgentDef`] is MAP's own definition; [`render`] writes it in each
//! format, keeping the keys of an existing file that MAP does not manage (`permissionMode`, `mcpServers`, …).
//! Folders come from `[agents]` in `map.toml`, and nothing is ever written outside them.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use regex::Regex;

use crate::config::AgentsConfig;
use crate::model::{
    AgentChange, AgentDef, AgentDetail, AgentDir, AgentFile, AgentFileError, AgentModels, AgentNote, AgentPlan, AgentProvider, AgentRow, AgentScope, AgentsOverview, Capability, Effort,
};
use crate::skills::{Raw, display_path, header_fields, split_tools};

use AgentProvider::{Claude, Codex, Gemini};

const PROVIDERS: [AgentProvider; 3] = [Claude, Codex, Gemini];
const CAPABILITIES: [Capability; 5] = [Capability::Read, Capability::Search, Capability::Edit, Capability::Shell, Capability::Web];
/// How deep the scanner looks below a Claude agents folder (Claude Code reads subfolders too).
const CLAUDE_DEPTH: usize = 4;
/// Larger files are reported, not read.
const MAX_AGENT_BYTES: u64 = 256 * 1024;
const MAX_DESCRIPTION: usize = 2000;
const MAX_INSTRUCTIONS: usize = 100_000;
/// Codex has no per-agent edit switch, so a read-only agent gets this line at the end of its instructions.
const READ_ONLY_LINE: &str = "Do not modify files.";
const CLAUDE_ALIASES: [&str; 5] = ["sonnet", "opus", "haiku", "fable", "inherit"];
const GEMINI_MODELS: [&str; 3] = ["inherit", "flash", "pro"];

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$").unwrap());
static MODEL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9._:/\[\]-]{0,99}$").unwrap());

pub fn valid_name(name: &str) -> bool {
    NAME.is_match(name)
}

fn label(p: AgentProvider) -> &'static str {
    match p {
        Claude => "Claude",
        Codex => "Codex",
        Gemini => "agy",
    }
}

fn effort_word(e: Effort) -> &'static str {
    match e {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    }
}

fn parse_effort(s: &str) -> Option<Effort> {
    Some(match s.trim().to_ascii_lowercase().as_str() {
        "low" => Effort::Low,
        "medium" => Effort::Medium,
        "high" => Effort::High,
        "xhigh" => Effort::Xhigh,
        "max" => Effort::Max,
        _ => return None,
    })
}

// ---- capabilities as each CLI's tools

/// The tools a capability stands for in Claude Code and agy (Codex has no tool list). agy's names come from this
/// fixed table only: a misspelled tool name can hang it.
fn tools_of(provider: AgentProvider, cap: Capability) -> &'static [&'static str] {
    match (provider, cap) {
        (Claude, Capability::Read) => &["Read"],
        (Claude, Capability::Search) => &["Grep", "Glob"],
        (Claude, Capability::Edit) => &["Edit", "Write"],
        (Claude, Capability::Shell) => &["Bash"],
        (Claude, Capability::Web) => &["WebFetch", "WebSearch"],
        (Gemini, Capability::Read) => &["view_file", "list_dir"],
        (Gemini, Capability::Search) => &["grep_search", "find_by_name"],
        (Gemini, Capability::Edit) => &["write_to_file", "replace_file_content"],
        (Gemini, Capability::Shell) => &["run_command"],
        (Gemini, Capability::Web) => &["read_url_content", "search_web"],
        (Codex, _) => &[],
    }
}

fn tool_names(provider: AgentProvider, caps: &[Capability]) -> Vec<&'static str> {
    CAPABILITIES.iter().filter(|c| caps.contains(c)).flat_map(|c| tools_of(provider, *c).iter().copied()).collect()
}

/// The capability a tool in a file belongs to: Claude's `Bash(git *)` is shell, `MultiEdit` is edit.
fn capability_of(provider: AgentProvider, tool: &str) -> Option<Capability> {
    match provider {
        Claude => Some(match tool.split('(').next().unwrap_or(tool).trim() {
            "Read" => Capability::Read,
            "Grep" | "Glob" | "LS" => Capability::Search,
            "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Capability::Edit,
            "Bash" => Capability::Shell,
            "WebFetch" | "WebSearch" => Capability::Web,
            _ => return None,
        }),
        Gemini => CAPABILITIES.into_iter().find(|c| tools_of(Gemini, *c).contains(&tool)),
        Codex => None,
    }
}

// ---- parsing

/// One agent file, read tolerantly: whatever could be read, and the first problem found.
#[derive(Debug, Clone)]
struct Parsed {
    provider: AgentProvider,
    scope: AgentScope,
    path: PathBuf,
    name: Option<String>,
    description: Option<String>,
    instructions: String,
    model: Option<String>,
    /// `None`: no `tools` key, so every tool.
    tools: Option<Vec<String>>,
    effort: Option<String>,
    /// Codex: `[features] shell_tool = false`.
    shell_off: bool,
    error: Option<String>,
}

impl Parsed {
    fn new(provider: AgentProvider, scope: AgentScope, path: PathBuf) -> Self {
        Self { provider, scope, path, name: None, description: None, instructions: String::new(), model: None, tools: None, effort: None, shell_off: false, error: None }
    }

    fn fail(&mut self, error: impl Into<String>) {
        self.error.get_or_insert(error.into());
    }
}

/// The body or instructions as MAP keeps them: Unix newlines, no blank lines around.
fn tidy_text(s: &str) -> String {
    s.replace("\r\n", "\n").trim_start_matches(['\n', '\r']).trim_end().to_string()
}

fn parse_markdown(p: &mut Parsed, text: &str) {
    let Some((fields, body)) = header_fields(text) else {
        p.fail("no `---` header");
        return;
    };
    let fields: HashMap<&str, &Raw> = fields.iter().map(|f| (f.key.as_str(), &f.value)).collect();
    let scalar = |k: &str| match fields.get(k) {
        Some(Raw::Scalar(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Raw::List(items)) if !items.is_empty() => Some(items.join(", ")),
        _ => None,
    };
    p.name = scalar("name");
    p.description = scalar("description");
    p.model = scalar("model");
    p.effort = scalar("effort");
    p.tools = match fields.get("tools") {
        Some(Raw::Scalar(s)) => Some(split_tools(s)),
        Some(Raw::List(items)) => Some(items.iter().flat_map(|i| split_tools(i)).collect()),
        None => None,
    };
    p.instructions = tidy_text(&body);
    match &p.name {
        None => p.fail("no `name`"),
        Some(n) if p.provider == Claude && n.contains(':') => p.fail("`name` contains `:`"),
        _ => {}
    }
    if p.description.is_none() {
        p.fail("no `description`");
    }
    if p.provider == Gemini {
        if let Some(m) = p.model.as_deref().filter(|m| !GEMINI_MODELS.contains(m)) {
            p.fail(format!("agy `model` must be inherit, flash or pro, not `{m}`"));
        }
        if let Some(t) = p.tools.iter().flatten().find(|t| capability_of(Gemini, t).is_none()) {
            p.fail(format!("`{t}` is not an agy tool MAP knows"));
        }
    }
}

fn parse_codex(p: &mut Parsed, text: &str) {
    let table: toml::Table = match toml::from_str(text) {
        Ok(t) => t,
        Err(e) => {
            p.fail(format!("invalid TOML: {}", e.message()));
            return;
        }
    };
    let string = |k: &str| table.get(k).and_then(toml::Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    p.name = string("name");
    p.description = string("description");
    p.model = string("model");
    p.effort = string("model_reasoning_effort");
    p.instructions = table.get("developer_instructions").and_then(toml::Value::as_str).map(tidy_text).unwrap_or_default();
    p.shell_off = table.get("features").and_then(|f| f.get("shell_tool")).and_then(toml::Value::as_bool) == Some(false);
    if p.name.is_none() {
        p.fail("no `name`");
    }
    if p.description.is_none() {
        p.fail("no `description`");
    }
    if p.instructions.is_empty() {
        p.fail("no `developer_instructions`");
    }
}

fn parse_file(provider: AgentProvider, scope: AgentScope, path: &Path) -> Parsed {
    let mut p = Parsed::new(provider, scope, path.to_path_buf());
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_AGENT_BYTES) {
        p.fail(format!("larger than {} KB", MAX_AGENT_BYTES / 1024));
        return p;
    }
    match std::fs::read_to_string(path) {
        Ok(text) if provider == Codex => parse_codex(&mut p, &text),
        Ok(text) => parse_markdown(&mut p, &text),
        Err(e) => p.fail(format!("unreadable: {e}")),
    }
    p
}

// ---- discovery

fn listing(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).collect();
    out.sort();
    out
}

/// Hidden names are skipped: MAP's own temporary files start with a dot.
fn hidden(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_none_or(|n| n.starts_with('.') || n == "node_modules")
}

fn has_ext(p: &Path, ext: &str) -> bool {
    p.extension().is_some_and(|e| e == ext) && p.is_file() && !hidden(p)
}

fn claude_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    for p in listing(dir) {
        if has_ext(&p, "md") {
            out.push(p);
        } else if depth > 0 && p.is_dir() && !hidden(&p) {
            claude_files(&p, depth - 1, out);
        }
    }
}

/// The agent files of one CLI in one folder.
fn files_in(provider: AgentProvider, dir: &Path) -> Vec<PathBuf> {
    let mut out = vec![];
    match provider {
        Claude => claude_files(dir, CLAUDE_DEPTH, &mut out),
        Codex => out.extend(listing(dir).into_iter().filter(|p| has_ext(p, "toml"))),
        Gemini => {
            for p in listing(dir) {
                if has_ext(&p, "md") {
                    out.push(p);
                } else if p.is_dir() && !hidden(&p) && p.join("agent.md").is_file() {
                    out.push(p.join("agent.md"));
                }
            }
        }
    }
    out
}

/// The configured folders of one CLI and scope, in order; saving writes to the first.
fn dirs_of(cfg: &AgentsConfig, workspace: &Path, provider: AgentProvider, scope: AgentScope) -> Vec<PathBuf> {
    let list = match (provider, scope) {
        (Claude, AgentScope::Project) => &cfg.claude_project_dirs,
        (Codex, AgentScope::Project) => &cfg.codex_project_dirs,
        (Gemini, AgentScope::Project) => &cfg.gemini_project_dirs,
        (Claude, AgentScope::User) => &cfg.claude_user_dirs,
        (Codex, AgentScope::User) => &cfg.codex_user_dirs,
        (Gemini, AgentScope::User) => &cfg.gemini_user_dirs,
    };
    match scope {
        AgentScope::Project => list.iter().map(|d| workspace.join(d)).collect(),
        AgentScope::User => list.clone(),
    }
}

fn scan(cfg: &AgentsConfig, workspace: &Path) -> Vec<Parsed> {
    let mut out = vec![];
    for scope in [AgentScope::Project, AgentScope::User] {
        for provider in PROVIDERS {
            for dir in dirs_of(cfg, workspace, provider, scope) {
                out.extend(files_in(provider, &dir).iter().map(|f| parse_file(provider, scope, f)));
            }
        }
    }
    out
}

// ---- MAP's definition from the files

/// The definition an agent's files describe: text and tools from its Claude file if it has one, else agy, else
/// Codex (files without errors first), each CLI's model from its own file. The notes say what a save would change.
fn reconstruct(files: &[&Parsed]) -> Option<(AgentDef, AgentProvider, Vec<AgentNote>)> {
    let order = |p: &&&Parsed| (p.error.is_some(), [Claude, Gemini, Codex].iter().position(|x| *x == p.provider));
    let src = *files.iter().min_by_key(order)?;
    let mut notes = vec![];
    let note = |notes: &mut Vec<AgentNote>, text: String| notes.push(AgentNote { provider: src.provider, text });

    let mut instructions = src.instructions.clone();
    let tools = match src.provider {
        Codex => {
            let read_only = instructions.ends_with(READ_ONLY_LINE);
            if read_only {
                instructions = tidy_text(&instructions[..instructions.len() - READ_ONLY_LINE.len()]);
            }
            (read_only || src.shell_off).then(|| CAPABILITIES.into_iter().filter(|c| !(read_only && *c == Capability::Edit) && !(src.shell_off && *c == Capability::Shell)).collect())
        }
        provider => src.tools.as_ref().map(|list| {
            let mut caps: Vec<Capability> = list.iter().filter_map(|t| capability_of(provider, t)).collect();
            caps.sort();
            caps.dedup();
            let written = tool_names(provider, &caps);
            for t in list.iter().filter(|t| !written.contains(&t.as_str())) {
                match capability_of(provider, t) {
                    Some(c) => note(&mut notes, format!("`{t}` is saved as {}", tools_of(provider, c).join(", "))),
                    None => note(&mut notes, format!("`{t}` is not one of MAP's capabilities; saving drops it")),
                }
            }
            caps
        }),
    };

    let model_of = |p: AgentProvider| files.iter().find(|f| f.provider == p).and_then(|f| f.model.clone());
    let effort_file = files.iter().filter(|f| f.provider != Gemini && f.effort.is_some()).min_by_key(|f| (f.provider != src.provider, f.provider));
    let effort = effort_file.and_then(|f| {
        let raw = f.effort.as_deref().unwrap_or_default();
        let e = parse_effort(raw);
        if e.is_none() {
            notes.push(AgentNote { provider: f.provider, text: format!("effort `{raw}` is not one of low, medium, high, xhigh, max; saving drops it") });
        }
        e
    });
    let mut targets: Vec<AgentProvider> = files.iter().map(|f| f.provider).collect();
    targets.sort();
    targets.dedup();
    let def = AgentDef {
        name: src.name.clone().unwrap_or_default(),
        description: src.description.clone().or_else(|| files.iter().find_map(|f| f.description.clone())).unwrap_or_default(),
        instructions,
        tools,
        models: AgentModels { claude: model_of(Claude), codex: model_of(Codex), gemini: model_of(Gemini) },
        effort,
        targets,
    };
    Some((def, src.provider, notes))
}

// ---- rendering

/// A YAML scalar: plain when that reads back as the same string, else double-quoted.
fn yaml_scalar(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let plain = !s.is_empty()
        && s == s.trim()
        && !s.starts_with(|c: char| "-?:,[]{}#&*!|>'\"%@`".contains(c))
        && !s.contains(": ")
        && !s.contains(" #")
        && !s.ends_with(':')
        && !s.chars().any(char::is_control)
        && !matches!(lower.as_str(), "true" | "false" | "yes" | "no" | "on" | "off" | "null" | "~")
        && s.parse::<f64>().is_err();
    if plain { s.to_string() } else { format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")) }
}

fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

/// The keys of a Markdown header MAP writes itself; any other key of an existing file is kept as written.
fn managed(provider: AgentProvider, key: &str) -> bool {
    matches!(key, "name" | "description" | "tools" | "model") || (provider == Claude && key == "effort")
}

/// What `def` loses in this CLI's format.
fn lossy(provider: AgentProvider, def: &AgentDef) -> Vec<String> {
    let mut out = vec![];
    match provider {
        Claude => {}
        Codex => {
            if let Some(caps) = &def.tools {
                if !caps.contains(&Capability::Edit) {
                    out.push(format!("Codex has no per-agent edit switch: “{READ_ONLY_LINE}” is added to its instructions"));
                }
                if !caps.contains(&Capability::Read) || !caps.contains(&Capability::Search) {
                    out.push("Codex agents can always read and search files".into());
                }
                if !caps.contains(&Capability::Web) {
                    out.push("Codex has no per-agent web switch: web search follows its own config".into());
                }
            }
            if def.effort == Some(Effort::Max) {
                out.push("Codex has no `max` effort: written as `xhigh`".into());
            }
        }
        Gemini => {
            if let Some(e) = def.effort {
                out.push(format!("agy has no effort setting: `{}` is dropped", effort_word(e)));
            }
        }
    }
    out
}

/// One file of `def` in this CLI's format, keeping the unmanaged keys of `existing` (the file it replaces).
pub fn render(provider: AgentProvider, def: &AgentDef, existing: Option<&str>) -> String {
    match provider {
        Codex => render_codex(def, existing),
        _ => render_markdown(provider, def, existing),
    }
}

fn render_markdown(provider: AgentProvider, def: &AgentDef, existing: Option<&str>) -> String {
    let mut head = vec![format!("name: {}", yaml_scalar(&def.name)), format!("description: {}", yaml_scalar(&def.description))];
    if let Some(caps) = &def.tools {
        let names = tool_names(provider, caps);
        // Claude's documented form is a comma list; agy's a YAML list.
        if provider == Claude {
            head.push(format!("tools: {}", names.join(", ")));
        } else {
            head.push("tools:".into());
            head.extend(names.iter().map(|n| format!("  - {n}")));
        }
    }
    let model = if provider == Claude { &def.models.claude } else { &def.models.gemini };
    if let Some(m) = model {
        head.push(format!("model: {}", yaml_scalar(m)));
    }
    if provider == Claude
        && let Some(e) = def.effort
    {
        head.push(format!("effort: {}", effort_word(e)));
    }
    if let Some((fields, _)) = existing.and_then(header_fields) {
        head.extend(fields.into_iter().filter(|f| !managed(provider, &f.key)).map(|f| f.raw));
    }
    format!("---\n{}\n---\n\n{}\n", head.join("\n"), def.instructions)
}

fn render_codex(def: &AgentDef, existing: Option<&str>) -> String {
    let mut rest: toml::Table = existing.and_then(|t| toml::from_str(t).ok()).unwrap_or_default();
    for key in ["name", "description", "model", "model_reasoning_effort", "developer_instructions"] {
        rest.remove(key);
    }
    let mut instructions = def.instructions.clone();
    let caps = def.tools.as_deref();
    if caps.is_some_and(|c| !c.contains(&Capability::Edit)) && !instructions.ends_with(READ_ONLY_LINE) {
        instructions = format!("{instructions}\n\n{READ_ONLY_LINE}");
    }
    // `shell_tool = false` when shell is not allowed; MAP's own `false` goes again when it is.
    let features = rest.entry("features").or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if let Some(f) = features.as_table_mut() {
        if caps.is_some_and(|c| !c.contains(&Capability::Shell)) {
            f.insert("shell_tool".into(), toml::Value::Boolean(false));
        } else if f.get("shell_tool").and_then(toml::Value::as_bool) == Some(false) {
            f.remove("shell_tool");
        }
    }
    if rest.get("features").and_then(toml::Value::as_table).is_some_and(toml::Table::is_empty) {
        rest.remove("features");
    }

    let mut out = vec![format!("name = {}", toml_string(&def.name)), format!("description = {}", toml_string(&def.description))];
    if let Some(m) = &def.models.codex {
        out.push(format!("model = {}", toml_string(m)));
    }
    if let Some(e) = def.effort {
        let word = if e == Effort::Max { "xhigh" } else { effort_word(e) };
        out.push(format!("model_reasoning_effort = {}", toml_string(word)));
    }
    out.push(format!("developer_instructions = {}", toml_string(&instructions)));
    let mut text = out.join("\n") + "\n";
    if !rest.is_empty() {
        // Plain keys come before tables in `to_string`, so the result is still one valid document.
        text.push('\n');
        text.push_str(&toml::to_string(&rest).unwrap_or_default());
    }
    text
}

// ---- checking a definition

/// Trims and orders a definition the way it is saved: one-line description, sorted unique tools and targets, empty
/// models as `None`.
fn normalize(mut def: AgentDef) -> AgentDef {
    def.name = def.name.trim().to_string();
    def.description = def.description.split_whitespace().collect::<Vec<_>>().join(" ");
    def.instructions = tidy_text(&def.instructions);
    if let Some(caps) = &mut def.tools {
        caps.sort();
        caps.dedup();
    }
    def.targets.sort();
    def.targets.dedup();
    for m in [&mut def.models.claude, &mut def.models.codex, &mut def.models.gemini] {
        *m = m.take().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
    def
}

fn check(def: &AgentDef) -> Result<()> {
    if !valid_name(&def.name) {
        bail!("name must be 1 to 64 of a-z, 0-9, - and _, starting with a letter or digit");
    }
    if def.description.is_empty() {
        bail!("a description is required: the parent agent reads it to decide when to delegate");
    }
    if def.description.chars().count() > MAX_DESCRIPTION {
        bail!("the description is longer than {MAX_DESCRIPTION} characters");
    }
    if def.instructions.is_empty() {
        bail!("instructions are required (Codex needs developer_instructions)");
    }
    if def.instructions.len() > MAX_INSTRUCTIONS {
        bail!("the instructions are longer than {} KB", MAX_INSTRUCTIONS / 1000);
    }
    if def.tools.as_ref().is_some_and(Vec::is_empty) {
        bail!("pick at least one capability, or all tools");
    }
    if def.targets.is_empty() {
        bail!("pick at least one CLI to write the agent for");
    }
    if let Some(m) = &def.models.claude
        && !(CLAUDE_ALIASES.contains(&m.as_str()) || (m.starts_with("claude-") && MODEL.is_match(m)))
    {
        bail!("Claude model must be one of {} or a model id like claude-opus-5-5", CLAUDE_ALIASES.join(", "));
    }
    if let Some(m) = &def.models.codex
        && !MODEL.is_match(m)
    {
        bail!("Codex model `{m}` has characters a model name does not use");
    }
    if let Some(m) = &def.models.gemini
        && !GEMINI_MODELS.contains(&m.as_str())
    {
        bail!("agy model must be inherit, flash or pro");
    }
    Ok(())
}

// ---- planning and writing

struct Step {
    provider: AgentProvider,
    path: PathBuf,
    content: Option<String>,
    exists: bool,
}

fn file_name(provider: AgentProvider, name: &str) -> String {
    if provider == Codex { format!("{name}.toml") } else { format!("{name}.md") }
}

/// The files saving `def` in `scope` writes or removes. Each target CLI's file is the one the scanner found with
/// this name (rewritten in place), else `<first folder>/<name>.<ext>`; a CLI dropped from the targets loses the
/// files found for it. A new file never replaces one that holds another agent.
fn plan(cfg: &AgentsConfig, workspace: &Path, scope: AgentScope, def: &AgentDef, scanned: &[Parsed]) -> (Vec<Step>, Vec<AgentNote>, Option<String>) {
    let (mut steps, mut notes, mut error) = (vec![], vec![], None);
    for provider in PROVIDERS {
        let found: Vec<&Parsed> = scanned.iter().filter(|p| p.scope == scope && p.provider == provider && p.name.as_deref() == Some(def.name.as_str())).collect();
        if !def.targets.contains(&provider) {
            steps.extend(found.iter().map(|f| Step { provider, path: f.path.clone(), content: None, exists: true }));
            continue;
        }
        let path = match found.first() {
            Some(f) => f.path.clone(),
            None => match dirs_of(cfg, workspace, provider, scope).into_iter().next() {
                Some(dir) => dir.join(file_name(provider, &def.name)),
                None => {
                    error.get_or_insert(format!("no {} folder is configured for {scope:?} agents in [agents]", label(provider)));
                    continue;
                }
            },
        };
        for extra in found.iter().skip(1) {
            notes.push(AgentNote { provider, text: format!("also defined in {}; {} loads only one of them", display_path(&extra.path, workspace), label(provider)) });
        }
        // An invalid name never reaches the disk: the path is only shown.
        let existing = if valid_name(&def.name) { std::fs::read_to_string(&path).ok() } else { None };
        if found.is_empty()
            && let Some(text) = &existing
        {
            let mut other = Parsed::new(provider, scope, path.clone());
            if provider == Codex { parse_codex(&mut other, text) } else { parse_markdown(&mut other, text) }
            let holder = other.name.map_or_else(|| "a file MAP cannot read as an agent".to_string(), |n| format!("agent `{n}`"));
            error.get_or_insert(format!("{} already holds {holder}; rename or remove it first", display_path(&path, workspace)));
        }
        notes.extend(lossy(provider, def).into_iter().map(|text| AgentNote { provider, text }));
        steps.push(Step { provider, content: Some(render(provider, def, existing.as_deref())), exists: existing.is_some() || path.exists(), path });
    }
    (steps, notes, error)
}

fn to_plan(steps: Vec<Step>, notes: Vec<AgentNote>, error: Option<String>, workspace: &Path) -> AgentPlan {
    let changes = steps.into_iter().map(|s| AgentChange { provider: s.provider, path: display_path(&s.path, workspace), content: s.content, exists: s.exists }).collect();
    AgentPlan { changes, notes, error }
}

/// What saving would write, with the reason it cannot be saved (if any) instead of an error.
pub fn preview(cfg: &AgentsConfig, workspace: &Path, scope: AgentScope, def: AgentDef) -> AgentPlan {
    let def = normalize(def);
    let invalid = check(&def).err().map(|e| e.to_string());
    let (steps, notes, error) = plan(cfg, workspace, scope, &def, &scan(cfg, workspace));
    to_plan(steps, notes, invalid.or(error), workspace)
}

/// The path must lie in one of the configured folders of its CLI and scope.
fn inside(cfg: &AgentsConfig, workspace: &Path, step: &Step, scope: AgentScope) -> bool {
    let lexical_ok = !step.path.components().any(|c| c == std::path::Component::ParentDir);
    lexical_ok && dirs_of(cfg, workspace, step.provider, scope).iter().any(|d| step.path.starts_with(d))
}

fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let dir = path.parent().context("agent path has no folder")?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let tmp = dir.join(format!(".{}.map-tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::write(&tmp, content).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))
}

fn remove(path: &Path) -> Result<()> {
    std::fs::remove_file(path).with_context(|| format!("removing {}", path.display()))?;
    // agy's folder form: `<name>/agent.md` leaves an empty folder behind.
    if path.file_name().is_some_and(|n| n == "agent.md")
        && let Some(dir) = path.parent()
    {
        let _ = std::fs::remove_dir(dir);
    }
    Ok(())
}

fn require_workspace(workspace: &Path, scope: AgentScope) -> Result<()> {
    if scope == AgentScope::Project && !workspace.is_dir() {
        bail!("workspace {} does not exist", workspace.display());
    }
    Ok(())
}

/// Writes `def` for each of its targets and removes the files of the CLIs it no longer targets.
pub fn save(cfg: &AgentsConfig, workspace: &Path, scope: AgentScope, def: AgentDef) -> Result<AgentPlan> {
    require_workspace(workspace, scope)?;
    let def = normalize(def);
    check(&def)?;
    let (steps, notes, error) = plan(cfg, workspace, scope, &def, &scan(cfg, workspace));
    if let Some(e) = error {
        bail!("{e}");
    }
    if let Some(s) = steps.iter().find(|s| !inside(cfg, workspace, s, scope)) {
        bail!("{} is outside the configured agent folders", s.path.display());
    }
    for s in &steps {
        match &s.content {
            Some(text) => write_atomic(&s.path, text)?,
            None => remove(&s.path)?,
        }
    }
    Ok(to_plan(steps, notes, None, workspace))
}

/// Removes every file of the agent in `scope`; `None` when it has none.
pub fn delete(cfg: &AgentsConfig, workspace: &Path, scope: AgentScope, name: &str) -> Result<Option<Vec<String>>> {
    let found: Vec<Parsed> = scan(cfg, workspace).into_iter().filter(|p| p.scope == scope && p.name.as_deref() == Some(name)).collect();
    if found.is_empty() {
        return Ok(None);
    }
    let mut removed = vec![];
    for f in &found {
        let step = Step { provider: f.provider, path: f.path.clone(), content: None, exists: true };
        if !inside(cfg, workspace, &step, scope) {
            bail!("{} is outside the configured agent folders", f.path.display());
        }
        remove(&f.path)?;
        removed.push(display_path(&f.path, workspace));
    }
    Ok(Some(removed))
}

// ---- views for the API

fn agent_file(p: &Parsed, workspace: &Path) -> AgentFile {
    let (tools, all_tools) = match p.provider {
        Codex => (if p.shell_off { vec!["shell_tool = false".to_string()] } else { vec![] }, !p.shell_off && !p.instructions.ends_with(READ_ONLY_LINE)),
        _ => (p.tools.clone().unwrap_or_default(), p.tools.is_none()),
    };
    AgentFile { provider: p.provider, path: display_path(&p.path, workspace), model: p.model.clone(), tools, all_tools, effort: p.effort.clone(), error: p.error.clone() }
}

type Groups<'a> = BTreeMap<(AgentScope, String), Vec<&'a Parsed>>;

/// Files grouped by scope and name, plus the ones that name no agent.
fn group(scanned: &[Parsed]) -> (Groups<'_>, Vec<&Parsed>) {
    let (mut rows, mut nameless): (Groups<'_>, Vec<&Parsed>) = (BTreeMap::new(), vec![]);
    for p in scanned {
        match &p.name {
            Some(n) => rows.entry((p.scope, n.clone())).or_default().push(p),
            None => nameless.push(p),
        }
    }
    for files in rows.values_mut() {
        files.sort_by_key(|f| f.provider);
    }
    (rows, nameless)
}

/// Reading notes, then what each of the agent's CLIs loses in its format.
fn row_notes(files: &[&Parsed]) -> Vec<AgentNote> {
    let Some((def, _, mut notes)) = reconstruct(files) else { return vec![] };
    for p in &def.targets {
        notes.extend(lossy(*p, &def).into_iter().map(|text| AgentNote { provider: *p, text }));
    }
    notes
}

pub fn overview(cfg: &AgentsConfig, workspace: &Path) -> AgentsOverview {
    let scanned = scan(cfg, workspace);
    let (grouped, nameless) = group(&scanned);
    let rows = grouped
        .into_iter()
        .map(|((scope, name), files)| AgentRow {
            description: files.iter().filter(|f| f.error.is_none()).chain(files.iter()).find_map(|f| f.description.clone()).unwrap_or_default(),
            notes: row_notes(&files),
            files: files.iter().map(|f| agent_file(f, workspace)).collect(),
            name,
            scope,
        })
        .collect();
    let errors = nameless
        .into_iter()
        .map(|p| AgentFileError { provider: p.provider, scope: p.scope, path: display_path(&p.path, workspace), error: p.error.clone().unwrap_or_else(|| "no `name`".into()) })
        .collect();
    let mut dirs = vec![];
    for scope in [AgentScope::Project, AgentScope::User] {
        for provider in PROVIDERS {
            dirs.extend(dirs_of(cfg, workspace, provider, scope).into_iter().map(|d| AgentDir { provider, scope, exists: d.is_dir(), path: display_path(&d, workspace) }));
        }
    }
    AgentsOverview { project: None, workspace: workspace.display().to_string(), rows, errors, dirs, scanned_at: Utc::now() }
}

pub fn detail(cfg: &AgentsConfig, workspace: &Path, scope: AgentScope, name: &str) -> Option<AgentDetail> {
    let scanned = scan(cfg, workspace);
    let (grouped, _) = group(&scanned);
    let files = grouped.get(&(scope, name.to_string()))?;
    let (def, source, mut notes) = reconstruct(files)?;
    for p in &def.targets {
        notes.extend(lossy(*p, &def).into_iter().map(|text| AgentNote { provider: *p, text }));
    }
    Some(AgentDetail { scope, def, source, files: files.iter().map(|f| agent_file(f, workspace)).collect(), notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// Every folder inside temporary directories: the real home folder is never read or written.
    fn test_config(home: &Path) -> AgentsConfig {
        AgentsConfig {
            claude_user_dirs: vec![home.join(".claude/agents")],
            codex_user_dirs: vec![home.join(".codex/agents")],
            gemini_user_dirs: vec![home.join(".gemini/config/agents")],
            ..AgentsConfig::default()
        }
    }

    fn def(name: &str) -> AgentDef {
        AgentDef {
            name: name.into(),
            description: "Reviews diffs: finds bugs, \"risky\" code and # comments.".into(),
            instructions: "You review code.\n\n---\n\nReport findings as a list.".into(),
            tools: Some(vec![Capability::Read, Capability::Search, Capability::Shell]),
            models: AgentModels { claude: Some("opus".into()), codex: Some("gpt-5.5-codex".into()), gemini: Some("pro".into()) },
            effort: Some(Effort::High),
            targets: PROVIDERS.to_vec(),
        }
    }

    fn parsed(provider: AgentProvider, text: &str) -> Parsed {
        let mut p = Parsed::new(provider, AgentScope::Project, PathBuf::from("x"));
        if provider == Codex { parse_codex(&mut p, text) } else { parse_markdown(&mut p, text) }
        p
    }

    #[test]
    fn hand_written_claude_agent_parses() {
        let p = parsed(Claude, "---\nname: code-reviewer\ndescription: Expert code reviewer. Use after code changes.\ntools: Read, Grep, Glob, Bash\nmodel: claude-opus-5-5\n---\nYou are a senior reviewer.\n");
        assert_eq!(p.error, None);
        assert_eq!(p.name.as_deref(), Some("code-reviewer"));
        assert_eq!(p.tools.as_deref().unwrap(), ["Read", "Grep", "Glob", "Bash"]);
        assert_eq!(p.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(p.instructions, "You are a senior reviewer.");
        let (def, source, notes) = reconstruct(&[&p]).unwrap();
        assert_eq!(source, Claude);
        assert_eq!(def.tools, Some(vec![Capability::Read, Capability::Search, Capability::Shell]));
        assert_eq!(def.models.claude.as_deref(), Some("claude-opus-5-5"));
        assert!(notes.is_empty(), "{notes:?}");
        assert!(check(&normalize(def)).is_ok());
    }

    #[test]
    fn round_trip_each_format() {
        let d = normalize(def("reviewer"));
        for provider in PROVIDERS {
            let text = render(provider, &d, None);
            let p = parsed(provider, &text);
            assert_eq!(p.error, None, "{provider:?}:\n{text}");
            assert_eq!(p.name.as_deref(), Some("reviewer"));
            assert_eq!(p.description.as_deref(), Some(d.description.as_str()), "{text}");
            let (back, _, _) = reconstruct(&[&p]).unwrap();
            assert_eq!(back.instructions, d.instructions, "{provider:?}");
            match provider {
                Claude => {
                    assert!(text.contains("tools: Read, Grep, Glob, Bash\nmodel: opus\neffort: high\n"), "{text}");
                    assert_eq!((back.tools.clone(), back.effort), (d.tools.clone(), Some(Effort::High)));
                }
                Gemini => {
                    assert!(text.contains("tools:\n  - view_file\n  - list_dir\n  - grep_search\n  - find_by_name\n  - run_command\nmodel: pro\n"), "{text}");
                    assert!(!text.contains("effort"));
                    assert_eq!(back.tools, d.tools);
                }
                Codex => {
                    assert!(text.contains("model_reasoning_effort = \"high\""), "{text}");
                    assert!(text.contains(READ_ONLY_LINE), "{text}");
                    // Read-only and no web: the shell stays on, edits are refused in the instructions.
                    assert!(!text.contains("shell_tool"));
                    assert_eq!(back.tools, Some(vec![Capability::Read, Capability::Search, Capability::Shell, Capability::Web]));
                    assert_eq!(back.models.codex.as_deref(), Some("gpt-5.5-codex"));
                }
            }
        }
        // No shell: Codex switches its shell tool off; every tool: no `tools` key at all.
        let mut d2 = d.clone();
        d2.tools = Some(vec![Capability::Read, Capability::Edit]);
        let codex = render(Codex, &d2, None);
        assert!(codex.contains("[features]\nshell_tool = false"), "{codex}");
        assert_eq!(reconstruct(&[&parsed(Codex, &codex)]).unwrap().0.tools, Some(vec![Capability::Read, Capability::Search, Capability::Edit, Capability::Web]));
        d2.tools = None;
        d2.effort = Some(Effort::Max);
        assert!(!render(Claude, &d2, None).contains("tools"));
        let codex = render(Codex, &d2, None);
        assert!(codex.contains("model_reasoning_effort = \"xhigh\"") && !codex.contains(READ_ONLY_LINE) && !codex.contains("features"), "{codex}");
        let notes = lossy(Codex, &d2);
        assert!(notes.iter().any(|n| n.contains("xhigh")));
        assert!(lossy(Gemini, &d2).iter().any(|n| n.contains("max")));
    }

    #[test]
    fn rewrite_keeps_unmanaged_keys() {
        let d = normalize(def("reviewer"));
        let old = "---\nname: reviewer\ndescription: old\npermissionMode: plan\nmcpServers:\n  - github\ntools: Read\n---\nold body\n";
        let text = render(Claude, &d, Some(old));
        assert!(text.contains("permissionMode: plan\nmcpServers:\n  - github\n---"), "{text}");
        assert!(!text.contains("old"));
        let old = "name = \"reviewer\"\ndescription = \"old\"\ndeveloper_instructions = \"x\"\nsandbox_mode = \"read-only\"\n\n[features]\nshell_tool = false\nother = true\n";
        let mut d2 = d.clone();
        d2.tools = None;
        let text = render(Codex, &d2, Some(old));
        let table: toml::Table = toml::from_str(&text).unwrap();
        assert_eq!(table["sandbox_mode"].as_str(), Some("read-only"));
        assert_eq!(table["features"].as_table().unwrap().len(), 1, "{text}");
        assert_eq!(table["features"]["other"].as_bool(), Some(true));
    }

    #[test]
    fn names_and_fields_are_checked() {
        for good in ["a", "code-reviewer", "x_1", &"a".repeat(64)] {
            assert!(valid_name(good), "{good}");
        }
        for bad in ["", "-x", "_x", "A", "a b", "a/b", "../x", "a.md", "a:b", &"a".repeat(65)] {
            assert!(!valid_name(bad), "{bad}");
        }
        let ok = normalize(def("ok"));
        let with = |f: &dyn Fn(&mut AgentDef)| {
            let mut d = ok.clone();
            f(&mut d);
            check(&normalize(d))
        };
        assert!(with(&|_| {}).is_ok());
        assert!(with(&|d| d.description = "  \n ".into()).is_err());
        assert!(with(&|d| d.instructions = "\n".into()).is_err());
        assert!(with(&|d| d.tools = Some(vec![])).is_err());
        assert!(with(&|d| d.targets.clear()).is_err());
        assert!(with(&|d| d.models.gemini = Some("ultra".into())).is_err());
        assert!(with(&|d| d.models.claude = Some("gpt-5".into())).is_err());
        assert!(with(&|d| d.models.claude = Some("claude-sonnet-5-5".into())).is_ok());
        assert!(with(&|d| d.models.codex = Some("a\"b".into())).is_err());
        assert!(with(&|d| d.models.codex = Some(" ".into())).is_ok());
        // agy files with a model or tool it does not know are flagged, since a bad tool name can hang it.
        assert!(parsed(Gemini, "---\nname: a\ndescription: b\ntools: [view_file, read_file]\n---\nx").error.unwrap().contains("read_file"));
        assert!(parsed(Gemini, "---\nname: a\ndescription: b\nmodel: gemini-3\n---\nx").error.is_some());
        assert!(parsed(Claude, "---\nname: a:b\ndescription: b\n---\nx").error.is_some());
        assert!(parsed(Codex, "name = \"a\"\ndescription = \"b\"\n").error.unwrap().contains("developer_instructions"));
    }

    #[test]
    fn isolated_mode_keeps_user_agents_in_maps_own_cli_folders() {
        let dir = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let text = format!("[workspace]\ndir = \"/r\"\n[agents_home]\nmode = \"isolated\"\ndir = \"{}\"\n", dir.path().display());
        let cfg = crate::config::Config::parse(&text, Path::new("/")).unwrap().agents;
        write(&dir.path().join("claude/agents/mine.md"), "---\nname: mine\ndescription: Mine.\n---\nMine.\n");
        let o = overview(&cfg, ws.path());
        assert_eq!(o.rows.iter().map(|r| (r.scope, r.name.as_str())).collect::<Vec<_>>(), [(AgentScope::User, "mine")]);
        save(&cfg, ws.path(), AgentScope::User, def("reviewer")).unwrap();
        assert!(dir.path().join("codex/agents/reviewer.toml").is_file());
        assert!(dir.path().join("gemini-home/.gemini/config/agents/reviewer.md").is_file());
    }

    #[test]
    fn scan_groups_by_name_and_shows_compatibility() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let (cfg, h, w) = (test_config(home.path()), home.path(), ws.path());
        // One agent in all three CLIs, under three different file names.
        write(&w.join(".claude/agents/review/main.md"), "---\nname: reviewer\ndescription: Reviews.\ntools: Read, Bash(git diff *), mcp__github__get_pr\nmodel: sonnet\n---\nReview.\n");
        write(&w.join(".codex/agents/rev.toml"), "name = \"reviewer\"\ndescription = \"Reviews.\"\ndeveloper_instructions = \"Review.\"\nmodel = \"gpt-5.5\"\n");
        write(&w.join(".agents/agents/reviewer/agent.md"), "---\nname: reviewer\ndescription: Reviews.\nmodel: flash\n---\nReview.\n");
        // Claude only, and a broken Codex file of its own.
        write(&w.join(".claude/agents/planner.md"), "---\nname: planner\ndescription: Plans.\n---\nPlan.\n");
        write(&w.join(".codex/agents/planner.toml"), "name = \"planner\"\n");
        write(&w.join(".codex/agents/broken.toml"), "name = [unclosed\n");
        write(&w.join(".claude/agents/notes.txt"), "not an agent");
        write(&w.join(".claude/agents/.reviewer.md.map-tmp"), "---\nname: tmp\ndescription: x\n---\n");
        // The same name user-wide is its own row.
        write(&h.join(".claude/agents/reviewer.md"), "---\nname: reviewer\ndescription: Mine.\n---\nMine.\n");

        let o = overview(&cfg, w);
        let names: Vec<(AgentScope, &str)> = o.rows.iter().map(|r| (r.scope, r.name.as_str())).collect();
        assert_eq!(names, vec![(AgentScope::Project, "planner"), (AgentScope::Project, "reviewer"), (AgentScope::User, "reviewer")]);
        let rev = &o.rows[1];
        assert_eq!(rev.files.iter().map(|f| f.provider).collect::<Vec<_>>(), PROVIDERS.to_vec());
        assert_eq!(rev.files[0].path, ".claude/agents/review/main.md");
        assert_eq!(rev.files[1].model.as_deref(), Some("gpt-5.5"));
        assert_eq!(rev.files[2].path, ".agents/agents/reviewer/agent.md");
        assert!(rev.files.iter().all(|f| f.error.is_none()));
        assert!(rev.notes.iter().any(|n| n.text.contains("mcp__github__get_pr") && n.text.contains("drops")));
        assert!(rev.notes.iter().any(|n| n.text.contains("`Bash(git diff *)` is saved as Bash")));
        let planner = &o.rows[0];
        assert_eq!(planner.files.len(), 2);
        assert!(planner.files[1].error.as_deref().unwrap().contains("description"));
        assert_eq!(o.errors.len(), 1);
        assert!(o.errors[0].path.ends_with("broken.toml") && o.errors[0].error.contains("TOML"));
        assert!(o.rows[2].files[0].path.ends_with(".claude/agents/reviewer.md"));
        assert_eq!(o.rows[2].description, "Mine.");
        assert_eq!(o.dirs.len(), 6);

        let d = detail(&cfg, w, AgentScope::Project, "reviewer").unwrap();
        assert_eq!(d.source, Claude);
        assert_eq!(d.def.targets, PROVIDERS.to_vec());
        assert_eq!(d.def.models, AgentModels { claude: Some("sonnet".into()), codex: Some("gpt-5.5".into()), gemini: Some("flash".into()) });
        assert_eq!(d.def.tools, Some(vec![Capability::Read, Capability::Shell]));
        assert!(detail(&cfg, w, AgentScope::User, "planner").is_none());
    }

    #[test]
    fn save_writes_targets_in_place_and_removes_dropped_ones() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let (cfg, w) = (test_config(home.path()), ws.path());
        write(&w.join(".agents/agents/reviewer/agent.md"), "---\nname: reviewer\ndescription: Old.\nsubagent: true\n---\nOld.\n");

        let plan = save(&cfg, w, AgentScope::Project, def("reviewer")).unwrap();
        let paths: Vec<&str> = plan.changes.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(paths, vec![".claude/agents/reviewer.md", ".codex/agents/reviewer.toml", ".agents/agents/reviewer/agent.md"]);
        assert!(plan.changes.iter().all(|c| c.content.is_some()));
        let agy = std::fs::read_to_string(w.join(".agents/agents/reviewer/agent.md")).unwrap();
        assert!(agy.contains("subagent: true") && agy.contains("You review code."), "{agy}");
        assert!(!w.join(".agents/agents/reviewer.md").exists());
        let row = &overview(&cfg, w).rows[0];
        assert_eq!((row.files.len(), row.description.as_str()), (3, "Reviews diffs: finds bugs, \"risky\" code and # comments."));
        assert!(row.files.iter().all(|f| f.error.is_none()), "{:?}", row.files);
        // Nothing left behind but the agent files.
        assert_eq!(std::fs::read_dir(w.join(".claude/agents")).unwrap().count(), 1);

        // Dropping two targets removes their files (and agy's empty folder); the user scope is untouched.
        write(&home.path().join(".codex/agents/reviewer.toml"), "name = \"reviewer\"\ndescription = \"x\"\ndeveloper_instructions = \"x\"\n");
        let mut d = def("reviewer");
        d.targets = vec![Claude];
        let plan = save(&cfg, w, AgentScope::Project, d.clone()).unwrap();
        assert_eq!(plan.changes.iter().filter(|c| c.content.is_none()).count(), 2);
        assert!(!w.join(".codex/agents/reviewer.toml").exists());
        assert!(!w.join(".agents/agents/reviewer").exists());
        assert!(home.path().join(".codex/agents/reviewer.toml").exists());
        let p = preview(&cfg, w, AgentScope::Project, d.clone());
        assert_eq!((p.changes.len(), p.error), (1, None));
        assert!(p.changes[0].exists);

        // Delete removes every file of that scope only.
        assert_eq!(delete(&cfg, w, AgentScope::Project, "reviewer").unwrap().unwrap(), vec![".claude/agents/reviewer.md"]);
        assert!(delete(&cfg, w, AgentScope::Project, "reviewer").unwrap().is_none());
        assert!(home.path().join(".codex/agents/reviewer.toml").exists());

        // User scope writes into the (temporary) home folders.
        d.targets = vec![Gemini];
        save(&cfg, w, AgentScope::User, d).unwrap();
        assert!(home.path().join(".gemini/config/agents/reviewer.md").exists());
    }

    #[test]
    fn save_refuses_bad_names_and_foreign_files() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let (cfg, w) = (test_config(home.path()), ws.path());
        write(&w.join(".claude/agents/reviewer.md"), "---\nname: code-reviewer\ndescription: Someone else.\n---\nx\n");
        let before = std::fs::read_to_string(w.join(".claude/agents/reviewer.md")).unwrap();
        let err = save(&cfg, w, AgentScope::Project, def("reviewer")).unwrap_err().to_string();
        assert!(err.contains("code-reviewer"), "{err}");
        assert_eq!(std::fs::read_to_string(w.join(".claude/agents/reviewer.md")).unwrap(), before);
        // Nothing else was written either.
        assert!(!w.join(".codex").exists() && !w.join(".agents").exists());
        assert!(preview(&cfg, w, AgentScope::Project, def("reviewer")).error.unwrap().contains("code-reviewer"));

        for bad in ["../evil", "a/b", "Evil", ""] {
            assert!(save(&cfg, w, AgentScope::Project, def(bad)).is_err(), "{bad}");
            assert!(preview(&cfg, w, AgentScope::Project, def(bad)).error.is_some());
        }
        assert!(!ws.path().parent().unwrap().join("evil.md").exists());
        assert!(save(&cfg, &w.join("missing"), AgentScope::Project, def("x")).is_err());
    }
}
