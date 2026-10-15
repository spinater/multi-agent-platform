//! The activity graph: who made each task, how MAP's pipeline handed it on (the orchestrator's pick and its gates,
//! the agent session, verify, audit, land, done or failed, the tasks it unblocked), and what each agent used — the
//! commands it ran, the skills, knowledge cards and sub-agents — read from its CLI's own transcript.
//!
//! Transcripts are read only, tail first and then incrementally (the offset is kept per session in
//! `session_traces`), and never written. Claude Code's are `<home>/projects/<folder>/<id>.jsonl` (a session is
//! found by the `--name` MAP gives it, recorded as `customTitle`); Codex's are `<home>/sessions/**/rollout-*.jsonl`
//! (found by folder and start time). Other providers have no trace; the graph still shows their sessions.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Result;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::Value;

use crate::hub::Hub;
use crate::model::{ActivityEdge, ActivityGraph, ActivityNode, Event, KnowledgeQuery, Session, SessionTrace, Task, TaskStatus, TraceItem};

/// The first read of a transcript starts at most this far from its end.
const FIRST_TAIL: u64 = 2 * 1024 * 1024;
/// A later read takes at most this much new text (the rest is skipped, not parsed).
const MAX_STEP: u64 = 4 * 1024 * 1024;
/// Distinct items kept per session (the most used).
const MAX_ITEMS: usize = 200;
const MAX_SAMPLES: usize = 8;
const MAX_DETAIL: usize = 300;

// ---- masking and naming commands

static SECRETS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    vec![
        (Regex::new(r"(?i)(authorization:\s*(?:bearer|token|basic)\s+)[^\s'\x22]+").unwrap(), "${1}***"),
        (Regex::new(r"(?i)(--?(?:token|password|passwd|pass|api[-_]?key|secret|auth)(?:=|\s+))[^\s'\x22]+").unwrap(), "${1}***"),
        (Regex::new(r"(?i)\b([A-Z0-9_]*(?:TOKEN|SECRET|PASSWORD|PASSWD|API_KEY|APIKEY|PRIVATE_KEY)[A-Z0-9_]*=)[^\s'\x22]+").unwrap(), "${1}***"),
        (Regex::new(r"(\s-u\s+)[^\s:]+:[^\s]+").unwrap(), "${1}***"),
        (Regex::new(r"(https?://)[^/\s:@]+:[^/\s@]+@").unwrap(), "${1}***@"),
        (Regex::new(r"\b(?:sk|pk|rk)-[A-Za-z0-9_-]{12,}").unwrap(), "***"),
        (Regex::new(r"\b(?:ghp|gho|ghs|ghu|github_pat|glpat|xox[abp])[-_][A-Za-z0-9_-]{10,}").unwrap(), "***"),
        (Regex::new(r"\b\d{6,}:[A-Za-z0-9_-]{25,}").unwrap(), "***"),
        // Long hex or base64 runs (keys, HMACs); `$VAR` references stay readable.
        (Regex::new(r"\b[A-Fa-f0-9]{32,}\b").unwrap(), "***"),
        (Regex::new(r"\b[A-Za-z0-9+/_-]{40,}={0,2}").unwrap(), "***"),
    ]
});

/// The command with anything secret-looking replaced by `***`, on one line, capped.
pub fn mask(text: &str) -> String {
    let mut out = text.split_whitespace().collect::<Vec<_>>().join(" ");
    for (re, with) in SECRETS.iter() {
        out = re.replace_all(&out, *with).into_owned();
    }
    cap(&out, MAX_DETAIL)
}

fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max - 1).collect::<String>()) }
}

const TWO_WORD: &[&str] = &["git", "cargo", "bun", "bunx", "npm", "npx", "pnpm", "yarn", "go", "docker", "kubectl", "make", "uv", "pip", "pip3", "gh", "glab", "rustup", "deno", "poetry", "tmux", "systemctl"];
const RUNNERS: &[&str] = &["bash", "sh", "zsh", "python", "python3", "node", "ruby", "perl"];

fn base(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// A command's head for grouping: `cargo test`, `bun run lint`, `bash scripts/verify.sh`, `git commit`,
/// `curl $P/tasks`; leading `cd …&&`, `VAR=…` and `timeout N` are skipped.
pub fn command_head(cmd: &str) -> String {
    let first_line = cmd.lines().find(|l| !l.trim().is_empty() && !l.trim_start().starts_with('#')).unwrap_or("");
    let segments = first_line.split("&&").flat_map(|s| s.split("||")).flat_map(|s| s.split(';')).map(|s| s.split('|').next().unwrap_or("").trim());
    for seg in segments {
        let mut toks: Vec<&str> = seg.split_whitespace().collect();
        while toks.first().is_some_and(|t| t.contains('=') && !t.starts_with('-') && !t.starts_with('"')) {
            toks.remove(0);
        }
        while let Some(t) = toks.first().copied() {
            match base(t) {
                "sudo" | "env" | "time" | "nice" | "exec" | "command" => {
                    toks.remove(0);
                }
                "timeout" => {
                    toks.remove(0);
                    if toks.first().is_some_and(|n| n.chars().next().is_some_and(|c| c.is_ascii_digit())) {
                        toks.remove(0);
                    }
                }
                _ => break,
            }
        }
        let Some(head) = toks.first().map(|t| base(t).trim_matches(['"', '\''])) else { continue };
        if matches!(head, "cd" | "export" | "source" | "." | "set" | "pushd" | "popd" | "true" | "echo" | "mkdir" | "") {
            continue;
        }
        let next = toks.get(1).copied().filter(|t| !t.starts_with('-'));
        let out = if TWO_WORD.contains(&head) {
            match (next, toks.get(2)) {
                (Some("run" | "exec"), Some(third)) if matches!(head, "bun" | "npm" | "pnpm" | "yarn" | "uv" | "poetry") && !third.starts_with('-') => format!("{head} run {third}"),
                (Some(n), _) => format!("{head} {n}"),
                _ => head.to_string(),
            }
        } else if RUNNERS.contains(&head) {
            match next {
                Some(script) if script.contains('/') || script.contains('.') => format!("{head} {script}"),
                _ => head.to_string(),
            }
        } else if head == "curl" {
            match toks.iter().skip(1).map(|t| t.trim_matches(['"', '\''])).find(|t| t.starts_with("http") || t.starts_with('$')) {
                Some(url) => {
                    let path = url.split_once("://").map_or(url, |(_, r)| r.find('/').map_or(r, |i| &r[i..]));
                    let path = path.split(['?', '#']).next().unwrap_or(path);
                    format!("curl {}", ID.replace_all(path, "/{id}"))
                }
                None => "curl".into(),
            }
        } else {
            head.to_string()
        };
        return cap(&out, 60);
    }
    cap(first_line.split_whitespace().next().unwrap_or("?"), 60)
}

static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/\d+").unwrap());
static SKILL_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:\.claude|\.agents|\.codex|\.gemini)/skills/([A-Za-z0-9._-]+)(?:/|\b)").unwrap());
static KNOWLEDGE_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\.docs/knowledge(?:/[^\s'\x22`)]*)?)").unwrap());
static KNOWLEDGE_API: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:/knowledge|MAP_KNOWLEDGE_URL\}?)/(search|doc)\b").unwrap());

// ---- parsing transcripts

/// One use, before aggregation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raw {
    pub kind: &'static str,
    pub name: String,
    pub detail: String,
    pub at: Option<DateTime<Utc>>,
    /// The tail of what the call printed, when the transcript has its result.
    pub output: Option<String>,
}

fn raw(kind: &'static str, name: impl Into<String>, detail: impl Into<String>, at: Option<DateTime<Utc>>) -> Raw {
    Raw { kind, name: name.into(), detail: detail.into(), at, output: None }
}

const TAIL_LINES: usize = 3;

/// The last few non-empty lines of a call's output, masked and capped.
pub fn tail(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(TAIL_LINES);
    let out: Vec<String> = lines[start..].iter().map(|l| cap(&mask(l), 160)).collect();
    cap(&out.join("\n"), MAX_DETAIL)
}

/// The text of a Claude `tool_result` block: a string, or an array of `{type: text, text}` parts.
fn result_text(block: &Value) -> String {
    match block.get("content") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => String::new(),
    }
}

/// What a shell command uses besides being a command: a skill's or knowledge card's file, MAP's knowledge API.
fn from_command(cmd: &str, at: Option<DateTime<Utc>>, out: &mut Vec<Raw>) {
    let masked = mask(cmd);
    out.push(raw("command", command_head(cmd), masked.clone(), at));
    if let Some(c) = SKILL_PATH.captures(cmd) {
        out.push(raw("skill", &c[1], masked.clone(), at));
    }
    if let Some(c) = KNOWLEDGE_PATH.captures(cmd) {
        out.push(raw("knowledge", c[1].trim_end_matches(['/', '.', ',']), masked.clone(), at));
    } else if let Some(c) = KNOWLEDGE_API.captures(cmd) {
        out.push(raw("knowledge", format!("MAP knowledge {}", &c[1]), masked, at));
    }
}

fn read_path(path: &str, tool: &str, at: Option<DateTime<Utc>>, out: &mut Vec<Raw>) {
    if let Some(c) = SKILL_PATH.captures(path) {
        out.push(raw("skill", &c[1], path, at));
    } else if let Some(c) = KNOWLEDGE_PATH.captures(path) {
        out.push(raw("knowledge", &c[1], path, at));
    } else {
        out.push(raw("tool", tool, mask(path), at));
    }
}

fn at_of(v: &Value) -> Option<DateTime<Utc>> {
    v.get("timestamp").and_then(Value::as_str).and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|d| d.with_timezone(&Utc))
}

/// Claude Code transcript lines: every `tool_use` block of the assistant's messages.
pub fn parse_claude(text: &str) -> Vec<Raw> {
    let mut out: Vec<Raw> = vec![];
    // A Bash call's id → its command's index in `out`, for the `tool_result` that comes later.
    let mut calls: HashMap<String, usize> = HashMap::new();
    for line in text.lines() {
        if !line.contains("\"tool_use") && !line.contains("\"tool_result\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let at = at_of(&v);
        let Some(content) = v.pointer("/message/content").and_then(Value::as_array) else { continue };
        for block in content.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")) {
            if let Some(&i) = block.get("tool_use_id").and_then(Value::as_str).and_then(|id| calls.get(id)) {
                out[i].output = Some(tail(&result_text(block)));
            }
        }
        for block in content.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_use")) {
            let name = block.get("name").and_then(Value::as_str).unwrap_or("?");
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            let s = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            match name {
                "Bash" => {
                    if let Some(id) = block.get("id").and_then(Value::as_str) {
                        calls.insert(id.to_string(), out.len());
                    }
                    from_command(&s("command"), at, &mut out)
                }
                "Skill" => {
                    let n = [s("skill"), s("name"), s("command")].into_iter().find(|x| !x.is_empty()).unwrap_or_else(|| "?".into());
                    out.push(raw("skill", n.trim_start_matches('/'), s("args"), at));
                }
                "Read" | "View" | "NotebookRead" => read_path(&s("file_path"), name, at, &mut out),
                "Agent" | "Task" => {
                    let n = [s("subagent_type"), s("description"), s("name")].into_iter().find(|x| !x.is_empty()).unwrap_or_else(|| "sub-agent".into());
                    out.push(raw("subagent", cap(&n, 60), cap(&s("description"), MAX_DETAIL), at));
                }
                "Grep" | "Glob" => {
                    let p = s("path");
                    if KNOWLEDGE_PATH.is_match(&p) {
                        read_path(&p, name, at, &mut out);
                    } else {
                        out.push(raw("tool", name, mask(&s("pattern")), at));
                    }
                }
                other => out.push(raw("tool", other, "", at)),
            }
        }
    }
    out
}

/// Codex rollout lines: `function_call` (`exec_command`, `shell`), `local_shell_call` and `custom_tool_call`.
pub fn parse_codex(text: &str) -> Vec<Raw> {
    let mut out: Vec<Raw> = vec![];
    let mut calls: HashMap<String, usize> = HashMap::new();
    for line in text.lines() {
        if !line.contains("call") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        let at = at_of(&v);
        let Some(p) = v.get("payload") else { continue };
        let kind = p.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "function_call_output" {
            if let Some(&i) = p.get("call_id").and_then(Value::as_str).and_then(|id| calls.get(id)) {
                // `output` is the text, or a JSON string `{"output": "...", "metadata": …}`.
                let text = match p.get("output") {
                    Some(Value::String(s)) => serde_json::from_str::<Value>(s).ok().and_then(|o| o.get("output").and_then(Value::as_str).map(String::from)).unwrap_or_else(|| s.clone()),
                    Some(o) => o.get("output").and_then(Value::as_str).unwrap_or("").to_string(),
                    None => String::new(),
                };
                out[i].output = Some(tail(&text));
            }
            continue;
        }
        let name = p.get("name").and_then(Value::as_str).unwrap_or("");
        let args: Value = match p.get("arguments") {
            Some(Value::String(s)) => serde_json::from_str(s).unwrap_or(Value::Null),
            Some(other) => other.clone(),
            None => p.pointer("/action").cloned().unwrap_or(Value::Null),
        };
        let cmd = match args.get("cmd").or_else(|| args.get("command")) {
            Some(Value::String(s)) => Some(s.clone()),
            Some(Value::Array(a)) => {
                let parts: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
                // `["bash", "-lc", "cargo test"]` is the script itself.
                Some(match parts.as_slice() {
                    [sh, flag, script] if RUNNERS.contains(&base(sh)) && flag.starts_with('-') => script.to_string(),
                    _ => parts.join(" "),
                })
            }
            _ => None,
        };
        match (kind, name) {
            ("function_call" | "local_shell_call", "exec_command" | "shell" | "container.exec" | "") if cmd.is_some() => {
                if let Some(id) = p.get("call_id").and_then(Value::as_str) {
                    calls.insert(id.to_string(), out.len());
                }
                from_command(cmd.as_deref().unwrap_or(""), at, &mut out)
            }
            ("function_call" | "custom_tool_call", n) if !n.is_empty() => out.push(raw("tool", n, "", at)),
            _ => {}
        }
    }
    out
}

/// Folds uses into items: one per (kind, name), counted, with the latest detail and a few samples; the
/// [`MAX_ITEMS`] most used are kept.
pub fn aggregate(items: &mut Vec<TraceItem>, new: Vec<Raw>) {
    let mut index: HashMap<(String, String), usize> = items.iter().enumerate().map(|(i, t)| ((t.kind.clone(), t.name.clone()), i)).collect();
    for r in new {
        let key = (r.kind.to_string(), r.name.clone());
        let i = match index.get(&key) {
            Some(&i) => i,
            None => {
                items.push(TraceItem { kind: key.0.clone(), name: key.1.clone(), detail: String::new(), count: 0, first_at: r.at, last_at: r.at, samples: vec![], outputs: vec![] });
                index.insert(key, items.len() - 1);
                items.len() - 1
            }
        };
        let t = &mut items[i];
        t.count += 1;
        if !r.detail.is_empty() {
            t.detail = r.detail.clone();
            // Older items have no outputs: line them up with the samples first.
            t.outputs.resize(t.samples.len(), String::new());
            let output = r.output.unwrap_or_default();
            if t.samples.last() != Some(&r.detail) {
                t.samples.push(r.detail);
                t.outputs.push(output);
                if t.samples.len() > MAX_SAMPLES {
                    t.samples.remove(0);
                    t.outputs.remove(0);
                }
            } else if !output.is_empty()
                && let Some(last) = t.outputs.last_mut()
            {
                *last = output;
            }
        }
        if r.at.is_some() && (t.first_at.is_none() || r.at < t.first_at) {
            t.first_at = r.at;
        }
        if r.at > t.last_at {
            t.last_at = r.at;
        }
    }
    if items.len() > MAX_ITEMS {
        items.sort_by(|a, b| b.count.cmp(&a.count).then(b.last_at.cmp(&a.last_at)));
        items.truncate(MAX_ITEMS);
    }
}

// ---- finding and reading a session's transcript

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    Claude,
    Codex,
}

/// Which transcript format the provider writes (from the variable that moves its state folder).
pub fn reader(hub: &Hub, provider: &str) -> Option<Reader> {
    match hub.spec(provider).ok()?.home_env.as_deref()? {
        "CLAUDE_CONFIG_DIR" => Some(Reader::Claude),
        "CODEX_HOME" => Some(Reader::Codex),
        _ => None,
    }
}

fn head_of(file: &Path, n: usize) -> String {
    let Ok(mut f) = std::fs::File::open(file) else { return String::new() };
    let mut buf = vec![0u8; n];
    let got = f.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..got]).into_owned()
}

fn modified(p: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(p).ok()?.modified().ok().map(DateTime::<Utc>::from)
}

/// `.jsonl` files under `dir` (recursing `depth` levels) written since `since`.
fn recent_jsonl(dir: &Path, since: DateTime<Utc>, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    for e in read.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                recent_jsonl(&p, since, depth - 1, out);
            }
        } else if p.extension().is_some_and(|x| x == "jsonl") && modified(&p).is_some_and(|m| m >= since) && out.len() < 2000 {
            out.push(p);
        }
    }
}

/// The folders a session may have run in: its task's worktree (even when gone) and its project's folder.
async fn folders(hub: &Hub, s: &Session) -> Vec<PathBuf> {
    let mut out = vec![];
    if let Some(id) = s.task_id
        && let Ok(Some(task)) = hub.store.task(id).await
        && let Some(wt) = task.git.as_ref().and_then(|g| g.worktree.clone())
    {
        out.push(PathBuf::from(wt));
    }
    out.push(hub.work_dir(s.project.as_deref()).await);
    out
}

/// The transcript of session `s`, read only.
pub async fn find_file(hub: &Hub, s: &Session) -> Option<PathBuf> {
    let home = hub.provider_home(&s.provider)?;
    let since = s.created_at - chrono::Duration::seconds(60);
    match reader(hub, &s.provider)? {
        Reader::Claude => {
            // A session MAP named itself (a manager, a persistent lane): its conversation id.
            if let Ok(Some(id)) = hub.store.runtime_setting(&format!("kept.conversation.{}", s.name)).await {
                let mut all = vec![];
                recent_jsonl(&home.join("projects"), s.created_at - chrono::Duration::days(365), 1, &mut all);
                if let Some(f) = all.into_iter().find(|p| p.file_stem().is_some_and(|n| n == id.as_str())) {
                    return Some(f);
                }
            }
            let title = [format!("\"customTitle\":\"{}\"", s.name), format!("\"agentName\":\"{}\"", s.name)];
            let mut candidates = vec![];
            for folder in folders(hub, s).await {
                recent_jsonl(&crate::lanes::claude_project_dir(&home, &folder), since, 0, &mut candidates);
            }
            if candidates.is_empty() {
                recent_jsonl(&home.join("projects"), since, 1, &mut candidates);
            }
            candidates.sort_by_key(|p| modified(p));
            candidates.into_iter().rev().find(|p| {
                let h = head_of(p, 64 * 1024).replace("\": \"", "\":\"");
                title.iter().any(|t| h.contains(t.as_str()))
            })
        }
        Reader::Codex => {
            let mut candidates = vec![];
            recent_jsonl(&home.join("sessions"), since, 4, &mut candidates);
            let dirs: Vec<String> = folders(hub, s).await.iter().map(|d| d.display().to_string()).collect();
            let mut best: Option<(DateTime<Utc>, PathBuf)> = None;
            for p in candidates {
                let head = head_of(&p, 16 * 1024);
                let Some(meta) = head.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).find(|v| v.get("type").and_then(Value::as_str) == Some("session_meta")) else { continue };
                let cwd = meta.pointer("/payload/cwd").and_then(Value::as_str).unwrap_or("");
                let started = meta.pointer("/payload/timestamp").and_then(Value::as_str).and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|d| d.with_timezone(&Utc)).or_else(|| at_of(&meta));
                let Some(started) = started.filter(|t| *t >= since) else { continue };
                if dirs.iter().any(|d| d == cwd) && best.as_ref().is_none_or(|(b, _)| started < *b) {
                    best = Some((started, p));
                }
            }
            best.map(|(_, p)| p)
        }
    }
}

/// Reads what is new in `file` from `offset` (complete lines only): the text and the next offset. The first read
/// starts [`FIRST_TAIL`] from the end; a step larger than [`MAX_STEP`] skips ahead.
pub fn read_new(file: &Path, offset: u64) -> Option<(String, u64)> {
    let mut f = std::fs::File::open(file).ok()?;
    let len = f.metadata().ok()?.len();
    let mut start = if offset > len { 0 } else { offset };
    if offset == 0 {
        start = len.saturating_sub(FIRST_TAIL);
    }
    if len - start > MAX_STEP {
        start = len - MAX_STEP;
    }
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.take(len - start).read_to_end(&mut buf).ok()?;
    // Only up to the last newline: a line still being written is read next time.
    let end = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    let mut text = String::from_utf8_lossy(&buf[..end]).into_owned();
    if start > 0 && start != offset {
        // A cut first line.
        text = text.split_once('\n').map(|(_, r)| r.to_string()).unwrap_or_default();
    }
    Some((text, start + end as u64))
}

/// The session's trace, reading what its transcript gained since the last call.
pub async fn refresh(hub: &Hub, s: &Session) -> SessionTrace {
    let mut out = SessionTrace { session: s.name.clone(), provider: s.provider.clone(), file: None, items: vec![] };
    let Some(kind) = reader(hub, &s.provider) else { return out };
    let stored = hub.store.session_trace(&s.name).await.ok().flatten();
    let (mut file, mut offset, mut items) = match &stored {
        Some((f, o, data)) => (Some(PathBuf::from(f)), *o as u64, serde_json::from_str::<Vec<TraceItem>>(data).unwrap_or_default()),
        None => (None, 0, vec![]),
    };
    if file.as_ref().is_none_or(|f| !f.is_file()) {
        let found = find_file(hub, s).await;
        // A new file for the same session (a restart, a `/clear`): read it from its tail, keep the counts.
        if found != file {
            offset = 0;
        }
        file = found;
    }
    let Some(path) = file else {
        out.items = items;
        return out;
    };
    if let Some((text, next)) = read_new(&path, offset) {
        let new = match kind {
            Reader::Claude => parse_claude(&text),
            Reader::Codex => parse_codex(&text),
        };
        aggregate(&mut items, new);
        offset = next;
        let _ = hub.store.set_session_trace(&s.name, &path.display().to_string(), offset as i64, &serde_json::to_string(&items).unwrap_or_default()).await;
    }
    out.file = Some(path.display().to_string());
    out.items = items;
    out
}

// ---- the graph

/// Everything [`build`] draws from (read from the store by [`activity`]; tests pass their own).
pub struct Input<'a> {
    pub slug: &'a str,
    pub prefix: &'a str,
    /// The project's tasks to draw.
    pub tasks: &'a [Task],
    /// The project's sessions (all of them; the ones the tasks used are found by the events).
    pub sessions: &'a [Session],
    /// The tasks' events, oldest first.
    pub events: &'a [Event],
    pub traces: &'a HashMap<String, Vec<TraceItem>>,
    pub queries: &'a [KnowledgeQuery],
    /// The manager's state (`running`, `stopped`, …) when it is on.
    pub manager_state: Option<String>,
    pub since: Option<DateTime<Utc>>,
}

#[derive(Default)]
struct G {
    nodes: Vec<ActivityNode>,
    index: HashMap<String, usize>,
    edges: Vec<ActivityEdge>,
    edge_index: HashMap<(String, String, String), usize>,
}

impl G {
    fn node(&mut self, n: ActivityNode) -> usize {
        if let Some(&i) = self.index.get(&n.id) {
            return i;
        }
        self.index.insert(n.id.clone(), self.nodes.len());
        self.nodes.push(n);
        self.nodes.len() - 1
    }

    fn get(&mut self, id: &str) -> Option<&mut ActivityNode> {
        self.index.get(id).copied().map(|i| &mut self.nodes[i])
    }

    fn edge(&mut self, from: &str, to: &str, kind: &str, label: Option<String>, at: Option<DateTime<Utc>>, count: u32) {
        let key = (from.to_string(), to.to_string(), kind.to_string());
        if let Some(&i) = self.edge_index.get(&key) {
            let e = &mut self.edges[i];
            e.count += count;
            if at > e.last_at {
                e.last_at = at;
            }
            if label.is_some() {
                e.label = label;
            }
            return;
        }
        self.edge_index.insert(key, self.edges.len());
        self.edges.push(ActivityEdge { from: from.into(), to: to.into(), kind: kind.into(), label, count, last_at: at, live: false });
    }
}

fn node(id: impl Into<String>, kind: &str, label: impl Into<String>, layer: u32, row: u32) -> ActivityNode {
    ActivityNode {
        id: id.into(),
        kind: kind.into(),
        label: label.into(),
        detail: None,
        count: 1,
        state: None,
        provider: None,
        model: None,
        task_id: None,
        session: None,
        parent: None,
        layer,
        row,
        started_at: None,
        last_at: None,
        samples: vec![],
        outputs: vec![],
    }
}

/// The creator node a task's `created_by` points at.
fn creator_id(by: &str) -> &'static str {
    match by {
        "manager" => "manager",
        "planner" => "planner",
        _ => "owner",
    }
}

fn usage_edge(kind: &str) -> &'static str {
    match kind {
        "command" => "called",
        "knowledge" => "read",
        "subagent" => "spawned",
        _ => "used",
    }
}

/// Hangs a session's trace items off its node.
fn hang(g: &mut G, owner: &str, items: &[TraceItem]) {
    let Some(&oi) = g.index.get(owner) else { return };
    let (layer, row) = (g.nodes[oi].layer, g.nodes[oi].row);
    for t in items {
        let id = format!("u:{owner}:{}:{}", t.kind, t.name);
        let mut n = node(&id, &t.kind, &t.name, layer, row);
        n.parent = Some(owner.to_string());
        n.count = t.count;
        n.detail = (!t.detail.is_empty()).then(|| t.detail.clone());
        n.samples = t.samples.clone();
        n.outputs = t.outputs.clone();
        n.started_at = t.first_at;
        n.last_at = t.last_at;
        let i = g.node(n);
        if g.nodes[i].count < t.count {
            g.nodes[i].count = t.count;
        }
        g.edge(owner, &id, usage_edge(&t.kind), None, t.last_at, t.count);
    }
}

fn step_label(kind: &str, msg: &str) -> (String, &'static str) {
    let outcome = kind.rsplit('.').next().unwrap_or("");
    let state = match outcome {
        "fail" | "failed" | "no_auditor" | "conflict" | "reverify_fail" | "error" | "worktree_failed" | "worktree_fatal" => "fail",
        "pass" | "merged" | "pushed" | "shared" | "nothing" | "off" | "skipped" | "skipped_reflex" | "closed" | "kept" => "ok",
        "keep" | "blocked" => "waiting",
        _ => "running",
    };
    (cap(msg, 120), state)
}

/// The task flow and what each agent used.
pub fn build(input: &Input) -> ActivityGraph {
    let mut g = G::default();
    let now = Utc::now();
    let by_name: HashMap<&str, &Session> = input.sessions.iter().map(|s| (s.name.as_str(), s)).collect();
    let mgr = format!("{}mgr-{}", input.prefix, input.slug);
    let planner = format!("{}plan-{}", input.prefix, input.slug);
    let lane_prefix = format!("{}lane-{}-", input.prefix, input.slug);

    // Creators, left: the manager, the planner, the owner (only those that made something or are running).
    let mut row = 0;
    let creators: HashSet<&str> = input.tasks.iter().map(|t| creator_id(&t.created_by)).collect();
    let lanes: Vec<&Session> = input.sessions.iter().filter(|s| s.name.starts_with(&lane_prefix) && s.task_id.is_none()).collect();
    let manager_on = input.manager_state.is_some() || by_name.contains_key(mgr.as_str());
    for (id, label) in [("manager", "manager"), ("planner", "planner"), ("owner", "owner")] {
        let session = match id {
            "manager" => by_name.get(mgr.as_str()),
            "planner" => by_name.get(planner.as_str()),
            _ => None,
        };
        let wanted = creators.contains(id)
            || (id == "manager" && (manager_on || lanes.iter().any(|l| l.spawned_by.starts_with("manager:"))))
            || (id == "planner" && session.is_some())
            || (id == "owner" && lanes.iter().any(|l| !l.spawned_by.starts_with("manager:")));
        if !wanted {
            continue;
        }
        let mut n = node(id, id, label, 0, row);
        row += 1;
        if let Some(s) = session {
            n.session = Some(s.name.clone());
            n.provider = Some(s.provider.clone());
            n.model = Some(s.model.clone());
            n.state = Some(enum_word(&s.state));
            n.started_at = Some(s.created_at);
            n.last_at = Some(s.last_activity_at);
        }
        if id == "manager"
            && let Some(st) = &input.manager_state
        {
            n.state = Some(st.clone());
        }
        g.node(n);
        if let Some(s) = session
            && let Some(items) = input.traces.get(&s.name)
        {
            hang(&mut g, id, items);
        }
    }

    // One row per task: creator → task → pick → agent → verify → audit → land → done, retries going round again.
    let mut tasks: Vec<&Task> = input.tasks.iter().collect();
    tasks.sort_by_key(|t| t.id);
    let mut end_of: HashMap<i64, String> = HashMap::new();
    let mut events_by: BTreeMap<i64, Vec<&Event>> = BTreeMap::new();
    for e in input.events {
        if let Some(id) = e.task_id {
            events_by.entry(id).or_default().push(e);
        }
    }
    for t in &tasks {
        let r = row;
        row += 1;
        let tid = format!("task:{}", t.id);
        let mut tn = node(&tid, "task", format!("#{} {}", t.id, cap(&t.title, 60)), 1, r);
        tn.task_id = Some(t.id);
        tn.state = Some(t.status.as_str().to_string());
        tn.provider = t.provider.clone();
        tn.model = t.model.clone();
        tn.started_at = Some(t.created_at);
        tn.last_at = Some(t.updated_at);
        tn.detail = t.result.as_deref().map(|r| cap(r, 200));
        g.node(tn);
        let creator = creator_id(&t.created_by);
        g.edge(creator, &tid, "created", Some(t.created_by.clone()).filter(|s| !s.is_empty()), Some(t.created_at), 1);

        let mut cur = tid.clone();
        let mut layer = 1;
        let mut attempt = 0;
        let (mut intake, mut tier, mut chosen): (Option<String>, Option<String>, Option<String>) = (None, None, None);
        let mut retry_next = false;
        let mut step_ids: HashMap<&'static str, String> = HashMap::new();
        let mut last_edge: Option<usize> = None;
        let events = events_by.get(&t.id).cloned().unwrap_or_default();
        for e in events {
            let k = e.kind.as_str();
            let mut advance = |g: &mut G, id: String, mut n: ActivityNode, kind: &str, label: Option<String>| {
                layer += 1;
                n.layer = layer;
                n.row = r;
                g.node(n);
                g.edge(&cur, &id, kind, label, Some(e.at), 1);
                last_edge = Some(g.edges.len() - 1);
                cur = id;
            };
            if let Some(o) = k.strip_prefix("decision.intake.") {
                if o != "shared_files" {
                    intake = Some(format!("intake: {o}"));
                }
                if matches!(o, "unclear" | "plan_first" | "duplicate")
                    && let Some(n) = g.get(&tid)
                {
                    n.detail = Some(format!("held by intake: {o} — {}", cap(&e.message, 120)));
                }
            } else if let Some(o) = k.strip_prefix("decision.pick.tier_") {
                tier = Some(format!("tier: {o}"));
            } else if k == "decision.pick.chosen" {
                chosen = Some(cap(&e.message, 120));
            } else if k == "session.started" {
                attempt += 1;
                step_ids.clear();
                let pick = format!("pick:{}:{attempt}", t.id);
                let mut pn = node(&pick, "orchestrator", "orchestrator", 0, r);
                let gates: Vec<String> = [intake.clone(), tier.take(), chosen.take()].into_iter().flatten().collect();
                pn.detail = (!gates.is_empty()).then(|| gates.join(" · "));
                pn.task_id = Some(t.id);
                pn.started_at = Some(e.at);
                pn.state = Some("ok".into());
                let kind = if retry_next { "retry" } else { "picked" };
                advance(&mut g, pick.clone(), pn, kind, Some(format!("attempt {attempt}")));
                retry_next = false;
                let sname = e.session.clone().unwrap_or_default();
                let aid = if g.index.contains_key(&format!("agent:{sname}")) { format!("agent:{sname}:{attempt}") } else { format!("agent:{sname}") };
                let mut an = node(&aid, "agent", sname.clone(), 0, r);
                an.session = Some(sname.clone());
                an.task_id = Some(t.id);
                an.started_at = Some(e.at);
                match by_name.get(sname.as_str()) {
                    Some(s) => {
                        an.provider = Some(s.provider.clone());
                        an.model = Some(s.model.clone());
                        an.state = Some(enum_word(&s.state));
                        an.last_at = Some(s.last_activity_at);
                    }
                    None => {
                        // "Started claude (sonnet)"
                        let m = e.message.trim_start_matches("Started ");
                        let (p, rest) = m.split_once(' ').unwrap_or((m, ""));
                        an.provider = Some(p.to_string());
                        an.model = Some(rest.trim_matches(['(', ')']).to_string()).filter(|s| !s.is_empty());
                        an.state = Some("exited".into());
                    }
                }
                let label = match (&an.provider, &an.model) {
                    (Some(p), Some(m)) => Some(format!("{p} · {m}")),
                    (Some(p), None) => Some(p.clone()),
                    _ => None,
                };
                advance(&mut g, aid.clone(), an, "assigned", label);
                if let Some(items) = input.traces.get(&sname) {
                    hang(&mut g, &aid, items);
                }
            } else if let Some(rest) = k.strip_prefix("decision.") {
                let phase = rest.split('.').next().unwrap_or("");
                let (step, label): (&'static str, &str) = match phase {
                    "verify" => ("verify", "verify"),
                    "audit" => ("audit", "audit"),
                    "land" => ("land", "land"),
                    "review" => ("review", "review"),
                    "post" => ("post", "post_done"),
                    "end" => ("done", "done"),
                    "retry" if rest == "retry.failed" => ("failed", "failed"),
                    "retry" if rest == "retry.again" => {
                        retry_next = true;
                        if let Some(n) = g.get(&cur) {
                            n.state = Some("fail".into());
                        }
                        continue;
                    }
                    "launch" if rest == "launch.worktree_fatal" => ("failed", "failed"),
                    _ => continue,
                };
                let (detail, state) = step_label(k, &e.message);
                let id = match step_ids.get(step) {
                    Some(id) => id.clone(),
                    None => {
                        let id = if matches!(step, "done" | "failed") { format!("step:{}:{step}", t.id) } else { format!("step:{}:{attempt}:{step}", t.id) };
                        let mut n = node(&id, "step", label, 0, r);
                        n.task_id = Some(t.id);
                        n.started_at = Some(e.at);
                        advance(&mut g, id.clone(), n, "handed", None);
                        step_ids.insert(step, id.clone());
                        id
                    }
                };
                if let Some(n) = g.get(&id) {
                    n.last_at = Some(e.at);
                    if state != "running" || n.state.is_none() {
                        n.state = Some(if matches!(step, "done") { "ok".into() } else if step == "failed" { "fail".into() } else { state.into() });
                    }
                    if k.ends_with("cmd_start") || k.ends_with("cmd_pass") {
                        if !n.samples.contains(&detail) {
                            n.samples.push(detail.clone());
                        }
                    } else {
                        n.detail = Some(detail);
                    }
                }
            }
        }
        // A task cancelled, or still waiting before its first session.
        if t.status == TaskStatus::Cancelled && !cur.ends_with(":done") && !cur.ends_with(":failed") {
            let id = format!("step:{}:cancelled", t.id);
            layer += 1;
            let mut n = node(&id, "step", "cancelled", layer, r);
            n.state = Some("fail".into());
            n.task_id = Some(t.id);
            g.node(n);
            g.edge(&cur, &id, "handed", None, Some(t.updated_at), 1);
            cur = id;
        }
        if !t.status.is_terminal() && t.status != TaskStatus::Review {
            if let Some(i) = last_edge {
                g.edges[i].live = true;
            }
            if let Some(n) = g.get(&cur)
                && n.kind == "step"
            {
                n.state = Some("running".into());
            }
        }
        end_of.insert(t.id, cur);
    }

    // A done task unblocks the ones that waited for it.
    for t in &tasks {
        for d in &t.depends_on {
            if let Some(end) = end_of.get(d) {
                let at = tasks.iter().find(|x| x.id == *d).map(|x| x.updated_at);
                g.edge(&end.clone(), &format!("task:{}", t.id), "unblocks", None, at, 1);
            }
        }
    }

    // Lanes: spawned by the manager or the owner, with what they used.
    for s in &lanes {
        let id = format!("agent:{}", s.name);
        let mut n = node(&id, "agent", s.name.trim_start_matches(&lane_prefix), 1, row);
        row += 1;
        n.session = Some(s.name.clone());
        n.provider = Some(s.provider.clone());
        n.model = Some(s.model.clone());
        n.state = Some(enum_word(&s.state));
        n.started_at = Some(s.created_at);
        n.last_at = Some(s.last_activity_at);
        n.detail = Some("persistent lane".into());
        g.node(n);
        let from = if s.spawned_by.starts_with("manager:") { "manager" } else { "owner" };
        g.edge(from, &id, "spawned", Some("lane".into()), Some(s.created_at), 1);
        if let Some(items) = input.traces.get(&s.name) {
            hang(&mut g, &id, items);
        }
    }

    // MAP's own log of knowledge searches: the manager's, and an agent's while it was the only one running.
    for q in input.queries.iter().filter(|q| input.since.is_none_or(|s| q.at >= s)) {
        let owner = match q.by.as_str() {
            "manager" if g.index.contains_key("manager") => Some("manager".to_string()),
            "agent" => {
                let running: Vec<&ActivityNode> = g.nodes.iter().filter(|n| n.kind == "agent" && n.started_at.is_some_and(|a| a <= q.at) && n.last_at.is_none_or(|l| l >= q.at)).collect();
                (running.len() == 1).then(|| running[0].id.clone())
            }
            _ => None,
        };
        if let Some(owner) = owner {
            let item = TraceItem { kind: "knowledge".into(), name: format!("search: {}", cap(&q.query, 50)), detail: format!("{} hits", q.hits), count: 1, first_at: Some(q.at), last_at: Some(q.at), samples: vec![], outputs: vec![] };
            hang(&mut g, &owner, &[item]);
        }
    }

    ActivityGraph { project: input.slug.into(), since: input.since, generated_at: now, nodes: g.nodes, edges: g.edges }
}

fn enum_word<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()
}

/// `GET /api/projects/{slug}/activity`: tasks still open or changed since `since` (default: the last 24 hours;
/// `None` with `all`), or one task and its neighbours.
pub async fn activity(hub: &Hub, slug: &str, since: Option<DateTime<Utc>>, task: Option<i64>) -> Result<ActivityGraph> {
    let all: Vec<Task> = hub.store.tasks().await?.into_iter().filter(|t| t.project.as_deref() == Some(slug)).collect();
    let tasks: Vec<Task> = match task {
        Some(id) => {
            let me = all.iter().find(|t| t.id == id).cloned();
            let deps: HashSet<i64> = me.as_ref().map(|t| t.depends_on.iter().copied().collect()).unwrap_or_default();
            all.iter().filter(|t| t.id == id || deps.contains(&t.id) || t.depends_on.contains(&id)).cloned().collect()
        }
        None => all.into_iter().filter(|t| !t.status.is_terminal() || since.is_none_or(|s| t.updated_at >= s)).collect(),
    };
    let ids: Vec<i64> = tasks.iter().map(|t| t.id).collect();
    let events = hub.store.events_of_tasks(&ids).await?;
    let sessions: Vec<Session> = hub.store.sessions().await?.into_iter().filter(|s| s.project.as_deref() == Some(slug)).collect();
    let used: HashSet<String> = events.iter().filter(|e| e.kind == "session.started").filter_map(|e| e.session.clone()).collect();
    let prefix = &hub.config.workspace.session_prefix;
    let own = [format!("{prefix}mgr-{slug}"), format!("{prefix}plan-{slug}")];
    let lane_prefix = format!("{prefix}lane-{slug}-");
    let mut traces = HashMap::new();
    for s in sessions.iter().filter(|s| used.contains(&s.name) || own.contains(&s.name) || s.name.starts_with(&lane_prefix)) {
        traces.insert(s.name.clone(), refresh(hub, s).await.items);
    }
    let queries = crate::knowledge::recent_queries(hub, slug, 300).await;
    let manager_state = match hub.store.project(slug).await?.is_some_and(|p| p.settings.manager.enabled) {
        true => crate::manager::info(hub, slug).await.ok().map(|i| i.state),
        false => None,
    };
    let input = Input { slug, prefix, tasks: &tasks, sessions: &sessions, events: &events, traces: &traces, queries: &queries, manager_state, since };
    Ok(build(&input))
}

#[cfg(test)]
mod tests;
