//! The provider CLIs' own settings files that the Configure screen's Permissions and MCP tabs read and write: which
//! CLIs MAP runs, which dialect each speaks, where its machine and project files are, and JSON / TOML editing that
//! keeps every key MAP does not own.
//!
//! Machine files live in the CLI's state folder as the CLI sees it ([`Config::provider_home`]): in
//! `[agents_home] mode = "isolated"` that is MAP's own agent home, never the owner's `~/.claude` or `~/.codex`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::config::{Config, TrustFormat};
use crate::model::{CliKind, ConfigCli, ConfigFile, ConfigScope};

/// The providers MAP runs: `orchestrator.provider_order`, then the audit chain, each once, configured ones only.
pub fn clis(config: &Config) -> Vec<(String, CliKind)> {
    let o = &config.orchestrator;
    let mut out: Vec<(String, CliKind)> = vec![];
    for name in o.provider_order.iter().chain(&o.audit.chain) {
        if config.providers.contains_key(name) && !out.iter().any(|(n, _)| n == name) {
            out.push((name.clone(), kind_of(config, name)));
        }
    }
    out
}

/// The dialect of a provider's settings: from its trust format, else its binary's name.
pub fn kind_of(config: &Config, name: &str) -> CliKind {
    let Some(spec) = config.providers.get(name) else { return CliKind::Other };
    match spec.trust.as_ref().map(|t| t.format) {
        Some(TrustFormat::ClaudeJson | TrustFormat::ClaudeJsonOnboarded) => return CliKind::Claude,
        Some(TrustFormat::CodexToml) => return CliKind::Codex,
        _ => {}
    }
    let binary = Path::new(&spec.binary).file_name().and_then(|b| b.to_str()).unwrap_or_default();
    match binary {
        "claude" => CliKind::Claude,
        "codex" => CliKind::Codex,
        "agy" | "gemini" => CliKind::Gemini,
        _ if name == "gemini" => CliKind::Gemini,
        _ => CliKind::Other,
    }
}

/// Whether the provider runs Google's `gemini` CLI rather than `agy` (they name their tools differently).
pub fn is_gemini_cli(config: &Config, name: &str) -> bool {
    config.providers.get(name).and_then(|s| Path::new(&s.binary).file_name()).and_then(|b| b.to_str()) == Some("gemini")
}

pub fn cli_entry(name: &str, kind: CliKind, note: Option<String>) -> ConfigCli {
    ConfigCli { name: name.to_string(), kind, note }
}

/// The provider's state folder, or an error naming the provider (a folder MAP cannot place is never guessed).
fn home(config: &Config, name: &str) -> Result<PathBuf> {
    config.provider_home(name).with_context(|| format!("provider `{name}` has no state folder (home_env / default_home)"))
}

/// Claude Code's settings with permission rules: `settings.json` in its home (machine), `.claude/settings.local.json`
/// in the project (MAP writes this one; `.claude/settings.json` is the shared one it only reads).
pub fn claude_settings(config: &Config, name: &str, scope: ConfigScope, workspace: &Path) -> Result<PathBuf> {
    Ok(match scope {
        ConfigScope::Machine => home(config, name)?.join("settings.json"),
        ConfigScope::Project => workspace.join(".claude/settings.local.json"),
    })
}

/// Claude Code's MCP servers: top-level `mcpServers` of `.claude.json` (machine; the file next to its config folder,
/// as for folder trust) and `.mcp.json` in the project.
pub fn claude_mcp(config: &Config, name: &str, scope: ConfigScope, workspace: &Path) -> Result<PathBuf> {
    if scope == ConfigScope::Project {
        return Ok(workspace.join(".mcp.json"));
    }
    if let Some(trust) = config.providers.get(name).and_then(|s| s.trust.as_ref()).filter(|t| matches!(t.format, TrustFormat::ClaudeJson | TrustFormat::ClaudeJsonOnboarded)) {
        return Ok(crate::trust::trust_file(config, name, trust));
    }
    match config.provider_home_var(name) {
        Some(dir) => Ok(dir.join(".claude.json")),
        None => Ok(crate::login::expand_home("~/.claude.json")),
    }
}

/// Codex's `config.toml`: in its home (machine) and `.codex/config.toml` in the project.
pub fn codex_config(config: &Config, name: &str, scope: ConfigScope, workspace: &Path) -> Result<PathBuf> {
    Ok(match scope {
        ConfigScope::Machine => home(config, name)?.join("config.toml"),
        ConfigScope::Project => workspace.join(".codex/config.toml"),
    })
}

/// Gemini / agy `settings.json`: in its `.gemini` folder (machine) and `.gemini/settings.json` in the project.
pub fn gemini_settings(config: &Config, name: &str, scope: ConfigScope, workspace: &Path) -> Result<PathBuf> {
    Ok(match scope {
        ConfigScope::Machine => home(config, name)?.join("settings.json"),
        ConfigScope::Project => workspace.join(".gemini/settings.json"),
    })
}

pub fn file_entry(provider: &str, scope: ConfigScope, path: &Path, writable: bool, error: Option<String>) -> ConfigFile {
    ConfigFile { provider: provider.to_string(), scope, path: path.display().to_string(), exists: path.is_file(), writable, error }
}

// ---- reading

/// Larger files are not read (a `.claude.json` with a long history is still far below this).
const MAX_BYTES: u64 = 16 * 1024 * 1024;

/// The file's text; `None` when it does not exist.
pub fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::metadata(path) {
        Ok(m) if m.len() > MAX_BYTES => bail!("{} is larger than {} MB; left alone", path.display(), MAX_BYTES >> 20),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
    }
    match std::fs::read_to_string(path) {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// A JSON object file; missing or empty is an empty object. Anything else that is not an object is an error.
pub fn read_json(path: &Path) -> Result<Map<String, Value>> {
    let Some(text) = read_text(path)? else { return Ok(Map::new()) };
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(&text).with_context(|| format!("{} is not valid JSON; fix it by hand first", path.display()))? {
        Value::Object(map) => Ok(map),
        _ => bail!("{} is not a JSON object", path.display()),
    }
}

pub fn read_toml(path: &Path) -> Result<toml_edit::DocumentMut> {
    let text = read_text(path)?.unwrap_or_default();
    text.parse::<toml_edit::DocumentMut>().with_context(|| format!("{} is not valid TOML; fix it by hand first", path.display()))
}

// ---- writing

/// Attempts at a write that the CLI's own rewrite of the file undid in between (Claude Code rewrites `.claude.json`).
const ATTEMPTS: usize = 3;

/// Rewrites `path` from its current text: `edit` gets the text (`None`: no file) and returns the new text, or `None`
/// to leave it. The write is atomic (temp file + rename, the old mode kept) and is retried when the file changed
/// while MAP was editing it. Returns whether it wrote.
pub fn rewrite(path: &Path, mut edit: impl FnMut(Option<&str>) -> Result<Option<String>>) -> Result<bool> {
    #[cfg(test)]
    if !path.starts_with(std::env::temp_dir()) {
        bail!("tests only write CLI settings under the temp dir, not {}", path.display());
    }
    // A symlinked settings file is written where it points, so the link survives.
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for _ in 0..ATTEMPTS {
        let original = read_text(&path)?;
        let Some(updated) = edit(original.as_deref())? else { return Ok(false) };
        if original.as_deref() == Some(updated.as_str()) {
            return Ok(false);
        }
        if crate::trust::replace_if_unchanged(&path, original.as_deref().map(str::as_bytes), updated.as_bytes())? {
            return Ok(true);
        }
    }
    bail!("{} kept changing while MAP wrote it; try again", path.display())
}

/// Edits a JSON object file in place: `edit` changes the object and says whether anything changed. Pretty-printed
/// with a final newline; keys MAP does not touch stay.
pub fn edit_json(path: &Path, mut edit: impl FnMut(&mut Map<String, Value>) -> Result<bool>) -> Result<bool> {
    rewrite(path, |text| {
        let mut root = match text.map(str::trim).filter(|t| !t.is_empty()) {
            None => Map::new(),
            Some(t) => match serde_json::from_str::<Value>(t).with_context(|| format!("{} is not valid JSON; fix it by hand first", path.display()))? {
                Value::Object(map) => map,
                _ => bail!("{} is not a JSON object", path.display()),
            },
        };
        if !edit(&mut root)? {
            return Ok(None);
        }
        Ok(Some(format!("{}\n", serde_json::to_string_pretty(&Value::Object(root))?)))
    })
}

/// Edits a TOML file in place with `toml_edit`, so comments and every other table stay as they were.
pub fn edit_toml(path: &Path, mut edit: impl FnMut(&mut toml_edit::DocumentMut) -> Result<bool>) -> Result<bool> {
    rewrite(path, |text| {
        let mut doc = text.unwrap_or_default().parse::<toml_edit::DocumentMut>().with_context(|| format!("{} is not valid TOML; fix it by hand first", path.display()))?;
        if !edit(&mut doc)? {
            return Ok(None);
        }
        Ok(Some(doc.to_string()))
    })
}

/// `obj[key]` as an object, created when missing; an error when it is something else.
pub fn object_at<'a>(obj: &'a mut Map<String, Value>, key: &str, file: &Path) -> Result<&'a mut Map<String, Value>> {
    let v = obj.entry(key).or_insert_with(|| Value::Object(Map::new()));
    v.as_object_mut().with_context(|| format!("`{key}` in {} is not an object", file.display()))
}

/// `obj[key]` as a list, created when missing; an error when it is something else.
pub fn array_at<'a>(obj: &'a mut Map<String, Value>, key: &str, file: &Path) -> Result<&'a mut Vec<Value>> {
    let v = obj.entry(key).or_insert_with(|| Value::Array(vec![]));
    v.as_array_mut().with_context(|| format!("`{key}` in {} is not a list", file.display()))
}
