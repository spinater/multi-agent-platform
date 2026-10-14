//! The Skills screen: each provider's context file in a workspace, the skills Claude Code and Codex can load there
//! (from the workspace, the home directory and installed plugins), whether Claude Code has them switched off, and
//! how often they were loaded, from Claude Code's own session transcripts mapped onto MAP's runs.
//!
//! Everything here is read only except `<workspace>/.claude/settings.local.json`, which [`set_enabled`] edits the
//! way Claude Code's own `/skills` menu does: a `Skill(<name>)` deny rule, plus `skillOverrides: {name: "off"}` for
//! skills that are not from a plugin.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::SystemTime;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use regex::Regex;
use serde_json::{Map, Value};

use crate::config::SkillsConfig;
use crate::hub::Hub;
use crate::model::{ContextFile, Session, Skill, SkillFile, SkillLoad, SkillSource, SkillUsage, SkillsOverview};

/// Largest SKILL.md the file endpoint returns in full.
const MAX_SKILL_BYTES: usize = 256 * 1024;
/// How deep the scanner looks for SKILL.md files below a skills folder, and for plugins below a plugins folder.
const SKILL_DEPTH: usize = 5;
const PLUGIN_DEPTH: usize = 7;
const SETTINGS_LOCAL: &str = ".claude/settings.local.json";

// ---- frontmatter

/// The fields of a SKILL.md that the screen shows. Parsing is deliberately tolerant: a missing or broken header
/// yields defaults, never an error.
#[derive(Debug, Default, PartialEq)]
pub struct Frontmatter {
    pub name: Option<String>,
    pub description: Option<String>,
    pub allowed_tools: Vec<String>,
    pub disable_model_invocation: bool,
    /// Everything after the header (the whole text when there is none).
    pub body: String,
}

pub enum Raw {
    Scalar(String),
    List(Vec<String>),
}

/// One top-level key of a frontmatter header: its value, and its lines as written (so a rewrite of the file can keep
/// the keys it does not manage).
pub struct Field {
    pub key: String,
    pub value: Raw,
    pub raw: String,
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        // `\n` reads as a space: the screens show these values on one line.
        let (mut out, mut chars) = (String::new(), s[1..s.len() - 1].chars());
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push(' '),
                Some('t') => out.push('\t'),
                Some(e @ ('"' | '\\' | '/')) => out.push(e),
                Some(e) => {
                    out.push('\\');
                    out.push(e);
                }
                None => out.push('\\'),
            }
        }
        return out;
    }
    if s.len() >= 2 && s.starts_with('\'') && s.ends_with('\'') {
        return s[1..s.len() - 1].replace("''", "'");
    }
    s.to_string()
}

/// Splits on commas and whitespace outside parentheses: `Read, Bash(git add *) Grep` → three tools.
pub fn split_tools(s: &str) -> Vec<String> {
    let (mut out, mut cur, mut depth) = (vec![], String::new(), 0i32);
    for c in s.chars() {
        match c {
            '(' => {
                depth += 1;
                cur.push(c);
            }
            ')' => {
                depth -= 1;
                cur.push(c);
            }
            ',' | ' ' | '\t' if depth <= 0 => {
                if !cur.trim().is_empty() {
                    out.push(unquote(&cur));
                }
                cur.clear();
            }
            _ => cur.push(c),
        }
    }
    if !cur.trim().is_empty() {
        out.push(unquote(&cur));
    }
    out
}

fn split_inline_list(s: &str) -> Vec<String> {
    let inner = s.trim().trim_start_matches('[').trim_end_matches(']');
    let (mut out, mut cur, mut depth) = (vec![], String::new(), 0i32);
    for c in inner.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if c == ',' && depth <= 0 {
            out.push(unquote(&cur));
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    out.push(unquote(&cur));
    out.into_iter().filter(|s| !s.is_empty()).collect()
}

/// The header's top-level fields in order, and the body after it; `None` when the text has no closed `---` header.
pub fn header_fields(text: &str) -> Option<(Vec<Field>, String)> {
    let text = text.trim_start_matches('\u{feff}');
    let lines: Vec<&str> = text.lines().collect();
    let close = if lines.first().map(|l| l.trim_end()) == Some("---") { lines.iter().skip(1).position(|l| matches!(l.trim_end(), "---" | "...")) } else { None };
    let close = close? + 1;
    let header = &lines[1..close];
    let body = lines[close + 1..].join("\n");

    let mut fields = vec![];
    let mut i = 0;
    while i < header.len() {
        let (line, start) = (header[i], i);
        i += 1;
        if line.starts_with([' ', '\t', '#']) || line.trim().is_empty() {
            continue;
        }
        let Some((key, rest)) = line.split_once(':') else { continue };
        let (key, rest) = (key.trim().to_ascii_lowercase(), rest.trim());
        // Indented continuation lines belong to this key (block scalars, block lists, wrapped plain scalars).
        let mut cont = vec![];
        while i < header.len() && (header[i].starts_with([' ', '\t']) || header[i].trim().is_empty() || (rest.is_empty() && header[i].starts_with("- "))) {
            cont.push(header[i].trim());
            i += 1;
        }
        let raw = header[start..i].join("\n").trim_end().to_string();
        let cont: Vec<&str> = cont.into_iter().filter(|l| !l.is_empty()).collect();
        let value = if rest.starts_with('[') {
            Raw::List(split_inline_list(rest))
        } else if rest.is_empty() && !cont.is_empty() && cont.iter().all(|l| l.starts_with('-')) {
            Raw::List(cont.iter().map(|l| unquote(l.trim_start_matches('-'))).filter(|s| !s.is_empty()).collect())
        } else if rest.starts_with(['|', '>']) || rest.is_empty() {
            Raw::Scalar(cont.join(" "))
        } else if cont.is_empty() {
            Raw::Scalar(unquote(rest))
        } else {
            Raw::Scalar(unquote(&format!("{rest} {}", cont.join(" "))))
        };
        fields.push(Field { key, value, raw });
    }
    Some((fields, body))
}

pub fn parse_frontmatter(text: &str) -> Frontmatter {
    let Some((list, body)) = header_fields(text) else {
        return Frontmatter { body: text.trim_start_matches('\u{feff}').to_string(), ..Default::default() };
    };
    // A key given twice: the last one wins.
    let fields: HashMap<String, Raw> = list.into_iter().map(|f| (f.key, f.value)).collect();
    let scalar = |k: &str| match fields.get(k) {
        Some(Raw::Scalar(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Raw::List(items)) if !items.is_empty() => Some(items.join(", ")),
        _ => None,
    };
    let allowed_tools = match fields.get("allowed-tools").or_else(|| fields.get("allowed_tools")) {
        Some(Raw::Scalar(s)) => split_tools(s),
        Some(Raw::List(items)) => items.iter().flat_map(|i| split_tools(i)).collect(),
        None => vec![],
    };
    Frontmatter {
        name: scalar("name"),
        description: scalar("description"),
        allowed_tools,
        disable_model_invocation: scalar("disable-model-invocation").is_some_and(|v| v.eq_ignore_ascii_case("true")),
        body,
    }
}

/// The first paragraph of prose, for skills without a `description`.
fn first_paragraph(body: &str) -> String {
    let mut out = vec![];
    for line in body.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') || line.starts_with("```") || line.starts_with("---") {
            if !out.is_empty() {
                break;
            }
            continue;
        }
        out.push(line);
    }
    clip(&out.join(" "), 300)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    format!("{}…", s.chars().take(max - 1).collect::<String>().trim_end())
}

// ---- context files

static IMPORT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|\s)@([A-Za-z0-9_~./-]*[A-Za-z0-9_])").unwrap());

pub fn context_file(workspace: &Path, provider: &str, file: &str) -> ContextFile {
    let path = workspace.join(file);
    let text = std::fs::read_to_string(&path).ok();
    let mut summary = vec![];
    let mut imports = vec![];
    if let Some(text) = &text {
        let mut fence = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with("```") {
                fence = !fence;
                continue;
            }
            if fence {
                continue;
            }
            for cap in IMPORT.captures_iter(line) {
                let target = cap[1].trim_start_matches("./").to_string();
                // `@` before a word with no dot or slash is a mention, not a file.
                if (target.contains('.') || target.contains('/')) && !imports.contains(&target) {
                    imports.push(target);
                }
            }
            let is_import = line.starts_with('@') && !line.contains(' ');
            if summary.len() < 3 && !line.is_empty() && !line.starts_with('#') && !line.starts_with("---") && !is_import {
                summary.push(line.trim_start_matches(['-', '*', ' ']).to_string());
            }
        }
    }
    ContextFile {
        provider: provider.to_string(),
        file: file.to_string(),
        path: path.display().to_string(),
        exists: text.is_some(),
        lines: text.as_deref().map_or(0, |t| t.lines().count() as u32),
        summary: clip(&summary.join(" "), 240),
        imports,
        imported_by: vec![],
    }
}

fn contexts(cfg: &SkillsConfig, workspace: &Path) -> Vec<ContextFile> {
    let mut out: Vec<ContextFile> = cfg.context_files.iter().map(|(provider, file)| context_file(workspace, provider, file)).collect();
    let imports: Vec<(String, Vec<String>)> = out.iter().map(|c| (c.file.clone(), c.imports.clone())).collect();
    for c in &mut out {
        c.imported_by = imports.iter().filter(|(f, imps)| *f != c.file && imps.iter().any(|i| i == &c.file)).map(|(f, _)| f.clone()).collect();
    }
    // Files that exist first, in config order otherwise.
    out.sort_by_key(|c| !c.exists);
    out
}

// ---- Claude Code settings

struct SettingsFile {
    label: String,
    local: bool,
    json: Value,
}

fn read_json(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok())
}

/// User, project and local settings, the files whose rules apply to a Claude session in `workspace`.
fn settings_files(cfg: &SkillsConfig, workspace: &Path) -> Vec<SettingsFile> {
    let mut out = vec![];
    if let Some(json) = read_json(&cfg.claude_settings) {
        out.push(SettingsFile { label: display_path(&cfg.claude_settings, workspace), local: false, json });
    }
    if let Some(json) = read_json(&workspace.join(".claude/settings.json")) {
        out.push(SettingsFile { label: ".claude/settings.json".into(), local: false, json });
    }
    if let Some(json) = read_json(&workspace.join(SETTINGS_LOCAL)) {
        out.push(SettingsFile { label: SETTINGS_LOCAL.into(), local: true, json });
    }
    out
}

fn deny_rules(json: &Value) -> impl Iterator<Item = &str> {
    json.pointer("/permissions/deny").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
}

/// Whether a permission rule blocks the skill. `Skill` alone blocks every skill; `Skill(x)` and `Skill(x *)`
/// match the invocation name (and the bare name, except for plugin skills, which are always namespaced);
/// `Skill(skill:x)` is the parameter form.
pub fn rule_denies(rule: &str, invocation: &str, name: &str, plugin: bool) -> bool {
    let rule = rule.trim();
    if rule == "Skill" {
        return true;
    }
    let Some(inner) = rule.strip_prefix("Skill(").and_then(|r| r.strip_suffix(')')) else { return false };
    let inner = inner.trim();
    let base = inner.strip_suffix(" *").or_else(|| inner.strip_suffix(":*")).unwrap_or(inner).trim();
    let base = base.strip_prefix("skill:").unwrap_or(base);
    base == invocation || (!plugin && base == name)
}

fn override_off(json: &Value, name: &str) -> bool {
    json.get("skillOverrides").and_then(|o| o.get(name)).and_then(Value::as_str) == Some("off")
}

/// `enabledPlugins` merged across the settings files: `"name@marketplace": bool`, later files win.
fn enabled_plugins(files: &[SettingsFile]) -> HashMap<String, bool> {
    let mut out = HashMap::new();
    for f in files {
        if let Some(map) = f.json.get("enabledPlugins").and_then(Value::as_object) {
            for (k, v) in map {
                if let Some(b) = v.as_bool() {
                    out.insert(k.clone(), b);
                }
            }
        }
    }
    out
}

/// Disables or re-enables a Claude skill for one workspace by editing `.claude/settings.local.json`, keeping every
/// other key. Disabling adds `Skill(<invocation>)` to `permissions.deny` and, when `override_name` is given (skills
/// that are not from a plugin), sets `skillOverrides.<name> = "off"`, so Claude neither lists nor runs it.
/// Enabling removes the rules that match this skill.
pub fn set_enabled(local: &Path, invocation: &str, name: &str, plugin: bool, enabled: bool) -> Result<()> {
    let mut root = match std::fs::read_to_string(local) {
        Ok(text) if !text.trim().is_empty() => serde_json::from_str::<Value>(&text).with_context(|| format!("{} is not valid JSON; fix it by hand first", local.display()))?,
        Ok(_) => Value::Object(Map::new()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Object(Map::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", local.display())),
    };
    let Some(obj) = root.as_object_mut() else { bail!("{} is not a JSON object", local.display()) };
    let perms = obj.entry("permissions").or_insert_with(|| Value::Object(Map::new()));
    let Some(perms) = perms.as_object_mut() else { bail!("`permissions` in {} is not an object", local.display()) };
    let deny = perms.entry("deny").or_insert_with(|| Value::Array(vec![]));
    let Some(deny) = deny.as_array_mut() else { bail!("`permissions.deny` in {} is not a list", local.display()) };
    let rule = format!("Skill({invocation})");
    if enabled {
        if deny.iter().any(|r| r.as_str().map(str::trim) == Some("Skill")) {
            bail!("{} denies every skill (`Skill`); remove that rule by hand", local.display());
        }
        deny.retain(|r| !r.as_str().is_some_and(|r| rule_denies(r, invocation, name, plugin)));
    } else if !deny.iter().any(|r| r.as_str() == Some(rule.as_str())) {
        deny.push(Value::String(rule));
    }
    if !plugin {
        if enabled {
            if let Some(overrides) = obj.get_mut("skillOverrides").and_then(Value::as_object_mut) {
                if overrides.get(name).and_then(Value::as_str) == Some("off") {
                    overrides.remove(name);
                }
                if overrides.is_empty() {
                    obj.remove("skillOverrides");
                }
            }
        } else {
            let overrides = obj.entry("skillOverrides").or_insert_with(|| Value::Object(Map::new()));
            let Some(overrides) = overrides.as_object_mut() else { bail!("`skillOverrides` in {} is not an object", local.display()) };
            overrides.insert(name.to_string(), Value::String("off".into()));
        }
    }
    let dir = local.parent().context("settings path has no folder")?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(".settings.local.json.map-tmp");
    std::fs::write(&tmp, format!("{}\n", serde_json::to_string_pretty(&root)?))?;
    std::fs::rename(&tmp, local)?;
    Ok(())
}

/// Keeps the local settings file out of commits (`post_done` hooks often `git add -A`) without touching tracked
/// files: an entry in the workspace's own `.git/info/exclude`.
fn exclude_from_git(workspace: &Path) {
    let git = workspace.join(".git");
    if !git.is_dir() {
        return;
    }
    let exclude = git.join("info/exclude");
    let text = std::fs::read_to_string(&exclude).unwrap_or_default();
    let line = format!("/{SETTINGS_LOCAL}");
    if text.lines().any(|l| l.trim() == line || l.trim() == SETTINGS_LOCAL) {
        return;
    }
    let _ = std::fs::create_dir_all(git.join("info"));
    let sep = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
    let _ = std::fs::write(&exclude, format!("{text}{sep}# written by MAP's Skills screen\n{line}\n"));
}

// ---- discovery

fn is_dir(p: &Path) -> bool {
    std::fs::metadata(p).is_ok_and(|m| m.is_dir())
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| is_dir(p) && !matches!(p.file_name().and_then(|n| n.to_str()), Some("node_modules" | ".git")))
        .collect();
    out.sort();
    out
}

/// Every `SKILL.md` below `dir`; a folder with a SKILL.md is a skill and is not searched further.
fn skill_files(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let file = dir.join("SKILL.md");
    if file.is_file() {
        out.push(file);
        return;
    }
    if depth == 0 {
        return;
    }
    for sub in subdirs(dir) {
        skill_files(&sub, depth - 1, out);
    }
}

/// Folders with `.claude-plugin/plugin.json` below `dir`.
fn plugin_roots(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if dir.join(".claude-plugin/plugin.json").is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    if depth == 0 {
        return;
    }
    for sub in subdirs(dir) {
        if !sub.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with('.')) {
            plugin_roots(&sub, depth - 1, out);
        }
    }
}

/// The component after `marker` in `path` below `base`: the marketplace of `marketplaces/<m>/…` or `cache/<m>/…`.
fn component_after(path: &Path, base: &Path, marker: &str) -> Option<String> {
    let rel = path.strip_prefix(base).ok()?;
    let parts: Vec<&str> = rel.components().filter_map(|c| c.as_os_str().to_str()).collect();
    parts.iter().position(|p| *p == marker).and_then(|i| parts.get(i + 1)).map(|s| s.to_string())
}

fn mtime(p: &Path) -> SystemTime {
    std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(SystemTime::UNIX_EPOCH)
}

pub fn display_path(p: &Path, workspace: &Path) -> String {
    if let Ok(rel) = p.strip_prefix(workspace) {
        return rel.display().to_string();
    }
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rel) = p.strip_prefix(&home)
    {
        return format!("~/{}", rel.display());
    }
    p.display().to_string()
}


struct Candidate {
    file: PathBuf,
    source: SkillSource,
    home: &'static str,
    plugin: Option<String>,
    /// `~/.claude/skills/synced/…`: synced from claude.ai, invoked as `anthropic-skills:<name>`.
    synced: bool,
    /// Turned off in `enabledPlugins`.
    plugin_off: Option<String>,
    folder: Option<String>,
}

fn candidates(cfg: &SkillsConfig, workspace: &Path, settings: &[SettingsFile]) -> Vec<Candidate> {
    let mut out = vec![];
    let push_dir = |dir: &Path, source: SkillSource, home: &'static str, folder: Option<String>, out: &mut Vec<Candidate>| {
        let mut files = vec![];
        skill_files(dir, SKILL_DEPTH, &mut files);
        for file in files {
            let synced = source == SkillSource::User && file.strip_prefix(dir).ok().and_then(|r| r.components().next()).is_some_and(|c| c.as_os_str() == "synced");
            out.push(Candidate { file, source, home, plugin: None, synced, plugin_off: None, folder: folder.clone() });
        }
    };
    for rel in &cfg.claude_project_dirs {
        push_dir(&workspace.join(rel), SkillSource::Project, "claude", Some(rel.display().to_string()), &mut out);
    }
    for rel in &cfg.codex_project_dirs {
        push_dir(&workspace.join(rel), SkillSource::Project, "codex", Some(rel.display().to_string()), &mut out);
    }
    for dir in &cfg.claude_user_dirs {
        push_dir(dir, SkillSource::User, "claude", None, &mut out);
    }
    for dir in &cfg.codex_user_dirs {
        push_dir(dir, SkillSource::User, "codex", None, &mut out);
    }

    let enabled = enabled_plugins(settings);
    for base in &cfg.claude_plugin_dirs {
        let mut roots = vec![];
        plugin_roots(base, PLUGIN_DEPTH, &mut roots);
        // One copy per plugin: an installed copy before a marketplace catalog's, the newest version first.
        let mut roots: Vec<(PathBuf, String, Option<String>, bool)> = roots
            .into_iter()
            .map(|root| {
                let manifest = read_json(&root.join(".claude-plugin/plugin.json"));
                let name = manifest.as_ref().and_then(|m| m.get("name")).and_then(Value::as_str).map(String::from).unwrap_or_else(|| root.file_name().unwrap_or_default().to_string_lossy().into_owned());
                let catalog = component_after(&root, base, "marketplaces");
                let market = catalog.clone().or_else(|| component_after(&root, base, "cache"));
                (root, name, market, catalog.is_some())
            })
            .collect();
        roots.sort_by_key(|(root, _, _, catalog)| (*catalog, std::cmp::Reverse(mtime(&root.join(".claude-plugin/plugin.json")))));
        let mut seen = BTreeSet::new();
        for (root, name, market, catalog) in roots {
            let keyed: Vec<bool> = enabled.iter().filter(|(k, _)| k.split_once('@').is_some_and(|(n, m)| n == name && market.as_deref().is_none_or(|mk| mk == m))).map(|(_, v)| *v).collect();
            let on = keyed.iter().any(|v| *v);
            // A marketplace folder holds every plugin it offers; only the enabled ones are installed.
            if (catalog && !on) || !seen.insert(name.clone()) {
                continue;
            }
            let plugin_off = (!keyed.is_empty() && !on).then(|| format!("Plugin `{name}` is turned off in enabledPlugins"));
            let mut files = vec![];
            skill_files(&root.join("skills"), SKILL_DEPTH, &mut files);
            for file in files {
                out.push(Candidate { file, source: SkillSource::Plugin, home: "claude", plugin: Some(name.clone()), synced: false, plugin_off: plugin_off.clone(), folder: None });
            }
        }
    }
    out
}

static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^A-Za-z0-9_-]").unwrap());

/// Whether a context file points the agent at this skill: its folder, `skills/<name>` or `/<name>`.
fn mentions(text: &str, name: &str, folder: Option<&str>) -> bool {
    if folder.is_some_and(|f| text.contains(f.trim_end_matches('/'))) || text.contains(&format!("skills/{name}")) {
        return true;
    }
    let needle = format!("/{name}");
    text.match_indices(&needle).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + needle.len()..].chars().next();
        before.is_none_or(|c| c.is_whitespace() || "`(\"'".contains(c)) && after.is_none_or(|c| WORD.is_match(&c.to_string()))
    })
}

fn scan(cfg: &SkillsConfig, workspace: &Path) -> (Vec<ContextFile>, Vec<Skill>) {
    let contexts = contexts(cfg, workspace);
    let settings = settings_files(cfg, workspace);
    let context_texts: Vec<(String, String, String)> = contexts
        .iter()
        .filter(|c| c.exists)
        .filter_map(|c| std::fs::read_to_string(&c.path).ok().map(|t| (c.provider.clone(), c.file.clone(), t)))
        .collect();

    let mut found = vec![];
    for c in candidates(cfg, workspace, &settings) {
        let text = std::fs::read_to_string(&c.file).unwrap_or_default();
        let fm = parse_frontmatter(&text);
        let dir_name = c.file.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let name = fm.name.clone().unwrap_or(dir_name);
        let plugin = c.source == SkillSource::Plugin;
        let invocation = match (&c.plugin, c.synced) {
            (Some(p), _) => format!("{p}:{name}"),
            (None, true) => format!("anthropic-skills:{name}"),
            _ => name.clone(),
        };

        let mut loads_in = vec![SkillLoad { provider: c.home.into(), how: "native".into() }];
        for (provider, file, ctx) in &context_texts {
            if provider != c.home && mentions(ctx, &name, c.folder.as_deref()) {
                loads_in.push(SkillLoad { provider: provider.clone(), how: format!("via {file}") });
            }
        }

        let (enabled, can_toggle, toggle_note) = if c.home != "claude" {
            (true, false, Some("Codex loads it from its own skills folder; MAP has no per-project switch for Codex skills.".to_string()))
        } else if let Some(note) = c.plugin_off.clone() {
            (false, false, Some(note))
        } else {
            let off_in: Vec<&SettingsFile> = settings
                .iter()
                .filter(|f| deny_rules(&f.json).any(|r| rule_denies(r, &invocation, &name, plugin)) || (!plugin && !c.synced && override_off(&f.json, &name)))
                .collect();
            let fixed: Vec<&str> = off_in.iter().filter(|f| !f.local).map(|f| f.label.as_str()).collect();
            if !fixed.is_empty() {
                (false, false, Some(format!("Turned off in {}; MAP only edits {SETTINGS_LOCAL}.", fixed.join(" and "))))
            } else if !off_in.is_empty() {
                (false, true, Some(format!("Off for Claude in this workspace ({SETTINGS_LOCAL}).")))
            } else {
                (true, true, None)
            }
        };

        found.push(Skill {
            id: c.file.display().to_string(),
            name,
            invocation,
            source: c.source,
            plugin: c.plugin,
            home: c.home.into(),
            description: fm.description.clone().map(|d| clip(&d, 600)).unwrap_or_else(|| first_paragraph(&fm.body)),
            allowed_tools: fm.allowed_tools,
            path: display_path(&c.file, workspace),
            loads_in,
            enabled,
            can_toggle,
            toggle_note,
            model_invocable: !fm.disable_model_invocation,
            usage_known: c.home == "claude",
            usage: SkillUsage::default(),
        });
    }
    (contexts, found)
}

// ---- usage from Claude Code transcripts

/// One skill load in a transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct Load {
    pub skill: String,
    pub at: DateTime<Utc>,
    /// Claude's session id.
    pub transcript: String,
    /// The session's `--name`, which MAP sets to the tmux session name.
    pub agent: Option<String>,
}

/// Claude Code keeps each project's transcripts in a folder named after the path with every character that is not
/// a letter or digit replaced by `-` (`/home/a/my.repo` → `-home-a-my-repo`).
pub fn encode_project_dir(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn transcript_dir(root: &Path, workspace: &Path) -> Option<PathBuf> {
    let mut names = vec![encode_project_dir(workspace)];
    if let Ok(real) = workspace.canonicalize() {
        names.push(encode_project_dir(&real));
    }
    names.into_iter().map(|n| root.join(n)).find(|d| d.is_dir())
}

static COMMAND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*<command-name>/([^<\s]+)</command-name>").unwrap());

/// Skill loads in one transcript: `Skill` tool calls, and `/name` typed by a person.
pub fn parse_transcript(reader: impl BufRead) -> Vec<Load> {
    let mut loads = vec![];
    let mut agent = None;
    for line in reader.lines().map_while(Result::ok) {
        if !(line.contains("\"Skill\"") || line.contains("<command-name>/") || line.contains("\"agent-name\"")) {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "agent-name" {
            agent = entry.get("agentName").and_then(Value::as_str).map(String::from);
            continue;
        }
        let Some(at) = entry.get("timestamp").and_then(Value::as_str).and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc)) else { continue };
        let transcript = entry.get("sessionId").and_then(Value::as_str).unwrap_or("").to_string();
        let content = entry.pointer("/message/content");
        let mut push = |skill: &str| loads.push(Load { skill: skill.trim_start_matches('/').to_string(), at, transcript: transcript.clone(), agent: None });
        match (kind, content) {
            ("assistant", Some(Value::Array(items))) => {
                for item in items {
                    if item.get("type").and_then(Value::as_str) == Some("tool_use")
                        && item.get("name").and_then(Value::as_str) == Some("Skill")
                        && let Some(skill) = item.pointer("/input/skill").and_then(Value::as_str)
                    {
                        push(skill);
                    }
                }
            }
            ("user", Some(Value::String(text))) if entry.get("isMeta").and_then(Value::as_bool) != Some(true) => {
                if let Some(cap) = COMMAND.captures(text) {
                    push(&cap[1]);
                }
            }
            _ => {}
        }
    }
    for l in &mut loads {
        l.agent = agent.clone();
    }
    loads
}

/// All loads in a transcript folder (including subagents' transcripts), and the number of files read.
fn read_loads(dir: &Path) -> (Vec<Load>, u32) {
    fn walk(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.extension().is_some_and(|e| e == "jsonl") && p.is_file() {
                files.push(p);
            } else if depth > 0 && p.is_dir() {
                walk(&p, depth - 1, files);
            }
        }
    }
    let mut files = vec![];
    walk(dir, 3, &mut files);
    let mut loads = vec![];
    for f in &files {
        if let Ok(file) = std::fs::File::open(f) {
            loads.extend(parse_transcript(BufReader::new(file)));
        }
    }
    // A subagent's transcript has no name of its own: it inherits its parent session's.
    let names: HashMap<String, String> = loads.iter().filter_map(|l| l.agent.clone().map(|a| (l.transcript.clone(), a))).collect();
    for l in &mut loads {
        if l.agent.is_none() {
            l.agent = names.get(&l.transcript).cloned();
        }
    }
    (loads, files.len() as u32)
}

/// A MAP run in this workspace: the tmux session, the task it ran, and when it was alive.
#[derive(Debug, Clone)]
pub struct Run {
    pub session: String,
    pub task_id: Option<i64>,
    pub claude: bool,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

/// Runs from the store's sessions of this project, plus run folders (`<data_dir>/runs/<session>/`) the store no
/// longer lists: their `launch.sh` names the project (`MAP_PROJECT`) and the agent binary, worker names carry
/// the task id (`<prefix>task-<id>-<attempt>`), and file times bound the run.
pub fn runs(sessions: &[Session], runs_dir: &Path, prefix: &str, project: Option<&str>, claude_binary: &str) -> Vec<Run> {
    let mut out: Vec<Run> = sessions
        .iter()
        .filter(|s| s.project.as_deref() == project)
        .map(|s| Run { session: s.name.clone(), task_id: s.task_id, claude: s.provider == "claude", start: s.created_at, end: s.last_activity_at })
        .collect();
    let task_re = Regex::new(&format!(r"^{}task-(\d+)-\d+$", regex::escape(prefix))).unwrap();
    let binary = Path::new(claude_binary).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    for dir in subdirs(runs_dir) {
        let name = dir.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if out.iter().any(|r| r.session == name) {
            continue;
        }
        let Ok(script) = std::fs::read_to_string(dir.join("launch.sh")) else { continue };
        let run_project = script.lines().find_map(|l| l.strip_prefix("export MAP_PROJECT=")).map(|v| v.trim().trim_matches('\'').to_string());
        if run_project.as_deref() != project {
            continue;
        }
        let exec = script.lines().find_map(|l| l.strip_prefix("exec ")).unwrap_or("");
        let first = exec.split_whitespace().next().unwrap_or("").trim_matches('\'');
        let claude = !binary.is_empty() && Path::new(first).file_name().is_some_and(|n| n.to_string_lossy() == binary);
        let times: Vec<SystemTime> = std::fs::read_dir(&dir).into_iter().flatten().flatten().filter_map(|e| e.metadata().ok()?.modified().ok()).collect();
        let start: DateTime<Utc> = mtime(&dir.join("launch.sh")).into();
        let end: DateTime<Utc> = times.into_iter().max().map_or(start, Into::into);
        let task_id = task_re.captures(&name).and_then(|c| c[1].parse().ok());
        out.push(Run { session: name, task_id, claude, start, end });
    }
    out
}

/// The run a load happened in: the one named like the Claude session (MAP starts Claude with `--name <session>`),
/// or for an unnamed session (e.g. a custom launch template), the only Claude run alive at the time.
fn run_of<'a>(load: &Load, runs: &'a [Run]) -> Option<&'a Run> {
    if let Some(agent) = &load.agent {
        return runs.iter().find(|r| &r.session == agent);
    }
    let mut alive = runs.iter().filter(|r| r.claude && load.at >= r.start - Duration::minutes(1) && load.at <= r.end + Duration::minutes(2));
    match (alive.next(), alive.next()) {
        (Some(r), None) => Some(r),
        _ => None,
    }
}

/// Adds the loads to the skills they name; returns how many named no skill here.
fn count_usage(skills: &mut [Skill], loads: &[Load], runs: &[Run], now: DateTime<Utc>) -> u32 {
    let mut unmatched = 0;
    let mut sessions: Vec<BTreeSet<String>> = vec![BTreeSet::new(); skills.len()];
    for load in loads {
        // Exact invocation first, then a bare name (`/pdf` for `anthropic-skills:pdf`), preferring non-plugin skills.
        let idx = skills
            .iter()
            .position(|s| s.usage_known && s.invocation == load.skill)
            .or_else(|| skills.iter().position(|s| s.usage_known && s.name == load.skill && s.source != SkillSource::Plugin))
            .or_else(|| skills.iter().position(|s| s.usage_known && s.name == load.skill));
        let Some(i) = idx else {
            unmatched += 1;
            continue;
        };
        let u = &mut skills[i].usage;
        u.loads += 1;
        if load.at >= now - Duration::days(7) {
            u.loads_week += 1;
        }
        if u.last_used.is_none_or(|t| load.at > t) {
            u.last_used = Some(load.at);
        }
        sessions[i].insert(load.agent.clone().unwrap_or_else(|| load.transcript.clone()));
        if let Some(task) = run_of(load, runs).and_then(|r| r.task_id)
            && !u.task_ids.contains(&task)
        {
            u.task_ids.push(task);
        }
    }
    for (s, set) in skills.iter_mut().zip(sessions) {
        s.usage.sessions = set.len() as u32;
        s.usage.task_ids.sort_unstable();
    }
    unmatched
}

// ---- entry points for the API

/// Context files, skills and usage for a project's workspace (`None`: `workspace.dir`).
pub async fn overview(hub: &Hub, project: Option<&str>) -> Result<SkillsOverview> {
    let workspace = hub.work_dir(project).await;
    let sessions = hub.store.sessions().await?;
    let cfg = hub.config.skills.clone();
    let runs_dir = hub.config.server.data_dir.join("runs");
    let prefix = hub.config.workspace.session_prefix.clone();
    let claude_binary = hub.config.providers.get("claude").map(|p| p.binary.clone()).unwrap_or_else(|| "claude".into());
    let slug = project.map(String::from);
    tokio::task::spawn_blocking(move || {
        let (contexts, found) = scan(&cfg, &workspace);
        let mut skills = found;
        let dir = transcript_dir(&cfg.claude_transcripts, &workspace);
        let (loads, files) = dir.as_deref().map(read_loads).unwrap_or_default();
        let runs = runs(&sessions, &runs_dir, &prefix, slug.as_deref(), &claude_binary);
        let now = Utc::now();
        let unmatched = count_usage(&mut skills, &loads, &runs, now);
        SkillsOverview {
            project: slug,
            workspace: workspace.display().to_string(),
            contexts,
            skills,
            transcripts: dir.map(|d| d.display().to_string()),
            transcript_files: files,
            unmatched_loads: unmatched,
            scanned_at: now,
        }
    })
    .await
    .context("skills scan panicked")
}

fn find(cfg: &SkillsConfig, workspace: &Path, id: &str) -> Result<Skill> {
    let (_, found) = scan(cfg, workspace);
    // Only files the scanner itself found can be read or switched: the id is never used as a path on its own.
    found.into_iter().find(|s| s.id == id).with_context(|| format!("no skill `{id}` in this workspace"))
}

/// The SKILL.md of a skill the scanner found for this workspace.
pub fn read_file(cfg: &SkillsConfig, workspace: &Path, id: &str) -> Result<SkillFile> {
    let skill = find(cfg, workspace, id)?;
    let bytes = std::fs::read(&skill.id).with_context(|| format!("reading {}", skill.path))?;
    let truncated = bytes.len() > MAX_SKILL_BYTES;
    let content = String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_SKILL_BYTES)]).into_owned();
    Ok(SkillFile { id: skill.id, path: skill.path, content, truncated })
}

/// Switches a Claude skill on or off for the workspace; returns the skill's invocation name.
pub fn toggle(cfg: &SkillsConfig, workspace: &Path, id: &str, enabled: bool) -> Result<String> {
    if !workspace.is_dir() {
        bail!("workspace {} does not exist", workspace.display());
    }
    let skill = find(cfg, workspace, id)?;
    if !skill.can_toggle {
        bail!("{}", skill.toggle_note.unwrap_or_else(|| "this skill cannot be switched from MAP".into()));
    }
    set_enabled(&workspace.join(SETTINGS_LOCAL), &skill.invocation, &skill.name, skill.source == SkillSource::Plugin, enabled)?;
    exclude_from_git(workspace);
    Ok(skill.invocation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn frontmatter_plain_quoted_and_lists() {
        let fm = parse_frontmatter("---\nname: verify\ndescription: \"Run checks: all of them.\"\nallowed-tools: Read, Bash(bun *), Grep\n---\n# Verify\nbody");
        assert_eq!(fm.name.as_deref(), Some("verify"));
        assert_eq!(fm.description.as_deref(), Some("Run checks: all of them."));
        assert_eq!(fm.allowed_tools, vec!["Read", "Bash(bun *)", "Grep"]);
        assert_eq!(fm.body, "# Verify\nbody");

        let fm = parse_frontmatter("---\nname: 'it''s'\ndescription: >\n  Folded text\n  over lines.\nallowed-tools:\n  - Read\n  - \"Bash(git *)\"\ndisable-model-invocation: true\n---\n");
        assert_eq!(fm.name.as_deref(), Some("it's"));
        assert_eq!(fm.description.as_deref(), Some("Folded text over lines."));
        assert_eq!(fm.allowed_tools, vec!["Read", "Bash(git *)"]);
        assert!(fm.disable_model_invocation);

        let fm = parse_frontmatter("---\nallowed-tools: [Read, \"Write\", Bash(a, b)]\nmetadata:\n  nested: x\n---\nx");
        assert_eq!(fm.allowed_tools, vec!["Read", "Write", "Bash(a, b)"]);
        assert_eq!(fm.name, None);
    }

    #[test]
    fn frontmatter_tolerates_missing_or_broken_headers() {
        let fm = parse_frontmatter("# Title\n\nFirst paragraph\nstill first.\n\nSecond.");
        assert_eq!(fm.name, None);
        assert_eq!(first_paragraph(&fm.body), "First paragraph still first.");
        // No closing line: the whole file is body.
        let fm = parse_frontmatter("---\nname: x\nno end");
        assert_eq!(fm.name, None);
        assert!(fm.body.starts_with("---"));
        let fm = parse_frontmatter("\u{feff}---\nname: bom\n---\n");
        assert_eq!(fm.name.as_deref(), Some("bom"));
    }

    #[test]
    fn deny_rules_match_like_claude_code() {
        assert!(rule_denies("Skill", "x", "x", false));
        assert!(rule_denies("Skill(deploy)", "deploy", "deploy", false));
        assert!(rule_denies("Skill(deploy *)", "deploy", "deploy", false));
        assert!(rule_denies("Skill(skill:deploy)", "deploy", "deploy", false));
        assert!(rule_denies("Skill(eng:debug)", "eng:debug", "debug", true));
        assert!(!rule_denies("Skill(debug)", "eng:debug", "debug", true));
        assert!(!rule_denies("Bash(deploy)", "deploy", "deploy", false));
        assert!(!rule_denies("Skill(deploy-all)", "deploy", "deploy", false));
    }

    #[test]
    fn settings_merge_keeps_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join(".claude/settings.local.json");
        write(&local, r#"{"permissions":{"allow":["Bash(ls)"],"deny":["Read(.env)"]},"model":"opus","skillOverrides":{"other":"name-only"}}"#);

        set_enabled(&local, "deploy", "deploy", false, false).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&local).unwrap()).unwrap();
        assert_eq!(v["model"], "opus");
        assert_eq!(v["permissions"]["allow"], serde_json::json!(["Bash(ls)"]));
        assert_eq!(v["permissions"]["deny"], serde_json::json!(["Read(.env)", "Skill(deploy)"]));
        assert_eq!(v["skillOverrides"], serde_json::json!({"other": "name-only", "deploy": "off"}));
        // Idempotent.
        set_enabled(&local, "deploy", "deploy", false, false).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&local).unwrap()).unwrap();
        assert_eq!(v["permissions"]["deny"].as_array().unwrap().len(), 2);

        set_enabled(&local, "deploy", "deploy", false, true).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&local).unwrap()).unwrap();
        assert_eq!(v["permissions"]["deny"], serde_json::json!(["Read(.env)"]));
        assert_eq!(v["skillOverrides"], serde_json::json!({"other": "name-only"}));

        // Plugin skills: a namespaced deny rule only, no skillOverrides (Claude Code ignores it for plugins).
        let fresh = dir.path().join("b/.claude/settings.local.json");
        set_enabled(&fresh, "eng:debug", "debug", true, false).unwrap();
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&fresh).unwrap()).unwrap();
        assert_eq!(v, serde_json::json!({"permissions": {"deny": ["Skill(eng:debug)"]}}));

        write(&local, "{not json");
        assert!(set_enabled(&local, "deploy", "deploy", false, false).is_err());
        assert_eq!(std::fs::read_to_string(&local).unwrap(), "{not json");
    }

    fn test_config(home: &Path) -> SkillsConfig {
        SkillsConfig {
            claude_user_dirs: vec![home.join(".claude/skills")],
            claude_plugin_dirs: vec![home.join(".claude/plugins")],
            codex_user_dirs: vec![home.join(".codex/skills")],
            claude_settings: home.join(".claude/settings.json"),
            claude_transcripts: home.join(".claude/projects"),
            ..SkillsConfig::default()
        }
    }

    #[test]
    fn scanner_finds_project_user_plugin_and_codex_skills() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let (h, w) = (home.path(), ws.path());
        write(&w.join("CLAUDE.md"), "@AGENTS.md\n");
        write(&w.join("AGENTS.md"), "# Rules\n\nAll writes go through Hub.\nSkills live in .claude/skills.\n");
        write(&w.join(".claude/skills/verify/SKILL.md"), "---\nname: verify\ndescription: Run the checks.\nallowed-tools: Bash(cargo *)\n---\n");
        write(&w.join(".claude/skills/nofm/SKILL.md"), "Just prose here.\n");
        write(&h.join(".claude/skills/commit-message/SKILL.md"), "---\nname: commit-message\ndescription: Commits.\n---\n");
        write(&h.join(".claude/skills/synced/abc/pdf/SKILL.md"), "---\nname: pdf\ndescription: PDFs.\n---\n");
        // An installed (synced) plugin, a marketplace catalog with one enabled and one not.
        write(&h.join(".claude/plugins/synced/x/engineering/.claude-plugin/plugin.json"), r#"{"name":"engineering"}"#);
        write(&h.join(".claude/plugins/synced/x/engineering/skills/debug/SKILL.md"), "---\nname: debug\ndescription: Debug.\n---\n");
        write(&h.join(".claude/plugins/marketplaces/official/plugins/docs/.claude-plugin/plugin.json"), r#"{"name":"docs"}"#);
        write(&h.join(".claude/plugins/marketplaces/official/plugins/docs/skills/writer/SKILL.md"), "---\nname: writer\ndescription: Docs.\n---\n");
        write(&h.join(".claude/plugins/marketplaces/official/plugins/unused/.claude-plugin/plugin.json"), r#"{"name":"unused"}"#);
        write(&h.join(".claude/plugins/marketplaces/official/plugins/unused/skills/nope/SKILL.md"), "---\nname: nope\n---\n");
        write(&h.join(".codex/skills/.system/imagegen/SKILL.md"), "---\nname: \"imagegen\"\ndescription: \"Images.\"\n---\n");
        write(&h.join(".claude/settings.json"), r#"{"enabledPlugins":{"docs@official":true},"permissions":{"deny":["Skill(commit-message)"]}}"#);
        write(&w.join(".claude/settings.local.json"), r#"{"skillOverrides":{"verify":"off"}}"#);

        let (contexts, found) = scan(&test_config(h), w);
        let claude_md = contexts.iter().find(|c| c.file == "CLAUDE.md").unwrap();
        assert_eq!(claude_md.imports, vec!["AGENTS.md"]);
        assert_eq!(claude_md.lines, 1);
        let agents = contexts.iter().find(|c| c.file == "AGENTS.md").unwrap();
        assert_eq!(agents.imported_by, vec!["CLAUDE.md"]);
        assert_eq!(agents.summary, "All writes go through Hub. Skills live in .claude/skills.");
        assert!(!contexts.iter().find(|c| c.file == "GEMINI.md").unwrap().exists);

        let by = |inv: &str| found.iter().find(|s| s.invocation == inv).unwrap_or_else(|| panic!("{inv} missing"));
        let names: BTreeSet<&str> = found.iter().map(|s| s.invocation.as_str()).collect();
        assert_eq!(names, ["verify", "nofm", "commit-message", "anthropic-skills:pdf", "engineering:debug", "docs:writer", "imagegen"].into_iter().collect());

        let verify = by("verify");
        assert_eq!((verify.source, verify.path.as_str()), (SkillSource::Project, ".claude/skills/verify/SKILL.md"));
        assert_eq!(verify.allowed_tools, vec!["Bash(cargo *)"]);
        assert!(!verify.enabled && verify.can_toggle);
        // AGENTS.md names the project skills folder, so codex may read them.
        assert!(verify.loads_in.iter().any(|l| l.provider == "codex" && l.how == "via AGENTS.md"));
        assert_eq!(by("nofm").description, "Just prose here.");
        let commit = by("commit-message");
        assert!(!commit.enabled && !commit.can_toggle && commit.toggle_note.as_deref().unwrap().contains("settings.json"));
        assert!(!commit.loads_in.iter().any(|l| l.provider == "codex"));
        assert_eq!(by("engineering:debug").plugin.as_deref(), Some("engineering"));
        assert!(by("docs:writer").enabled);
        let img = by("imagegen");
        assert_eq!((img.home.as_str(), img.can_toggle, img.usage_known), ("codex", false, false));
        assert_eq!(img.loads_in[0].provider, "codex");
    }

    #[test]
    fn file_reads_and_toggles_only_found_skills() {
        let home = tempfile::tempdir().unwrap();
        let ws = tempfile::tempdir().unwrap();
        let (cfg, w) = (test_config(home.path()), ws.path());
        std::fs::create_dir(w.join(".git")).unwrap();
        write(&w.join(".claude/skills/a/SKILL.md"), "---\nname: a\n---\nA");
        write(&w.join("secret.txt"), "no");
        let id = w.join(".claude/skills/a/SKILL.md").display().to_string();
        let file = read_file(&cfg, w, &id).unwrap();
        assert_eq!((file.path.as_str(), file.content.as_str(), file.truncated), (".claude/skills/a/SKILL.md", "---\nname: a\n---\nA", false));
        let outside = [w.join("secret.txt").display().to_string(), format!("{}/../../../secret.txt", w.join(".claude/skills/a").display()), "/etc/hostname".to_string()];
        for bad in &outside {
            assert!(read_file(&cfg, w, bad).is_err(), "{bad}");
            assert!(toggle(&cfg, w, bad, false).is_err(), "{bad}");
        }

        assert_eq!(toggle(&cfg, w, &id, false).unwrap(), "a");
        let (_, found) = scan(&cfg, w);
        assert!(!found[0].enabled && found[0].can_toggle);
        let exclude = std::fs::read_to_string(w.join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("/.claude/settings.local.json"));
        toggle(&cfg, w, &id, true).unwrap();
        let (_, found) = scan(&cfg, w);
        assert!(found[0].enabled);
        // Excluded once only.
        assert_eq!(std::fs::read_to_string(w.join(".git/info/exclude")).unwrap(), exclude);
    }

    #[test]
    fn transcripts_yield_loads_and_map_to_tasks() {
        assert_eq!(encode_project_dir(Path::new("/home/a/my.repo/.x")), "-home-a-my-repo--x");
        let jsonl = [
            r#"{"type":"agent-name","agentName":"map-task-7-1","sessionId":"s1"}"#,
            r#"{"type":"assistant","timestamp":"2026-10-01T10:00:00Z","sessionId":"s1","message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"verify"}}]}}"#,
            r#"{"type":"user","timestamp":"2026-10-01T10:05:00Z","sessionId":"s1","message":{"content":"<command-name>/eng:debug</command-name>\n<command-args></command-args>"}}"#,
            r#"{"type":"user","timestamp":"2026-10-01T10:06:00Z","sessionId":"s1","message":{"content":"<command-name>/clear</command-name>"}}"#,
            r#"{"type":"user","isMeta":true,"timestamp":"2026-10-01T10:07:00Z","sessionId":"s1","message":{"content":"<command-name>/verify</command-name>"}}"#,
            r#"not json "Skill""#,
        ]
        .join("\n");
        let loads = parse_transcript(jsonl.as_bytes());
        assert_eq!(loads.iter().map(|l| l.skill.as_str()).collect::<Vec<_>>(), vec!["verify", "eng:debug", "clear"]);
        assert!(loads.iter().all(|l| l.agent.as_deref() == Some("map-task-7-1")));

        let mk = |name: &str, inv: &str, plugin: bool| Skill {
            id: name.into(),
            name: name.into(),
            invocation: inv.into(),
            source: if plugin { SkillSource::Plugin } else { SkillSource::Project },
            plugin: None,
            home: "claude".into(),
            description: String::new(),
            allowed_tools: vec![],
            path: String::new(),
            loads_in: vec![],
            enabled: true,
            can_toggle: true,
            toggle_note: None,
            model_invocable: true,
            usage_known: true,
            usage: SkillUsage::default(),
        };
        let mut skills = vec![mk("verify", "verify", false), mk("debug", "eng:debug", true)];
        let t = |s: &str| DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc);
        let runs = vec![
            Run { session: "map-task-7-1".into(), task_id: Some(7), claude: true, start: t("2026-10-01T09:59:00Z"), end: t("2026-10-01T10:30:00Z") },
            Run { session: "map-task-9-1".into(), task_id: Some(9), claude: true, start: t("2026-10-02T09:00:00Z"), end: t("2026-10-02T10:00:00Z") },
        ];
        // An unnamed session alive only during task 9 is attributed to it by time.
        let mut all = loads.clone();
        all.push(Load { skill: "verify".into(), at: t("2026-10-02T09:30:00Z"), transcript: "s2".into(), agent: None });
        all.push(Load { skill: "verify".into(), at: t("2026-10-05T09:30:00Z"), transcript: "s3".into(), agent: None });
        let unmatched = count_usage(&mut skills, &all, &runs, t("2026-10-06T00:00:00Z"));
        assert_eq!(unmatched, 1); // /clear
        assert_eq!(skills[0].usage.loads, 3);
        assert_eq!(skills[0].usage.loads_week, 3);
        assert_eq!(skills[0].usage.sessions, 3);
        assert_eq!(skills[0].usage.task_ids, vec![7, 9]);
        assert_eq!(skills[0].usage.last_used, Some(t("2026-10-05T09:30:00Z")));
        assert_eq!(skills[1].usage.task_ids, vec![7]);
    }

    #[test]
    fn run_folders_name_project_and_task() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("map-task-3-2/launch.sh"), "#!/bin/sh\nexport MAP_PROJECT='shop'\nexec claude --name map-task-3-2\n");
        write(&dir.path().join("map-claude-1/launch.sh"), "#!/bin/sh\nexec codex -m x\n");
        let shop = runs(&[], dir.path(), "map-", Some("shop"), "claude");
        assert_eq!(shop.len(), 1);
        assert_eq!((shop[0].task_id, shop[0].claude), (Some(3), true));
        let default = runs(&[], dir.path(), "map-", None, "claude");
        assert_eq!(default.len(), 1);
        assert!(!default[0].claude && default[0].task_id.is_none());
    }
}
