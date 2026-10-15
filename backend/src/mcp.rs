//! Configure → MCP servers: one list across the CLIs MAP runs, read from and written to each CLI's own file.
//!
//! | CLI | machine | project |
//! |---|---|---|
//! | Claude Code | top-level `mcpServers` of `.claude.json` (next to its config folder; nothing else in it is touched) | `<project>/.mcp.json` |
//! | Codex | `[mcp_servers.<name>]` in `config.toml` of its home | `<project>/.codex/config.toml` |
//! | Gemini / agy | `mcpServers` in `settings.json` of its `.gemini` folder | `<project>/.gemini/settings.json` |
//!
//! The same name in several files is one server; in project scope a project definition replaces the machine one of
//! the same CLI. Environment and header values are written but never returned (only their names and whether they
//! are set): they are often tokens. [`test`] starts a server and lists its tools (`initialize`,
//! `notifications/initialized`, `tools/list`) over stdio or streamable HTTP, within a time limit, and always ends
//! the process group it started.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use crate::cli_files::{self, edit_json, edit_toml, file_entry, object_at, read_json, read_toml};
use crate::config::Config;
use crate::model::{CliKind, ConfigCli, ConfigFile, ConfigScope, ConfigSource, McpKey, McpServer, McpServerInput, McpState, McpStatus, McpTestResult, McpTransport, McpView};

/// How long a test may take, start to `tools/list`.
pub const TEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TOOL_NAMES: usize = 30;
const PROTOCOL_VERSION: &str = "2025-06-18";

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_-]{0,63}$").unwrap());
static VAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]{0,127}$").unwrap());
static HEADER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9!#$%&'*+.^_`|~-]{1,128}$").unwrap());
/// `${VAR}` and `${VAR:-default}`, which Claude Code and Gemini expand in their MCP settings.
static REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}").unwrap());

pub fn valid_name(name: &str) -> bool {
    NAME.is_match(name)
}

/// A server as one CLI file defines it, values included (never sent to the dashboard).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Def {
    pub transport: McpTransport,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub url: Option<String>,
    pub env: BTreeMap<String, String>,
    pub headers: BTreeMap<String, String>,
    /// An SSE-only server (Claude `type: "sse"`, Gemini `url`).
    pub sse: bool,
    /// Codex `bearer_token_env_var`: the variable whose value is sent as `Authorization: Bearer …`.
    pub bearer_env: Option<String>,
    /// The CLI expands `${VAR}` in it (Claude Code, Gemini; not Codex).
    pub expands: bool,
}

/// Where a provider keeps MCP servers in a scope; `None` for a CLI MAP does not know.
fn mcp_file(config: &Config, name: &str, kind: CliKind, scope: ConfigScope, workspace: &Path) -> Option<Result<PathBuf>> {
    Some(match kind {
        CliKind::Claude => cli_files::claude_mcp(config, name, scope, workspace),
        CliKind::Codex => cli_files::codex_config(config, name, scope, workspace),
        CliKind::Gemini => cli_files::gemini_settings(config, name, scope, workspace),
        CliKind::Other => return None,
    })
}

/// The CLIs a server can be given to: those MAP runs whose MCP file it knows.
pub fn mcp_clis(config: &Config) -> Vec<(String, CliKind)> {
    cli_files::clis(config).into_iter().filter(|(_, k)| *k != CliKind::Other).collect()
}

// ---- parsing

fn str_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object).map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().map(String::from).unwrap_or_else(|| v.to_string()))).collect()).unwrap_or_default()
}

fn str_list(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect()).unwrap_or_default()
}

fn json_def(kind: CliKind, v: &Value) -> Option<Def> {
    let o = v.as_object()?;
    let s = |k: &str| o.get(k).and_then(Value::as_str).map(String::from);
    let mut def = Def {
        transport: McpTransport::Stdio,
        command: s("command"),
        args: str_list(o.get("args")),
        url: None,
        env: str_map(o.get("env")),
        headers: str_map(o.get("headers")),
        sse: false,
        bearer_env: None,
        expands: true,
    };
    match kind {
        CliKind::Claude => {
            let ty = s("type").unwrap_or_default();
            if ty == "http" || ty == "sse" || (def.command.is_none() && s("url").is_some()) {
                def.transport = McpTransport::Http;
                def.url = s("url");
                def.sse = ty == "sse";
            }
        }
        _ => {
            if let Some(url) = s("httpUrl") {
                def.transport = McpTransport::Http;
                def.url = Some(url);
            } else if def.command.is_none()
                && let Some(url) = s("url")
            {
                def.transport = McpTransport::Http;
                def.url = Some(url);
                def.sse = true;
            }
        }
    }
    Some(def)
}

fn toml_str_map(item: Option<&toml_edit::Item>) -> BTreeMap<String, String> {
    let Some(t) = item.and_then(|i| i.as_table_like()) else { return BTreeMap::new() };
    t.iter().map(|(k, v)| (k.to_string(), v.as_str().map(String::from).unwrap_or_else(|| v.to_string().trim().to_string()))).collect()
}

fn toml_def(item: &toml_edit::Item) -> Option<Def> {
    let t = item.as_table_like()?;
    let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(String::from);
    let args = t.get("args").and_then(|a| a.as_array()).map(|a| a.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect()).unwrap_or_default();
    let url = s("url");
    Some(Def {
        transport: if url.is_some() && s("command").is_none() { McpTransport::Http } else { McpTransport::Stdio },
        command: s("command"),
        args,
        url,
        env: toml_str_map(t.get("env")),
        headers: toml_str_map(t.get("http_headers")),
        sse: false,
        bearer_env: s("bearer_token_env_var"),
        expands: false,
    })
}

/// The servers of one file, by name.
fn read_file(kind: CliKind, path: &Path) -> Result<BTreeMap<String, Def>> {
    let mut out = BTreeMap::new();
    if kind == CliKind::Codex {
        let doc = read_toml(path)?;
        if let Some(servers) = doc.get("mcp_servers").and_then(|i| i.as_table_like()) {
            for (name, item) in servers.iter() {
                if let Some(def) = toml_def(item) {
                    out.insert(name.to_string(), def);
                }
            }
        }
    } else {
        let root = read_json(path)?;
        if let Some(servers) = root.get("mcpServers").and_then(Value::as_object) {
            for (name, v) in servers {
                if let Some(def) = json_def(kind, v) {
                    out.insert(name.clone(), def);
                }
            }
        }
    }
    Ok(out)
}

// ---- the list

/// One server definition in one provider's file.
#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub provider: String,
    pub scope: ConfigScope,
    pub def: Def,
}

pub struct Collected {
    pub clis: Vec<(String, CliKind)>,
    pub entries: Vec<Entry>,
    pub files: Vec<ConfigFile>,
}

impl Collected {
    /// The definition `provider` uses in `scope` (project scope: its project one, else its machine one).
    fn effective(&self, provider: &str, name: &str, scope: ConfigScope) -> Option<&Entry> {
        let find = |s: ConfigScope| self.entries.iter().find(|e| e.provider == provider && e.scope == s && e.name == name);
        match scope {
            ConfigScope::Project => find(ConfigScope::Project).or_else(|| find(ConfigScope::Machine)),
            ConfigScope::Machine => find(ConfigScope::Machine),
        }
    }
}

/// Reads every MCP file of the scope (and the machine's, for project scope).
pub fn collect(config: &Config, scope: ConfigScope, workspace: &Path) -> Collected {
    let clis = mcp_clis(config);
    let mut entries = vec![];
    let mut files = vec![];
    let scopes: &[ConfigScope] = match scope {
        ConfigScope::Machine => &[ConfigScope::Machine],
        ConfigScope::Project => &[ConfigScope::Project, ConfigScope::Machine],
    };
    for (provider, kind) in &clis {
        for s in scopes {
            let path = match mcp_file(config, provider, *kind, *s, workspace) {
                Some(Ok(p)) => p,
                Some(Err(e)) => {
                    tracing::warn!("mcp servers of {provider}: {e:#}");
                    continue;
                }
                None => continue,
            };
            match read_file(*kind, &path) {
                Ok(defs) => {
                    files.push(file_entry(provider, *s, &path, true, None));
                    for (name, def) in defs {
                        entries.push(Entry { name, provider: provider.clone(), scope: *s, def });
                    }
                }
                Err(e) => files.push(file_entry(provider, *s, &path, false, Some(format!("{e:#}")))),
            }
        }
    }
    Collected { clis, entries, files }
}

/// The server names a scope has, sorted.
pub fn names(c: &Collected) -> Vec<String> {
    let mut v: Vec<String> = c.entries.iter().map(|e| e.name.clone()).collect();
    v.sort();
    v.dedup();
    v
}

fn keys(map: &BTreeMap<String, String>) -> Vec<McpKey> {
    map.iter().map(|(k, v)| McpKey { key: k.clone(), set: !v.is_empty() }).collect()
}

fn differences(a: &Def, b: &Def) -> Vec<&'static str> {
    let mut d = vec![];
    if a.transport != b.transport {
        d.push("transport");
    }
    if a.command != b.command {
        d.push("command");
    }
    if a.args != b.args {
        d.push("args");
    }
    if a.url != b.url {
        d.push("url");
    }
    if a.env != b.env {
        d.push("env");
    }
    if a.headers != b.headers {
        d.push("headers");
    }
    d
}

/// A server's last test, as shown on its card.
pub fn status_of(result: Option<&Tested>) -> McpStatus {
    match result {
        None => McpStatus { state: McpState::NotTested, label: "not tested".into(), tools: None, tested_at: None },
        Some((r, at)) if r.ok => McpStatus { state: McpState::Connected, label: format!("connected · {} tools", r.tools), tools: Some(r.tools), tested_at: Some(*at) },
        Some((r, at)) => McpStatus {
            state: McpState::Failed,
            label: format!("failed: {}", r.error.as_deref().unwrap_or("unknown error")),
            tools: None,
            tested_at: Some(*at),
        },
    }
}

pub fn view(c: &Collected, scope: ConfigScope, status: impl Fn(&str) -> McpStatus) -> McpView {
    let mut servers = vec![];
    for name in names(c) {
        let mut providers = vec![];
        let mut shown: Option<&Entry> = None;
        let mut notes = vec![];
        for (p, _) in &c.clis {
            let Some(e) = c.effective(p, &name, scope) else { continue };
            providers.push(p.clone());
            match shown {
                None => shown = Some(e),
                Some(first) => {
                    let d = differences(&first.def, &e.def);
                    if !d.is_empty() {
                        notes.push(format!("{p}'s definition differs from {}'s ({})", first.provider, d.join(", ")));
                    }
                }
            }
        }
        let Some(shown) = shown else { continue };
        let of = |s: ConfigScope| c.entries.iter().filter(|e| e.scope == s && e.name == name).collect::<Vec<_>>();
        let (in_project, in_machine) = (of(ConfigScope::Project), of(ConfigScope::Machine));
        let source = match (scope, in_project.first(), in_machine.first()) {
            (ConfigScope::Machine, ..) => ConfigSource::Machine,
            (_, Some(p), Some(m)) if p.def != m.def => ConfigSource::Override,
            (_, Some(_), _) => ConfigSource::Project,
            _ => ConfigSource::Machine,
        };
        let def = &shown.def;
        if def.sse {
            notes.push("an SSE server: the test speaks streamable HTTP, which an SSE-only server may not answer".into());
        }
        if let Some(var) = &def.bearer_env {
            notes.push(format!("Codex sends ${var} as its bearer token"));
        }
        servers.push(McpServer {
            name: name.clone(),
            transport: def.transport,
            command: def.command.clone(),
            args: def.args.clone(),
            url: def.url.clone(),
            env_keys: keys(&def.env),
            header_keys: keys(&def.headers),
            providers,
            source,
            status: status(&name),
            notes,
        });
    }
    let providers: Vec<ConfigCli> = c
        .clis
        .iter()
        .map(|(n, k)| {
            let note = match k {
                CliKind::Claude => Some("project servers in .mcp.json run once each person approves them in Claude Code".to_string()),
                CliKind::Codex => Some("Codex reads a project's .codex/config.toml only in folders it trusts".to_string()),
                _ => None,
            };
            cli_files::cli_entry(n, *k, note)
        })
        .collect();
    McpView { project: None, scope, providers, servers, files: c.files.clone() }
}

/// The definition `POST /api/mcp/test` runs: the first CLI's (in provider order) that has it in the scope.
pub fn def_for_test(c: &Collected, scope: ConfigScope, name: &str) -> Option<Def> {
    c.clis.iter().find_map(|(p, _)| c.effective(p, name, scope)).map(|e| e.def.clone())
}

// ---- writing

fn def_from_input(c: &Collected, input: &McpServerInput) -> Result<Def> {
    if !valid_name(&input.name) {
        bail!("a server name is letters, digits, - and _ (up to 64), starting with a letter or digit");
    }
    let existing = |pick: fn(&Def) -> &BTreeMap<String, String>, key: &str| -> Option<String> {
        c.entries.iter().filter(|e| e.name == input.name).find_map(|e| pick(&e.def).get(key).cloned())
    };
    let mut env = BTreeMap::new();
    for v in &input.env {
        if !VAR.is_match(&v.key) {
            bail!("`{}` is not a valid environment variable name", v.key);
        }
        let value = match &v.value {
            Some(s) => s.clone(),
            None => existing(|d| &d.env, &v.key).with_context(|| format!("env `{}` needs a value (the server has none to keep)", v.key))?,
        };
        if value.contains('\0') {
            bail!("env `{}` contains a NUL byte", v.key);
        }
        env.insert(v.key.clone(), value);
    }
    let mut headers = BTreeMap::new();
    for v in &input.headers {
        if !HEADER.is_match(&v.key) {
            bail!("`{}` is not a valid header name", v.key);
        }
        let value = match &v.value {
            Some(s) => s.clone(),
            None => existing(|d| &d.headers, &v.key).with_context(|| format!("header `{}` needs a value (the server has none to keep)", v.key))?,
        };
        if value.contains(['\r', '\n', '\0']) {
            bail!("header `{}` contains a line break", v.key);
        }
        headers.insert(v.key.clone(), value);
    }
    let trimmed = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(String::from);
    let def = match input.transport {
        McpTransport::Stdio => {
            let command = trimmed(&input.command).context("a stdio server needs a command")?;
            Def { transport: McpTransport::Stdio, command: Some(command), args: input.args.clone(), url: None, env, headers: BTreeMap::new(), sse: false, bearer_env: None, expands: true }
        }
        McpTransport::Http => {
            let url = trimmed(&input.url).context("an http server needs a url")?;
            if !(url.starts_with("https://") || url.starts_with("http://")) {
                bail!("the url must start with https:// or http://");
            }
            Def { transport: McpTransport::Http, command: None, args: vec![], url: Some(url), env: BTreeMap::new(), headers, sse: false, bearer_env: None, expands: true }
        }
    };
    Ok(def)
}

fn json_map(m: &BTreeMap<String, String>) -> Value {
    Value::Object(m.iter().map(|(k, v)| (k.clone(), Value::String(v.clone()))).collect())
}

fn set_or_remove(o: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    match value {
        Some(v) => {
            o.insert(key.into(), v);
        }
        None => {
            o.remove(key);
        }
    }
}

/// Writes `def` into a JSON entry, keeping the entry's other keys (`timeout`, `trust`, `includeTools`, …).
fn put_json(kind: CliKind, o: &mut Map<String, Value>, def: &Def) {
    let nonempty = |m: &BTreeMap<String, String>| (!m.is_empty()).then(|| json_map(m));
    match def.transport {
        McpTransport::Stdio => {
            if kind == CliKind::Claude {
                o.insert("type".into(), json!("stdio"));
            }
            o.insert("command".into(), json!(def.command));
            o.insert("args".into(), json!(def.args));
            set_or_remove(o, "env", nonempty(&def.env));
            for k in ["url", "httpUrl", "headers"] {
                o.remove(k);
            }
        }
        McpTransport::Http => {
            if kind == CliKind::Claude {
                o.insert("type".into(), json!("http"));
                o.insert("url".into(), json!(def.url));
            } else {
                o.remove("url");
                o.insert("httpUrl".into(), json!(def.url));
            }
            set_or_remove(o, "headers", nonempty(&def.headers));
            for k in ["command", "args", "env"] {
                o.remove(k);
            }
        }
    }
}

fn toml_table(m: &BTreeMap<String, String>) -> toml_edit::Item {
    let mut t = toml_edit::InlineTable::new();
    for (k, v) in m {
        t.insert(k, v.as_str().into());
    }
    toml_edit::value(t)
}

fn put_toml(t: &mut dyn toml_edit::TableLike, def: &Def) {
    let nonempty = |m: &BTreeMap<String, String>| (!m.is_empty()).then(|| toml_table(m));
    let set = |t: &mut dyn toml_edit::TableLike, k: &str, v: Option<toml_edit::Item>| match v {
        Some(v) => {
            t.insert(k, v);
        }
        None => {
            t.remove(k);
        }
    };
    match def.transport {
        McpTransport::Stdio => {
            set(t, "command", def.command.as_deref().map(toml_edit::value));
            let args: toml_edit::Array = def.args.iter().map(String::as_str).collect();
            set(t, "args", Some(toml_edit::value(args)));
            set(t, "env", nonempty(&def.env));
            for k in ["url", "http_headers", "bearer_token_env_var", "env_http_headers"] {
                t.remove(k);
            }
        }
        McpTransport::Http => {
            set(t, "url", def.url.as_deref().map(toml_edit::value));
            set(t, "http_headers", nonempty(&def.headers));
            for k in ["command", "args", "env", "env_vars", "cwd"] {
                t.remove(k);
            }
        }
    }
}

fn upsert(kind: CliKind, path: &Path, name: &str, def: &Def) -> Result<bool> {
    if kind == CliKind::Codex {
        return edit_toml(path, |doc| {
            let servers = doc.entry("mcp_servers").or_insert_with(toml_edit::table);
            if let Some(t) = servers.as_table_mut()
                && t.is_empty()
            {
                t.set_implicit(true);
            }
            let servers = servers.as_table_like_mut().context("`mcp_servers` is not a table")?;
            if servers.get(name).is_none_or(|i| i.as_table_like().is_none()) {
                servers.insert(name, toml_edit::table());
            }
            let t = servers.get_mut(name).and_then(|i| i.as_table_like_mut()).context("server entry is not a table")?;
            put_toml(t, def);
            Ok(true)
        });
    }
    edit_json(path, |root| {
        let servers = object_at(root, "mcpServers", path)?;
        let entry = servers.entry(name.to_string()).or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        put_json(kind, entry.as_object_mut().expect("an object"), def);
        Ok(true)
    })
}

fn remove(kind: CliKind, path: &Path, name: &str) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    if kind == CliKind::Codex {
        return edit_toml(path, |doc| Ok(doc.get_mut("mcp_servers").and_then(|i| i.as_table_like_mut()).is_some_and(|t| t.remove(name).is_some())));
    }
    edit_json(path, |root| Ok(root.get_mut("mcpServers").and_then(Value::as_object_mut).is_some_and(|o| o.remove(name).is_some())))
}

/// The scope's MCP files, each once, with the dialect and the providers that read it.
fn scope_files(config: &Config, c: &Collected, scope: ConfigScope, workspace: &Path) -> Result<Vec<(PathBuf, CliKind, Vec<String>)>> {
    let mut out: Vec<(PathBuf, CliKind, Vec<String>)> = vec![];
    for (p, k) in &c.clis {
        let Some(path) = mcp_file(config, p, *k, scope, workspace) else { continue };
        let path = path?;
        match out.iter_mut().find(|(q, _, _)| *q == path) {
            Some((_, _, ps)) => ps.push(p.clone()),
            None => out.push((path, *k, vec![p.clone()])),
        }
    }
    Ok(out)
}

/// Writes the server into the scope's file of each provider in `providers` and removes it from the scope's other
/// files. Returns the files written.
pub fn save(config: &Config, scope: ConfigScope, workspace: &Path, input: &McpServerInput, providers: &[String]) -> Result<Vec<PathBuf>> {
    let c = collect(config, scope, workspace);
    if providers.is_empty() {
        bail!("pick at least one CLI (remove the server to take it from all of them)");
    }
    if let Some(p) = providers.iter().find(|p| !c.clis.iter().any(|(n, _)| n == *p)) {
        bail!("`{p}` is not a CLI MAP runs that has MCP settings");
    }
    let def = def_from_input(&c, input)?;
    let mut written = vec![];
    for (path, kind, readers) in scope_files(config, &c, scope, workspace)? {
        let changed = if readers.iter().any(|r| providers.contains(r)) { upsert(kind, &path, &input.name, &def)? } else { remove(kind, &path, &input.name)? };
        if changed {
            written.push(path);
        }
    }
    Ok(written)
}

/// Removes the server from every file of the scope; the files it was in (empty: not found).
pub fn delete(config: &Config, scope: ConfigScope, workspace: &Path, name: &str) -> Result<Vec<PathBuf>> {
    if !valid_name(name) {
        bail!("`{name}` is not a server name");
    }
    let c = collect(config, scope, workspace);
    let mut removed = vec![];
    for (path, kind, _) in scope_files(config, &c, scope, workspace)? {
        if remove(kind, &path, name)? {
            removed.push(path);
        }
    }
    Ok(removed)
}

// ---- the connection test

/// The last test of each server per scope (`"machine"` or `"project:<slug>"`), until MAP restarts.
#[derive(Default)]
pub struct Tests(Mutex<HashMap<(String, String), Tested>>);

/// A test's result and when it ran.
type Tested = (McpTestResult, DateTime<Utc>);

impl Tests {
    pub fn status(&self, scope: &str, name: &str) -> McpStatus {
        status_of(self.0.lock().unwrap().get(&(scope.to_string(), name.to_string())))
    }

    pub fn record(&self, scope: &str, name: &str, result: &McpTestResult) {
        self.0.lock().unwrap().insert((scope.to_string(), name.to_string()), (result.clone(), Utc::now()));
    }

    pub fn forget(&self, scope: &str, name: &str) {
        self.0.lock().unwrap().remove(&(scope.to_string(), name.to_string()));
    }
}

pub fn scope_key(project: Option<&str>) -> String {
    match project {
        Some(p) => format!("project:{p}"),
        None => "machine".into(),
    }
}

/// `${VAR}` / `${VAR:-default}` read from `lookup`, for a CLI that expands them.
fn expand(def: &Def, s: &str, lookup: &(dyn Fn(&str) -> Option<String> + Sync)) -> String {
    if !def.expands {
        return s.to_string();
    }
    REF.replace_all(s, |c: &regex::Captures| lookup(&c[1]).or_else(|| c.get(2).map(|d| d.as_str().to_string())).unwrap_or_default()).into_owned()
}

/// Replaces every secret-looking value of the server in `text` (a server may echo its token in an error).
fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets.iter().filter(|s| s.len() >= 4) {
        out = out.replace(s.as_str(), "***");
    }
    out
}

fn short(text: &str) -> String {
    let t = text.trim();
    if t.chars().count() > 300 { format!("{}…", t.chars().take(300).collect::<String>()) } else { t.to_string() }
}

fn tool_names(result: &Value) -> (Vec<String>, Option<String>) {
    let names = result.get("tools").and_then(Value::as_array).map(|a| a.iter().filter_map(|t| t.get("name").and_then(Value::as_str)).map(String::from).collect()).unwrap_or_default();
    (names, result.get("nextCursor").and_then(Value::as_str).filter(|c| !c.is_empty()).map(String::from))
}

fn initialize() -> Value {
    json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": PROTOCOL_VERSION, "capabilities": {}, "clientInfo": { "name": "map", "version": env!("CARGO_PKG_VERSION") } } })
}

fn tools_list(id: u64, cursor: Option<&str>) -> Value {
    match cursor {
        Some(c) => json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": { "cursor": c } }),
        None => json!({ "jsonrpc": "2.0", "id": id, "method": "tools/list", "params": {} }),
    }
}

const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;

/// The `result` of a JSON-RPC response, or its error.
fn result_of(msg: Value) -> Result<Value> {
    if let Some(e) = msg.get("error") {
        bail!("{}", e.get("message").and_then(Value::as_str).map(String::from).unwrap_or_else(|| e.to_string()));
    }
    msg.get("result").cloned().context("a response without a result")
}

fn finish(started: Instant, outcome: Result<Vec<String>>, secrets: &[String]) -> McpTestResult {
    let duration_ms = started.elapsed().as_millis() as u64;
    match outcome {
        Ok(names) => McpTestResult { ok: true, tools: names.len() as u32, tool_names: names.into_iter().take(MAX_TOOL_NAMES).collect(), error: None, duration_ms },
        Err(e) => McpTestResult { ok: false, tools: 0, tool_names: vec![], error: Some(short(&redact(&format!("{e:#}"), secrets))), duration_ms },
    }
}

/// Starts the server and lists its tools within `limit`. `cwd` is where a stdio server runs (the project's folder),
/// `lookup` reads a variable as MAP's own commands see it ([`Config::env_value`]).
pub async fn test(def: &Def, cwd: &Path, limit: Duration, http: &reqwest::Client, lookup: &(dyn Fn(&str) -> Option<String> + Sync)) -> McpTestResult {
    let started = Instant::now();
    let env: Vec<(String, String)> = def.env.iter().map(|(k, v)| (k.clone(), expand(def, v, lookup))).collect();
    let mut headers: Vec<(String, String)> = def.headers.iter().map(|(k, v)| (k.clone(), expand(def, v, lookup))).collect();
    if let Some(token) = def.bearer_env.as_deref().and_then(lookup) {
        headers.push(("Authorization".into(), format!("Bearer {token}")));
    }
    let secrets: Vec<String> = env.iter().chain(&headers).map(|(_, v)| v.clone()).chain(def.env.values().cloned()).chain(def.headers.values().cloned()).collect();
    let outcome = match def.transport {
        McpTransport::Stdio => test_stdio(def, &env, cwd, limit, lookup).await,
        McpTransport::Http => match def.url.as_deref() {
            Some(url) => match tokio::time::timeout(limit, test_http(http, &expand(def, url, lookup), &headers)).await {
                Ok(r) => r,
                Err(_) => Err(anyhow!("no answer within {} s", limit.as_secs_f32())),
            },
            None => Err(anyhow!("the server has no url")),
        },
    };
    finish(started, outcome, &secrets)
}

async fn test_stdio(def: &Def, env: &[(String, String)], cwd: &Path, limit: Duration, lookup: &(dyn Fn(&str) -> Option<String> + Sync)) -> Result<Vec<String>> {
    let command = expand(def, def.command.as_deref().context("the server has no command")?, lookup);
    let args: Vec<String> = def.args.iter().map(|a| expand(def, a, lookup)).collect();
    let mut cmd = tokio::process::Command::new(&command);
    cmd.args(&args).envs(env.iter().map(|(k, v)| (k, v))).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    if cwd.is_dir() {
        cmd.current_dir(cwd);
    }
    // Its own process group, so whatever it starts (npx, uvx, a shell) ends with it.
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = cmd.spawn().with_context(|| format!("could not start `{command}`"))?;
    let pid = child.id();
    let stderr = child.stderr.take().map(|mut err| {
        tokio::spawn(async move {
            let mut buf = vec![];
            let _ = (&mut err).take(64 * 1024).read_to_end(&mut buf).await;
            String::from_utf8_lossy(&buf).into_owned()
        })
    });
    let mut stdin = child.stdin.take().context("no stdin")?;
    let mut lines = BufReader::new(child.stdout.take().context("no stdout")?).lines();
    let talk = async {
        async fn send(stdin: &mut tokio::process::ChildStdin, msg: &str) -> Result<()> {
            stdin.write_all(msg.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            stdin.flush().await?;
            Ok(())
        }
        async fn answer(lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>, id: u64) -> Result<Value> {
            while let Some(line) = lines.next_line().await? {
                let Ok(msg) = serde_json::from_str::<Value>(line.trim()) else { continue };
                if msg.get("id").and_then(Value::as_u64) == Some(id) && (msg.get("result").is_some() || msg.get("error").is_some()) {
                    return result_of(msg);
                }
            }
            bail!("the server closed its output before answering")
        }
        send(&mut stdin, &initialize().to_string()).await?;
        answer(&mut lines, 1).await.context("initialize")?;
        send(&mut stdin, INITIALIZED).await?;
        let mut names = vec![];
        let mut cursor: Option<String> = None;
        for id in 2..22u64 {
            send(&mut stdin, &tools_list(id, cursor.as_deref()).to_string()).await?;
            let (page, next) = tool_names(&answer(&mut lines, id).await.context("tools/list")?);
            names.extend(page);
            match next {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Ok::<_, anyhow::Error>(names)
    };
    let outcome = match tokio::time::timeout(limit, talk).await {
        Ok(r) => r,
        Err(_) => Err(anyhow!("no answer within {} s", limit.as_secs_f32())),
    };
    // Always end it: the whole group, then the child itself, and reap it.
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 0) {
        // SAFETY: kill(2) on the process group this test created (process_group(0) made the child its leader).
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill().await;
    let _ = child.wait().await;
    let err_text = match stderr {
        Some(h) => tokio::time::timeout(Duration::from_secs(1), h).await.ok().and_then(|r| r.ok()).unwrap_or_default(),
        None => String::new(),
    };
    outcome.map_err(|e| {
        let tail = err_text.lines().rev().find(|l| !l.trim().is_empty()).map(str::trim).unwrap_or_default();
        if tail.is_empty() { e } else { anyhow!("{e:#} (stderr: {tail})") }
    })
}

/// One JSON-RPC message over streamable HTTP: the reply (JSON, or the matching event of an SSE stream) and the
/// session id the server gave. `id: None` is a notification (any 2xx is fine).
async fn post(http: &reqwest::Client, url: &str, headers: &[(String, String)], session: Option<&str>, body: &Value, id: Option<u64>) -> Result<(Option<Value>, Option<String>)> {
    let mut req = http.post(url).header("Content-Type", "application/json").header("Accept", "application/json, text/event-stream");
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(s) = session {
        req = req.header("Mcp-Session-Id", s).header("MCP-Protocol-Version", PROTOCOL_VERSION);
    }
    let mut resp = req.body(body.to_string()).send().await.map_err(|e| anyhow!("{}", e.without_url()))?;
    let status = resp.status();
    let session = resp.headers().get("mcp-session-id").and_then(|v| v.to_str().ok()).map(String::from);
    if !status.is_success() {
        bail!("HTTP {status}");
    }
    let Some(id) = id else { return Ok((None, session)) };
    let sse = resp.headers().get("content-type").and_then(|v| v.to_str().ok()).is_some_and(|t| t.starts_with("text/event-stream"));
    let matching = |v: Value| -> Option<Value> {
        let items = match v {
            Value::Array(a) => a,
            other => vec![other],
        };
        items.into_iter().find(|m| m.get("id").and_then(Value::as_u64) == Some(id))
    };
    if !sse {
        let text = resp.text().await?;
        let v: Value = serde_json::from_str(&text).context("the reply is not JSON")?;
        return Ok((Some(matching(v).context("the reply does not answer the request")?), session));
    }
    // An event stream may stay open after the answer: read events as they come.
    let mut buf = String::new();
    while let Some(chunk) = resp.chunk().await? {
        buf.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(end) = buf.find("\n\n").or_else(|| buf.find("\r\n\r\n")) {
            let event: String = buf.drain(..end).collect();
            buf = buf.trim_start_matches(['\r', '\n']).to_string();
            let data: Vec<&str> = event.lines().filter_map(|l| l.strip_prefix("data:")).map(str::trim_start).collect();
            if let Ok(v) = serde_json::from_str::<Value>(&data.join("\n"))
                && let Some(m) = matching(v)
            {
                return Ok((Some(m), session));
            }
        }
    }
    bail!("the event stream ended before the answer")
}

async fn test_http(http: &reqwest::Client, url: &str, headers: &[(String, String)]) -> Result<Vec<String>> {
    let (reply, session) = post(http, url, headers, None, &initialize(), Some(1)).await.context("initialize")?;
    result_of(reply.unwrap_or_default()).context("initialize")?;
    let session = session.as_deref();
    post(http, url, headers, session, &serde_json::from_str(INITIALIZED)?, None).await.context("notifications/initialized")?;
    let mut names = vec![];
    let mut cursor: Option<String> = None;
    for id in 2..22u64 {
        let (reply, _) = post(http, url, headers, session, &tools_list(id, cursor.as_deref()), Some(id)).await.context("tools/list")?;
        let (page, next) = tool_names(&result_of(reply.unwrap_or_default()).context("tools/list")?);
        names.extend(page);
        match next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests;
