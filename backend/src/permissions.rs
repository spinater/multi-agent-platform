//! Configure → Permissions: tool rules × the CLIs MAP runs, each cell allow / ask / deny, written into the CLI's
//! own settings file. MAP's workers run one CLI per provider and each CLI enforces its own file, so the columns are
//! providers, not agents.
//!
//! - **Claude Code** (`claude`, `openrouter`): `permissions.allow / ask / deny` rule lists in `settings.json` of its
//!   config folder (machine) and `<project>/.claude/settings.local.json` (project; the shared
//!   `.claude/settings.json` is read, never written). Rules: `Read`, `Grep`, `Bash(git *)` (the legacy `Bash(git:*)`
//!   reads the same), `Bash` for every command, `mcp__<server>` for a server's tools.
//! - **Gemini / agy**: `tools.allowed` (runs without asking) and `tools.exclude` (never available) in `settings.json`
//!   of its `.gemini` folder and `<project>/.gemini/settings.json`; a file that already uses the older top-level
//!   `allowedTools` / `excludeTools` keeps them. There is no ask list: "ask" is a rule in neither list.
//! - **Codex**: no per-tool rules (only `approval_policy` and `sandbox_mode` for everything), so its cells are
//!   `unsupported`.
//!
//! Both Claude Code and Gemini merge the lists of every settings file and a deny anywhere wins, so in project scope
//! a cell shows the stricter of the project's and the machine's value, with a note when the project's one loses.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::cli_files::{self, array_at, edit_json, file_entry, object_at, read_json, read_toml};
use crate::config::Config;
use crate::model::{CliKind, ConfigCli, ConfigFile, ConfigScope, ConfigSource, PermCell, PermChange, PermRow, PermValue, PermissionsView};

/// The default rows: the rule as shown and what it covers. One `mcp__<name>__*` row per MCP server follows.
pub const ROWS: [(&str, &str); 6] = [
    ("Read / Grep", "read and search files in the workspace"),
    ("Edit / Write", "change files"),
    ("Bash(git *)", "git commands"),
    ("Bash(npm *, bun *)", "package scripts"),
    ("Bash(*)", "any shell command"),
    ("WebFetch / WebSearch", "read the web"),
];

/// One CLI rule a row stands for: the spelling MAP writes, and the other spellings that mean the same.
struct Rule {
    write: String,
    aliases: Vec<String>,
}

impl Rule {
    fn new(write: &str, aliases: &[&str]) -> Self {
        Rule { write: write.into(), aliases: aliases.iter().map(|a| a.to_string()).collect() }
    }

    fn matches(&self, s: &str) -> bool {
        let s = s.trim();
        s == self.write || self.aliases.iter().any(|a| a == s)
    }
}

fn plain(names: &[&str]) -> Vec<Rule> {
    names.iter().map(|n| Rule::new(n, &[])).collect()
}

/// The MCP server of an `mcp__<name>__*` row.
pub fn mcp_row_server(rule: &str) -> Option<&str> {
    let name = rule.strip_prefix("mcp__")?.strip_suffix("__*")?;
    crate::mcp::valid_name(name).then_some(name)
}

pub fn mcp_row(name: &str) -> String {
    format!("mcp__{name}__*")
}

/// The CLI's rules for a row; `None`: the CLI has no setting for it.
fn rules(kind: CliKind, gemini_cli: bool, row: &str) -> Option<Vec<Rule>> {
    match kind {
        CliKind::Claude => Some(match row {
            "Read / Grep" => plain(&["Read", "Grep", "Glob"]),
            "Edit / Write" => plain(&["Edit", "Write"]),
            "Bash(git *)" => vec![Rule::new("Bash(git *)", &["Bash(git:*)"])],
            "Bash(npm *, bun *)" => vec![Rule::new("Bash(npm *)", &["Bash(npm:*)"]), Rule::new("Bash(bun *)", &["Bash(bun:*)"])],
            "Bash(*)" => vec![Rule::new("Bash", &["Bash(*)"])],
            "WebFetch / WebSearch" => plain(&["WebFetch", "WebSearch"]),
            other => {
                let name = mcp_row_server(other)?;
                vec![Rule { write: format!("mcp__{name}"), aliases: vec![format!("mcp__{name}__*")] }]
            }
        }),
        // Gemini CLI's tool names, and agy's (from the same fixed table as `agents.rs`: a misspelled agy tool can
        // hang it). Both take a command prefix in parentheses on the shell tool.
        CliKind::Gemini if gemini_cli => Some(match row {
            "Read / Grep" => plain(&["read_file", "read_many_files", "list_directory", "glob", "search_file_content"]),
            "Edit / Write" => plain(&["write_file", "replace"]),
            "Bash(git *)" => plain(&["run_shell_command(git)"]),
            "Bash(npm *, bun *)" => plain(&["run_shell_command(npm)", "run_shell_command(bun)"]),
            "Bash(*)" => plain(&["run_shell_command"]),
            "WebFetch / WebSearch" => plain(&["web_fetch", "google_web_search"]),
            _ => return None,
        }),
        CliKind::Gemini => Some(match row {
            "Read / Grep" => plain(&["view_file", "list_dir", "grep_search", "find_by_name"]),
            "Edit / Write" => plain(&["write_to_file", "replace_file_content"]),
            "Bash(git *)" => plain(&["run_command(git)"]),
            "Bash(npm *, bun *)" => plain(&["run_command(npm)", "run_command(bun)"]),
            "Bash(*)" => plain(&["run_command"]),
            "WebFetch / WebSearch" => plain(&["read_url_content", "search_web"]),
            _ => return None,
        }),
        CliKind::Codex | CliKind::Other => None,
    }
}

fn label(v: PermValue) -> &'static str {
    match v {
        PermValue::Allow => "allow",
        PermValue::Ask => "ask",
        PermValue::Deny => "deny",
        PermValue::Unsupported => "unsupported",
    }
}

fn strictness(v: PermValue) -> u8 {
    match v {
        PermValue::Deny => 3,
        PermValue::Ask => 2,
        PermValue::Allow => 1,
        PermValue::Unsupported => 0,
    }
}

/// What the CLI does with a rule no file sets: read-only tools run, everything else asks.
fn default_value(row: &str) -> PermValue {
    if row == "Read / Grep" { PermValue::Allow } else { PermValue::Ask }
}

// ---- one settings file's lists

#[derive(Default)]
struct Lists {
    deny: Vec<String>,
    ask: Vec<String>,
    allow: Vec<String>,
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(String::from).collect()).unwrap_or_default()
}

fn claude_lists(root: &serde_json::Map<String, Value>) -> Lists {
    let p = root.get("permissions");
    Lists { deny: strings(p.and_then(|p| p.get("deny"))), ask: strings(p.and_then(|p| p.get("ask"))), allow: strings(p.and_then(|p| p.get("allow"))) }
}

/// Whether a Gemini settings file uses the older top-level `allowedTools` / `excludeTools`.
fn gemini_legacy(root: &serde_json::Map<String, Value>) -> bool {
    !root.get("tools").is_some_and(Value::is_object) && (root.contains_key("allowedTools") || root.contains_key("excludeTools"))
}

fn gemini_lists(root: &serde_json::Map<String, Value>) -> Lists {
    if gemini_legacy(root) {
        return Lists { deny: strings(root.get("excludeTools")), ask: vec![], allow: strings(root.get("allowedTools")) };
    }
    let t = root.get("tools");
    Lists { deny: strings(t.and_then(|t| t.get("exclude"))), ask: vec![], allow: strings(t.and_then(|t| t.get("allowed"))) }
}

/// The value one file gives a row, and a note when only some of the row's rules are in that list.
fn file_value(lists: &Lists, rules: &[Rule]) -> Option<(PermValue, Option<String>)> {
    for (value, list) in [(PermValue::Deny, &lists.deny), (PermValue::Ask, &lists.ask), (PermValue::Allow, &lists.allow)] {
        let hit: Vec<&Rule> = rules.iter().filter(|r| list.iter().any(|s| r.matches(s))).collect();
        if hit.is_empty() {
            continue;
        }
        let note = (hit.len() < rules.len()).then(|| {
            let missing: Vec<&str> = rules.iter().filter(|r| !hit.iter().any(|h| h.write == r.write)).map(|r| r.write.as_str()).collect();
            format!("partly set: {} not in {}", missing.join(", "), label(value))
        });
        return Some((value, note));
    }
    None
}

// ---- the matrix

/// The files one provider's column reads in a scope: (path, writable). Claude's project scope also reads the
/// shared `.claude/settings.json`.
fn column_files(config: &Config, name: &str, kind: CliKind, scope: ConfigScope, workspace: &Path) -> Result<Vec<(PathBuf, bool)>> {
    Ok(match kind {
        CliKind::Claude => {
            let mut v = vec![(cli_files::claude_settings(config, name, scope, workspace)?, true)];
            if scope == ConfigScope::Project {
                v.push((workspace.join(".claude/settings.json"), false));
            }
            v
        }
        CliKind::Gemini => vec![(cli_files::gemini_settings(config, name, scope, workspace)?, true)],
        CliKind::Codex => vec![(cli_files::codex_config(config, name, scope, workspace)?, false)],
        CliKind::Other => vec![],
    })
}

struct Layer {
    lists: Lists,
    /// Read only (Claude's shared project settings).
    shared: bool,
}

struct Column {
    name: String,
    kind: CliKind,
    gemini_cli: bool,
    machine: Vec<Layer>,
    project: Vec<Layer>,
}

fn load_layers(kind: CliKind, files: &[(PathBuf, bool)], provider: &str, scope: ConfigScope, out_files: &mut Vec<ConfigFile>) -> Vec<Layer> {
    let mut layers = vec![];
    for (path, writable) in files {
        let (lists, error) = match kind {
            CliKind::Codex => (Lists::default(), read_toml(path).err().map(|e| format!("{e:#}"))),
            _ => match read_json(path) {
                Ok(root) => (if kind == CliKind::Claude { claude_lists(&root) } else { gemini_lists(&root) }, None),
                Err(e) => (Lists::default(), Some(format!("{e:#}"))),
            },
        };
        out_files.push(file_entry(provider, scope, path, *writable && error.is_none() && kind != CliKind::Codex, error));
        layers.push(Layer { lists, shared: !writable });
    }
    layers
}

fn codex_note(config: &Config, name: &str, workspace: &Path, project: bool) -> String {
    let mut parts = vec![];
    let mut scopes = vec![ConfigScope::Machine];
    if project {
        scopes.insert(0, ConfigScope::Project);
    }
    for key in ["approval_policy", "sandbox_mode"] {
        let found = scopes.iter().find_map(|s| {
            let path = cli_files::codex_config(config, name, *s, workspace).ok()?;
            let doc = read_toml(&path).ok()?;
            doc.get(key).and_then(|v| v.as_str()).map(|v| format!("{key} = \"{v}\""))
        });
        parts.push(found.unwrap_or_else(|| format!("{key} not set")));
    }
    format!("Codex has no per-tool rules: {} in config.toml apply to every tool (MAP's launch flags may override them)", parts.join(", "))
}

fn column_note(config: &Config, name: &str, kind: CliKind, gemini_cli: bool, clis: &[(String, CliKind)], workspace: &Path, project: bool) -> Option<String> {
    match kind {
        CliKind::Codex => Some(codex_note(config, name, workspace, project)),
        CliKind::Other => Some(format!("MAP does not know where {name} keeps permission rules")),
        CliKind::Gemini => {
            let cli = if gemini_cli { "gemini" } else { "agy" };
            Some(format!("{cli} has no ask list: ask means the rule is in neither tools.allowed nor tools.exclude; MCP servers are trusted per server, not here"))
        }
        CliKind::Claude => {
            let others: Vec<&str> = clis.iter().filter(|(n, k)| *k == CliKind::Claude && n != name).map(|(n, _)| n.as_str()).collect();
            let mut note = "project rules go to .claude/settings.local.json; .claude/settings.json is read too".to_string();
            if project && !others.is_empty() {
                note.push_str(&format!("; shares the project files with {}", others.join(", ")));
            }
            Some(note)
        }
    }
}

/// The strictest value the layers give, the note of the layer that gave it, and whether a shared (read-only) layer
/// gave it.
fn layered(layers: &[Layer], rules: &[Rule]) -> Option<(PermValue, Option<String>, bool)> {
    let mut best: Option<(PermValue, Option<String>, bool)> = None;
    for l in layers {
        if let Some((v, note)) = file_value(&l.lists, rules)
            && best.as_ref().is_none_or(|b| strictness(v) > strictness(b.0))
        {
            best = Some((v, note, l.shared));
        }
    }
    best
}

fn cell(col: &Column, row: &str, scope: ConfigScope) -> PermCell {
    let provider = col.name.clone();
    let Some(rules) = rules(col.kind, col.gemini_cli, row) else {
        let note = match col.kind {
            CliKind::Codex => "Codex has no per-tool rules".to_string(),
            CliKind::Gemini => "set `trust` on the MCP server instead".to_string(),
            _ => "no permission settings MAP knows for this CLI".to_string(),
        };
        return PermCell { provider, value: PermValue::Unsupported, source: None, note: Some(note) };
    };
    let machine = layered(&col.machine, &rules);
    let project = if scope == ConfigScope::Project { layered(&col.project, &rules) } else { None };
    let shared_note = |s: bool| s.then(|| "from .claude/settings.json, which MAP does not edit".to_string());
    let join = |a: Option<String>, b: Option<String>| match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    };
    match (project, machine) {
        (None, None) => {
            let value = default_value(row);
            PermCell { provider, value, source: None, note: Some(format!("not set: the CLI's default ({})", label(value))) }
        }
        (None, Some((m, note, _))) => PermCell { provider, value: m, source: Some(ConfigSource::Machine), note },
        (Some((p, note, shared)), None) => PermCell { provider, value: p, source: Some(ConfigSource::Project), note: join(note, shared_note(shared)) },
        (Some((p, pnote, shared)), Some((m, mnote, _))) => {
            if p == m {
                PermCell { provider, value: p, source: Some(ConfigSource::Project), note: join(pnote, shared_note(shared)) }
            } else if strictness(p) > strictness(m) {
                PermCell { provider, value: p, source: Some(ConfigSource::Override), note: join(pnote, shared_note(shared)) }
            } else {
                let lost = format!("the project's {} does not apply: the machine's stricter {} wins", label(p), label(m));
                PermCell { provider, value: m, source: Some(ConfigSource::Machine), note: join(Some(lost), mnote) }
            }
        }
    }
}

/// The matrix for a scope. `workspace` is the project's folder (project scope) and `mcp_servers` the MCP servers
/// of that scope, one row each.
pub fn view(config: &Config, scope: ConfigScope, workspace: &Path, mcp_servers: &[String]) -> PermissionsView {
    let clis = cli_files::clis(config);
    let mut files = vec![];
    let mut columns = vec![];
    let mut providers: Vec<ConfigCli> = vec![];
    for (name, kind) in &clis {
        let gemini_cli = cli_files::is_gemini_cli(config, name);
        let mut machine = vec![];
        let mut project = vec![];
        match column_files(config, name, *kind, ConfigScope::Machine, workspace) {
            Ok(f) => machine = load_layers(*kind, &f, name, ConfigScope::Machine, &mut files),
            Err(e) => tracing::warn!("permissions for {name}: {e:#}"),
        }
        if scope == ConfigScope::Project
            && let Ok(f) = column_files(config, name, *kind, ConfigScope::Project, workspace)
        {
            project = load_layers(*kind, &f, name, ConfigScope::Project, &mut files);
        }
        providers.push(cli_files::cli_entry(name, *kind, column_note(config, name, *kind, gemini_cli, &clis, workspace, scope == ConfigScope::Project)));
        columns.push(Column { name: name.clone(), kind: *kind, gemini_cli, machine, project });
    }
    let mut rows: Vec<(String, String, Option<String>)> = ROWS.iter().map(|(r, d)| (r.to_string(), d.to_string(), None)).collect();
    for name in mcp_servers {
        rows.push((mcp_row(name), format!("{name} MCP tools"), Some(name.clone())));
    }
    let rows = rows.into_iter().map(|(rule, description, mcp_server)| PermRow { cells: columns.iter().map(|c| cell(c, &rule, scope)).collect(), rule, description, mcp_server }).collect();
    PermissionsView { project: None, scope, providers, rows, files }
}

/// Sets or clears cells in the scope's files. Every change is checked before any file is written.
pub fn apply(config: &Config, scope: ConfigScope, workspace: &Path, changes: &[PermChange]) -> Result<Vec<PathBuf>> {
    let clis = cli_files::clis(config);
    let mut plan = vec![];
    for c in changes {
        if !ROWS.iter().any(|(r, _)| *r == c.rule) && mcp_row_server(&c.rule).is_none() {
            bail!("unknown rule `{}`", c.rule);
        }
        let Some((_, kind)) = clis.iter().find(|(n, _)| *n == c.provider) else { bail!("`{}` is not a provider MAP runs", c.provider) };
        if c.value == Some(PermValue::Unsupported) {
            bail!("`unsupported` cannot be written; use allow, ask, deny or null");
        }
        let gemini_cli = cli_files::is_gemini_cli(config, &c.provider);
        let Some(rules) = rules(*kind, gemini_cli, &c.rule) else {
            bail!("{} has no setting for `{}`", c.provider, c.rule);
        };
        let path = match kind {
            CliKind::Claude => cli_files::claude_settings(config, &c.provider, scope, workspace)?,
            CliKind::Gemini => cli_files::gemini_settings(config, &c.provider, scope, workspace)?,
            _ => unreachable!("rules() is None for {kind:?}"),
        };
        plan.push((path, *kind, rules, c.value));
    }
    let mut written = vec![];
    for (path, kind, rules, value) in plan {
        let changed = edit_json(&path, |root| match kind {
            CliKind::Claude => set_claude(root, &rules, value, &path),
            _ => set_gemini(root, &rules, value, &path),
        })
        .with_context(|| format!("writing {}", path.display()))?;
        if changed && !written.contains(&path) {
            written.push(path);
        }
    }
    Ok(written)
}

/// Removes every spelling of the rules from `list`; whether anything went.
fn remove_rules(list: &mut Vec<Value>, rules: &[Rule]) -> bool {
    let before = list.len();
    list.retain(|v| !v.as_str().is_some_and(|s| rules.iter().any(|r| r.matches(s))));
    list.len() != before
}

fn push_rules(list: &mut Vec<Value>, rules: &[Rule]) {
    for r in rules {
        list.push(Value::String(r.write.clone()));
    }
}

fn set_claude(root: &mut serde_json::Map<String, Value>, rules: &[Rule], value: Option<PermValue>, path: &Path) -> Result<bool> {
    let mut changed = false;
    if let Some(perms) = root.get_mut("permissions") {
        let perms = perms.as_object_mut().with_context(|| format!("`permissions` in {} is not an object", path.display()))?;
        for key in ["allow", "ask", "deny"] {
            if let Some(list) = perms.get_mut(key) {
                let list = list.as_array_mut().with_context(|| format!("`permissions.{key}` in {} is not a list", path.display()))?;
                changed |= remove_rules(list, rules);
            }
        }
    }
    if let Some(v) = value {
        let perms = object_at(root, "permissions", path)?;
        push_rules(array_at(perms, label(v), path)?, rules);
        changed = true;
    }
    Ok(changed)
}

fn set_gemini(root: &mut serde_json::Map<String, Value>, rules: &[Rule], value: Option<PermValue>, path: &Path) -> Result<bool> {
    let legacy = gemini_legacy(root);
    let (allow_key, deny_key) = if legacy { ("allowedTools", "excludeTools") } else { ("allowed", "exclude") };
    let mut changed = false;
    {
        let lists: Option<&mut serde_json::Map<String, Value>> = if legacy {
            Some(&mut *root)
        } else {
            match root.get_mut("tools") {
                Some(t) => Some(t.as_object_mut().with_context(|| format!("`tools` in {} is not an object", path.display()))?),
                None => None,
            }
        };
        if let Some(lists) = lists {
            for key in [allow_key, deny_key] {
                if let Some(list) = lists.get_mut(key) {
                    let list = list.as_array_mut().with_context(|| format!("`{key}` in {} is not a list", path.display()))?;
                    changed |= remove_rules(list, rules);
                }
            }
        }
    }
    let key = match value {
        Some(PermValue::Allow) => allow_key,
        Some(PermValue::Deny) => deny_key,
        // No ask list: a rule in neither list asks.
        _ => return Ok(changed),
    };
    let lists = if legacy { root } else { object_at(root, "tools", path)? };
    push_rules(array_at(lists, key, path)?, rules);
    Ok(true)
}

#[cfg(test)]
pub(crate) mod tests;
