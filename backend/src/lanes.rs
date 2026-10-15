//! Lanes: tmux sessions someone else owns (on tmux's default server or a named socket) that MAP watches, reports
//! to Telegram and, only when allowed, types into: `/clear` when a card is done and the CLI says it is time, the
//! lane's `status_prompt` to set it going again, and instructions queued with `POST /api/lanes/{name}/tell`.
//!
//! Ported from agent-watcher (`watcher.py`: `parse_pane`, `classify`, `decide_clear`, `do_clear`, `decide_ask`,
//! `stall_alert`, `done_signature`, `--tell`), which encodes these rules learned the hard way:
//! - type only into an idle pane (no spinner, no `esc to interrupt`, unchanged over two captures, nothing typed on
//!   the input line, no dialog), and (MAP's own rule) with no terminal attached;
//! - never `/clear` while parked at the usage limit, nor mid-card when `only_when_done`; the agent's `need_clear`
//!   in its status file wins over the CLI's hint either way;
//! - once the quota reset time printed by the CLI has passed, the resume prompt beats the cooldown and an empty
//!   `todo/`; before it, nothing is typed and queued tells wait;
//! - an empty `todo/` stops the status prompt, never the notices;
//! - a pane stuck where MAP cannot act (a dialog, unsent text, parked after the reset) alerts, again every
//!   `repeat_mins`, not every tick;
//! - one prompted session per lane per tick, so two agents in one folder never grab the same card.
//!
//! MAP never creates, kills, renames or resizes a lane session: [`LaneTerm`] only lists, captures and types. The one
//! exception is MAP's own lanes (a project's manager and the persistent lanes it opens, `manager.rs`): their sessions
//! are MAP's, started and closed there through the hub like any MAP session; here they are only watched.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration as StdDuration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Duration, FixedOffset, Local, Offset, TimeZone, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::{Config, LaneConfig};
use crate::hub::Hub;
use crate::model::{
    LaneAskWhen, LaneClearWhen, LaneGit, LaneImport, LaneInfo, LaneScreen, LaneSession, LaneStallWhen, LaneState, LaneStatusFile, LaneTasks, LaneTell, Level,
    TellVerb,
};
use crate::terminal::{TermSession, Terminal};
use crate::terminal::tmux::Tmux;

/// A tell nobody could carry out within this long is stale and dropped.
pub const TELL_TTL_MINS: i64 = 180;
/// Lines of scrollback captured above the visible pane.
const CAPTURE_LINES: usize = 60;
/// Lines kept for `GET /api/lanes/{name}/screen`.
const SCREEN_KEEP: usize = 200;
const NEVER: f64 = 1e9;

// ---- reading a Claude Code pane

static BORDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[─━]{10,}").unwrap());
static PROMPT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[❯>]\s?(.*)$").unwrap());
const SPIN: &str = "✻✽✳✶✢∗*·";
static SPINNER: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"^\s*[{SPIN}]\s+(\S.*)$")).unwrap());
/// The line after a finished turn: `<verb> for <time>` (Cooked for 3m 2s).
static DONE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z]+(?: [A-Za-z]+)? for \d+(?:\.\d+)?[hms]\b").unwrap());
static AGENT_ROW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[●◯○]\s+\S").unwrap());
/// A selection dialog puts `❯` before the chosen option; numbered lists in an answer have no cursor.
static DIALOG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[❯>]\s*[1-9]\.\s+\S").unwrap());
const DIALOG_WORDS: [&str; 5] = ["Do you want", "Would you like", "(y/n)", "Choose an option", "esc to cancel"];
static CONTEXT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\d{1,3})%\s+context\s+used").unwrap());
static CLEAR_HINT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)new task\?[^\n]*?/clear").unwrap());
/// The CLI's own limit notice (`You've hit your session limit · resets 5:20am (UTC)`); it counts only with a reset
/// time on the same line or the next, so an agent writing about limits does not park the lane.
static RATE_LIMIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:hit|reached)\s+your\b[^\n]*?\blimit\b|\b(?:usage|session|weekly|hour)\s+limit\s+reached\b").unwrap());
static RESET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bresets?\b\s*(?:at\s+)?(\d{1,2})(?::(\d{2}))?\s*([ap]\.?m\.?)?\s*(?:\(\s*([A-Za-z/_+-]+)\s*\))?").unwrap()
});
/// Timers, token counts and spinners change every second; they are ignored when comparing panes.
static VOLATILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"[{SPIN}]|\d+(\.\d+)?[smhk]\b|\d+")).unwrap());
static ANSI: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b\[[0-9;:?]*[A-Za-z]|\x1b[()][B0]|\x1b[=>]").unwrap());
static AGENT_NOISE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\s*·\s*↓\s*[\d.]+k?\s*tokens?").unwrap());

pub fn strip(text: &str) -> String {
    ANSI.replace_all(text, "").into_owned()
}

/// What one capture shows.
#[derive(Debug, Clone, Default)]
pub struct Pane {
    pub lines: Vec<String>,
    pub box_top: Option<usize>,
    /// Typed, unsent text on the input line (faint suggestions and the cursor do not count).
    pub input_text: String,
    pub spinner_text: String,
    pub agents: Vec<String>,
    pub dialog: bool,
    pub interrupt_hint: bool,
    pub context_pct: Option<u32>,
    pub clear_hint: bool,
    /// The CLI's limit line (empty: not parked).
    pub rate_limit_line: String,
    pub rate_limit_resets: Option<DateTime<Utc>>,
}

/// The local offset now (the CLI prints reset times in the machine's zone unless it says `(UTC)`).
pub fn local_offset() -> FixedOffset {
    Local::now().offset().fix()
}

/// When the quota comes back, read from the CLI's line. `(UTC)` (or GMT, Z) is UTC, no zone is `local`. A time more
/// than 15 minutes past means tomorrow; within 15 minutes it has just reset (a tick two minutes late must not wait a
/// day).
pub fn parse_reset(line: &str, now: DateTime<Utc>, local: FixedOffset) -> Option<DateTime<Utc>> {
    let caps = RESET.captures(line)?;
    let mut hour: u32 = caps[1].parse().ok()?;
    let minute: u32 = caps.get(2).map_or(Some(0), |m| m.as_str().parse().ok())?;
    if hour > 23 || minute > 59 {
        return None;
    }
    let ampm = caps.get(3).map(|m| m.as_str().to_ascii_lowercase().replace('.', "")).unwrap_or_default();
    if ampm == "pm" && hour < 12 {
        hour += 12;
    } else if ampm == "am" && hour == 12 {
        hour = 0;
    }
    let label = caps.get(4).map(|m| m.as_str().to_ascii_uppercase()).unwrap_or_default();
    let tz = if matches!(label.as_str(), "UTC" | "GMT" | "Z" | "UTC+0" | "UTC-0") { FixedOffset::east_opt(0)? } else { local };
    let base = now.with_timezone(&tz);
    let naive = base.date_naive().and_hms_opt(hour, minute, 0)?;
    let mut when = tz.from_local_datetime(&naive).single()?;
    if when < base - Duration::minutes(15) {
        when += Duration::days(1);
    }
    Some(when.with_timezone(&Utc))
}

/// The bottom-most line announcing the limit **and** its reset time (the next line joins in, for a narrow pane that
/// wrapped, but nothing further: the wider the window, the easier a false hit).
pub fn find_rate_limit(lines: &[String]) -> String {
    for i in (0..lines.len()).rev() {
        let blob = format!("{} {}", lines[i], lines.get(i + 1).map_or("", |s| s.as_str()));
        if RATE_LIMIT.is_match(&blob) && RESET.is_match(&blob) {
            return blob.trim().to_string();
        }
    }
    String::new()
}

fn tail_of<T>(v: &[T], n: usize) -> &[T] {
    &v[v.len().saturating_sub(n)..]
}

pub fn parse_pane(raw: &str, now: DateTime<Utc>, local: FixedOffset) -> Pane {
    let raw_lines: Vec<&str> = raw.trim_end_matches('\n').split('\n').collect();
    let lines: Vec<String> = raw_lines.iter().map(|l| strip(l)).collect();
    let borders: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| BORDER.is_match(l)).map(|(i, _)| i).collect();
    let (mut box_top, mut box_bottom) = (None, None);
    if borders.len() >= 2 {
        let (top, bottom) = (borders[borders.len() - 2], borders[borders.len() - 1]);
        // The input box sits at the bottom with only a few lines in it.
        if bottom - top <= 12 && lines.len() - bottom <= 12 {
            (box_top, box_bottom) = (Some(top), Some(bottom));
        }
    }
    let input_text = match (box_top, box_bottom) {
        (Some(top), Some(bottom)) => {
            let mut text = String::new();
            for raw_line in &raw_lines[top + 1..bottom] {
                if PROMPT.is_match(&strip(raw_line)) {
                    text.push_str(&crate::detect::pending_input(raw_line));
                }
            }
            text.trim().to_string()
        }
        // No input box found: fall back to MAP's own reading of the bottom lines (errs towards "someone typed").
        _ => crate::detect::pending_input(raw),
    };
    // The status line: the last non-empty line above the input box.
    let head_end = box_top.unwrap_or(lines.len());
    let status_line = lines[..head_end].iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string()).unwrap_or_default();
    let spinner_text = SPINNER.captures(&status_line).map(|c| c[1].trim().to_string()).unwrap_or_default();
    let tail = tail_of(&lines, 30).join("\n");
    let sidebar: &[String] = box_bottom.map_or(&[], |b| &lines[b + 1..]);
    let agents: Vec<String> = sidebar.iter().filter(|l| AGENT_ROW.is_match(l)).map(|l| l.trim().to_string()).collect();
    let agents = tail_of(&agents, 6).to_vec();
    let dialog = DIALOG_WORDS.iter().any(|w| tail.contains(w)) || tail_of(&lines, 25).iter().any(|l| DIALOG.is_match(l));
    let context_pct = CONTEXT.captures(&tail).and_then(|c| c[1].parse().ok());
    let rate_limit_line = find_rate_limit(tail_of(&lines, 25));
    let rate_limit_resets = if rate_limit_line.is_empty() { None } else { parse_reset(&rate_limit_line, now, local) };
    Pane {
        interrupt_hint: tail.contains("esc to interrupt"),
        clear_hint: CLEAR_HINT.is_match(&tail),
        lines,
        box_top,
        input_text,
        spinner_text,
        agents,
        dialog,
        context_pct,
        rate_limit_line,
        rate_limit_resets,
    }
}

/// The pane without colours, timers, counters and spinners: equal means it did not move.
pub fn normalize(text: &str) -> String {
    VOLATILE.replace_all(&strip(text), "").into_owned()
}

/// A lane session's state as the agent-watcher reads it; parked at the limit is still `Idle` here (the rules check
/// [`Pane::rate_limit_line`] on their own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Raw {
    Busy,
    Waiting,
    PendingInput,
    Idle,
}

fn busy_spinner(p: &Pane) -> bool {
    p.interrupt_hint || (!p.spinner_text.is_empty() && !DONE.is_match(&p.spinner_text))
}

/// Two captures taken a few seconds apart.
pub fn classify(first: &str, second: &str, now: DateTime<Utc>, local: FixedOffset) -> (Raw, String, Pane) {
    let pane = parse_pane(second, now, local);
    let moving = normalize(first) != normalize(second);
    let (state, why) = if pane.dialog {
        (Raw::Waiting, "a question/permission dialog is open on screen".to_string())
    } else if busy_spinner(&pane) {
        (Raw::Busy, if pane.spinner_text.is_empty() { "busy".to_string() } else { pane.spinner_text.clone() })
    } else if moving {
        (Raw::Busy, "screen still moving".to_string())
    } else if !pane.input_text.is_empty() {
        (Raw::PendingInput, format!("unsent text in input box: \"{}\"", pane.input_text))
    } else {
        (Raw::Idle, "idle".to_string())
    };
    (state, why, pane)
}

/// The TUI's frame and status rows, not content (a `●` line is the start of a message and is kept).
fn is_chrome(line: &str) -> bool {
    if BORDER.is_match(line) || line.contains("auto mode on") || line.contains("manual mode on") || line.contains("shift+tab to cycle") {
        return true;
    }
    if let Some(c) = SPINNER.captures(line.trim())
        && (DONE.is_match(&c[1]) || line.contains("esc to interrupt"))
    {
        return true;
    }
    PROMPT.captures(line).is_some_and(|c| c[1].trim().is_empty())
}

fn cap_lines(text: &str, keep: usize) -> String {
    let mut lines: Vec<&str> = text.split('\n').collect();
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    if lines.len() <= keep { lines.join("\n") } else { format!("…\n{}", tail_of(&lines, keep).join("\n")) }
}

/// The last paragraph above the input box, at most `keep` lines.
pub fn screen_tail(p: &Pane, keep: usize) -> String {
    let head = &p.lines[..p.box_top.unwrap_or(p.lines.len())];
    let mut body: Vec<&str> = head.iter().map(|l| l.trim_end()).filter(|l| !is_chrome(l)).collect();
    while body.last().is_some_and(|l| l.trim().is_empty()) {
        body.pop();
    }
    let para = match body.iter().rposition(|l| l.trim().is_empty()) {
        Some(i) => &body[i + 1..],
        None => &body[..],
    };
    cap_lines(&para.join("\n"), keep)
}

fn trim_agent_line(line: &str) -> String {
    let line = AGENT_NOISE.replace_all(line, "");
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 70 { format!("{}…", line.chars().take(69).collect::<String>()) } else { line }
}

/// Whitespace collapsed: a newline would send the prompt half-way.
pub fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- facts around the pane

/// The agent's own status file; missing or broken is `None` (the rules then go by the pane alone).
pub fn status_facts(path: &Path, rel: &str) -> Option<LaneStatusFile> {
    let file = path.join(rel);
    if std::fs::metadata(&file).ok()?.len() > 1 << 20 {
        return None;
    }
    let Value::Object(mut obj) = serde_json::from_str::<Value>(&std::fs::read_to_string(file).ok()?).ok()? else { return None };
    // Agents write `"card": 68` as often as `"card": "068"`; anything else of the wrong type is dropped.
    for key in ["card", "state", "note", "at"] {
        match obj.get(key) {
            Some(Value::Number(n)) => {
                let n = n.to_string();
                obj.insert(key.into(), Value::String(n));
            }
            Some(Value::String(_)) | None => {}
            Some(_) => {
                obj.remove(key);
            }
        }
    }
    if !matches!(obj.get("need_clear"), Some(Value::Bool(_)) | None) {
        obj.remove("need_clear");
    }
    serde_json::from_value(Value::Object(obj)).ok()
}

fn md_files(dir: &Path) -> Vec<(String, std::time::SystemTime)> {
    let Ok(read) = std::fs::read_dir(dir) else { return vec![] };
    read.flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".md")?.to_string();
            Some((stem, e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH)))
        })
        .collect()
}

/// Cards in `tasks_dir`; `None` without a `todo/` folder (the lane does not use cards).
pub fn task_facts(path: &Path, rel: &str) -> Option<LaneTasks> {
    let dir = path.join(rel);
    if !dir.join("todo").is_dir() {
        return None;
    }
    let mut done = md_files(&dir.join("done"));
    done.sort_by_key(|d| std::cmp::Reverse(d.1));
    Some(LaneTasks {
        todo: md_files(&dir.join("todo")).len() as u32,
        todo_human: md_files(&dir.join("todo-human")).len() as u32,
        done: done.len() as u32,
        last_done: done.first().map(|d| d.0.clone()),
    })
}

async fn git(path: &Path, args: &[&str]) -> String {
    let mut cmd = tokio::process::Command::new("git");
    // Read-only: never take the index lock the agent may need.
    cmd.args(args).current_dir(path).env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0").kill_on_drop(true);
    match tokio::time::timeout(StdDuration::from_secs(10), cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        _ => String::new(),
    }
}

pub async fn git_facts(path: &Path) -> Option<LaneGit> {
    if !path.join(".git").exists() {
        return None;
    }
    let branch = git(path, &["branch", "--show-current"]).await;
    let head = git(path, &["log", "-1", "--format=%h|%s|%cr"]).await;
    let mut parts = head.splitn(3, '|').map(String::from);
    let (sha, subject, when) = (parts.next().unwrap_or_default(), parts.next().unwrap_or_default(), parts.next().unwrap_or_default());
    let dirty = git(path, &["status", "--porcelain"]).await.lines().filter(|l| !l.is_empty()).count() as u32;
    let (mut ahead, mut behind) = (None, None);
    if !branch.is_empty() {
        let counts = git(path, &["rev-list", "--left-right", "--count", &format!("origin/{branch}...HEAD")]).await;
        if let Some((b, a)) = counts.split_once('\t') {
            (behind, ahead) = (b.trim().parse().ok(), a.trim().parse().ok());
        }
    }
    Some(LaneGit { branch, sha, subject, when, dirty, ahead, behind })
}

// ---- context used, from the Claude Code transcript

/// The tail of a transcript read for its last usage line.
const TRANSCRIPT_TAIL: u64 = 256 * 1024;

/// Claude Code's folder for the transcripts of sessions started in `path`: every character that is not a letter or a
/// digit becomes `-` (`/home/a/my.app` → `-home-a-my-app`).
pub fn claude_project_dir(claude_dir: &Path, path: &Path) -> PathBuf {
    let mangled: String = path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    claude_dir.join("projects").join(mangled)
}

/// The most recently written `*.jsonl` in `dir` (a `/clear` starts a new file).
pub fn latest_transcript(dir: &Path) -> Option<(PathBuf, std::time::SystemTime)> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .max_by_key(|(_, m)| *m)
}

/// Context tokens of the last main-thread assistant message in the transcript's tail: `input_tokens +
/// cache_read_input_tokens + cache_creation_input_tokens` of its `message.usage`. `None` without one.
pub fn transcript_tokens(file: &Path) -> Option<u64> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(file).ok()?;
    let len = f.metadata().ok()?.len();
    let start = len.saturating_sub(TRANSCRIPT_TAIL);
    f.seek(SeekFrom::Start(start)).ok()?;
    let mut buf = Vec::with_capacity((len - start) as usize);
    f.read_to_end(&mut buf).ok()?;
    let text = String::from_utf8_lossy(&buf);
    // The first line of a cut tail is partial; it fails to parse and is skipped.
    for line in text.lines().rev() {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else { continue };
        if v.get("type").and_then(Value::as_str) != Some("assistant") || v.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let Some(u) = v.pointer("/message/usage") else { continue };
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let total = n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens");
        if total > 0 {
            return Some(total);
        }
    }
    None
}

/// What the newest transcript of `path` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptUsage {
    pub file: PathBuf,
    /// `None`: the file has no assistant message yet (just cleared).
    pub tokens: Option<u64>,
}

pub fn transcript_usage(claude_dir: &Path, path: &Path) -> Option<TranscriptUsage> {
    let mut dir = claude_project_dir(claude_dir, path);
    if !dir.is_dir()
        && let Ok(real) = std::fs::canonicalize(path)
    {
        dir = claude_project_dir(claude_dir, &real);
    }
    let (file, _) = latest_transcript(&dir)?;
    let tokens = transcript_tokens(&file);
    Some(TranscriptUsage { file, tokens })
}

/// `tokens / window` as a whole percentage.
pub fn pct_of(tokens: u64, window: u64) -> u32 {
    if window == 0 { 0 } else { ((tokens as f64 / window as f64) * 100.0).round().min(999.0) as u32 }
}

/// The lane's transcript folder base: `claude_dir`, else `~/.claude`.
pub fn claude_dir_of(cfg: &LaneConfig) -> PathBuf {
    crate::config::expand_home(cfg.claude_dir.as_deref().unwrap_or(Path::new("~/.claude")))
}

/// The pane looks like Claude Code's TUI (its input box between two rules), so its folder's transcript is its own.
fn looks_like_claude(p: &Pane) -> bool {
    p.box_top.is_some()
}

/// Where the context figure came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextReading {
    pub pct: Option<u32>,
    pub tokens: Option<u64>,
    pub source: Option<&'static str>,
    pub transcript: Option<TranscriptUsage>,
}

/// The pane's `N% context used` first; else, for a Claude pane with a folder, the newest transcript.
pub fn read_context(cfg: &LaneConfig, pane: &Pane) -> ContextReading {
    if let Some(pct) = pane.context_pct {
        return ContextReading { pct: Some(pct), tokens: None, source: Some("pane"), transcript: None };
    }
    let Some(path) = cfg.path.as_deref().filter(|_| looks_like_claude(pane)) else { return ContextReading::default() };
    let Some(t) = transcript_usage(&claude_dir_of(cfg), path) else { return ContextReading::default() };
    match t.tokens {
        Some(tokens) => ContextReading { pct: Some(pct_of(tokens, cfg.context_window)), tokens: Some(tokens), source: Some("transcript"), transcript: Some(t) },
        None => ContextReading { pct: None, tokens: None, source: None, transcript: Some(t) },
    }
}

/// After `/clear`: did the transcript show it? A new file, or fewer tokens than before (or none yet).
pub fn transcript_cleared(before: Option<&TranscriptUsage>, after: Option<&TranscriptUsage>) -> bool {
    match (before, after) {
        (Some(b), Some(a)) => a.file != b.file || a.tokens.unwrap_or(0) < b.tokens.unwrap_or(0),
        _ => false,
    }
}

// ---- the rules

fn mins_since(t: Option<DateTime<Utc>>, now: DateTime<Utc>) -> f64 {
    t.map_or(NEVER, |t| (now - t).num_seconds() as f64 / 60.0)
}

pub fn status_at(s: &LaneStatusFile) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.at.as_deref()?.trim()).ok().map(|d| d.with_timezone(&Utc))
}

fn status_is(s: Option<&LaneStatusFile>, state: &str) -> bool {
    s.and_then(|s| s.state.as_deref()) == Some(state)
}

/// What MAP remembers about one lane session (kept across restarts).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionMem {
    pub pane_hash: String,
    pub pane_since: Option<DateTime<Utc>>,
    pub last_clear: Option<DateTime<Utc>>,
    pub last_ask: Option<DateTime<Utc>>,
    pub stall_paged: Option<DateTime<Utc>>,
    pub pause_until: Option<DateTime<Utc>>,
    /// The limit line already announced.
    pub rate_limit_line: Option<String>,
}

/// Why `/clear` is due now, or `""`. A clear loses context for good, so a missed moment beats a wrong one: the pane
/// is idle, not parked at the limit, not cleared within the cooldown, the card is done (`only_when_done`), and then
/// the agent's `need_clear` decides, else the CLI's own hint or context meter after `idle_mins` unchanged.
pub fn decide_clear(rules: &LaneClearWhen, state: Raw, mem: &SessionMem, pane: &Pane, stale_mins: f64, status: Option<&LaneStatusFile>, now: DateTime<Utc>) -> String {
    if !rules.enabled || state != Raw::Idle {
        return String::new();
    }
    // Parked at the limit: the interrupted turn's work is still in the context; resume first.
    if !pane.rate_limit_line.is_empty() {
        return String::new();
    }
    if mins_since(mem.last_clear, now) < f64::from(rules.cooldown_mins) {
        return String::new();
    }
    let fresh = status.is_some_and(|s| mins_since(status_at(s), now) <= f64::from(rules.status_max_age_mins));
    if rules.only_when_done && !(fresh && status_is(status, "done")) {
        return String::new();
    }
    if fresh && let Some(s) = status {
        match s.need_clear {
            Some(true) => return format!("agent asked for a clear before its next card (card {})", s.card.as_deref().unwrap_or("?")),
            Some(false) => return String::new(),
            None => {}
        }
    }
    if stale_mins < f64::from(rules.idle_mins) {
        return String::new();
    }
    if pane.clear_hint {
        return format!("CLI suggests a new topic (/clear) and idle for {} min", stale_mins as i64);
    }
    match pane.context_pct {
        Some(pct) if pct >= rules.context_pct => format!("context at {pct}% (threshold {}%) and idle for {} min", rules.context_pct, stale_mins as i64),
        _ => String::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskKind {
    /// The usage limit reset and the pane is still parked.
    Resume,
    /// The agent reported its card done.
    Done,
    /// Idle and unchanged for `idle_stale_mins`.
    Stale,
}

/// Whether to type the status prompt now, and why. Only into an idle pane. A reset limit beats the cooldown and an
/// empty `todo/`; before the reset nothing is typed. A finished card uses the short `resume_cooldown_mins`; an
/// empty `todo/` stops both the done and the stale prompt (there is no card to pick).
#[allow(clippy::too_many_arguments)] // the watcher's own signature: every input is a separate rule
pub fn decide_ask(rules: &LaneAskWhen, state: Raw, mem: &SessionMem, stale_mins: f64, status: Option<&LaneStatusFile>, no_todo: bool, pane: &Pane, now: DateTime<Utc>) -> Option<(String, AskKind)> {
    if state != Raw::Idle {
        return None;
    }
    let since_ask = mins_since(mem.last_ask, now);
    if !pane.rate_limit_line.is_empty()
        && let Some(resets) = pane.rate_limit_resets
    {
        if resets > now || since_ask < f64::from(rules.resume_cooldown_mins) {
            return None;
        }
        let at = resets.with_timezone(&Local).format("%H:%M");
        return Some((format!("quota reset (CLI says {at}) but the session is still parked at the limit"), AskKind::Resume));
    }
    // A limit line without a readable time: no guess; the usual rules decide (decide_clear still refuses to clear).
    if !no_todo && let Some(s) = status
        && mins_since(status_at(s), now) <= f64::from(rules.status_max_age_mins)
        && status_is(status, "done")
    {
        if since_ask < f64::from(rules.resume_cooldown_mins) {
            return None;
        }
        return Some((format!("agent reported card {} finished", s.card.as_deref().unwrap_or("?")), AskKind::Done));
    }
    if since_ask < f64::from(rules.cooldown_mins) {
        return None;
    }
    if !no_todo && rules.idle_stale_mins > 0 && stale_mins >= f64::from(rules.idle_stale_mins) {
        return Some((format!("idle, screen unchanged for {} min", stale_mins as i64), AskKind::Stale));
    }
    None
}

/// A pane stuck where MAP cannot act by design: a dialog (MAP never answers for a person), unsent text (MAP never
/// appends to it), or still parked after the quota reset (woken and did not move). Plain idle is not a stall (the
/// status prompt handles it), nor is busy (a long build is quiet). After a reset there is `stall_mins` of grace, so
/// MAP's own wake-up does not page.
pub fn stall_alert(state: Raw, pane: &Pane, stale_mins: f64, rules: &LaneStallWhen, now: DateTime<Utc>) -> String {
    let limit = f64::from(rules.stall_mins);
    if stale_mins < limit {
        return String::new();
    }
    let mins = stale_mins as i64;
    match state {
        Raw::Waiting => return format!("🚨 dialog stuck on screen for {mins} min — MAP cannot click it, a person must"),
        Raw::PendingInput => return format!("🚨 unsent text in input box for {mins} min — nobody pressed Enter"),
        _ => {}
    }
    if !pane.rate_limit_line.is_empty() {
        let past = mins_since(pane.rate_limit_resets, now);
        if 0.0 < past && past < NEVER / 10.0 && past >= limit {
            return format!("🚨 still parked at the usage limit for {mins} min although the quota reset {} min ago", past as i64);
        }
    }
    String::new()
}

/// "This card is done", changing only with a new card or a new write: the notice goes out once per card.
pub fn done_signature(status: Option<&LaneStatusFile>) -> String {
    match status {
        Some(s) if status_is(status, "done") => format!("{}|{}", s.card.as_deref().unwrap_or("?"), s.at.as_deref().unwrap_or("")),
        _ => String::new(),
    }
}

/// `*` and `?` globs (session names).
pub fn glob(pattern: &str, name: &str) -> bool {
    fn m(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|i| m(rest, &n[i..])),
            Some(('?', rest)) => !n.is_empty() && m(rest, &n[1..]),
            Some((c, rest)) => n.first() == Some(c) && m(rest, &n[1..]),
        }
    }
    let (p, n): (Vec<char>, Vec<char>) = (pattern.chars().collect(), name.chars().collect());
    m(&p, &n)
}

// ---- the terminal, narrowed

/// The only things MAP does to a lane session: list, capture, type a line. No spawn, kill, rename or resize.
struct LaneTerm(Arc<dyn Terminal>, StdDuration);

impl LaneTerm {
    async fn list(&self) -> Result<Vec<TermSession>> {
        self.0.list().await
    }

    async fn capture(&self, name: &str) -> Result<String> {
        self.0.capture(name, CAPTURE_LINES, true).await
    }

    /// Types `text` literally, waits for the TUI, then presses Enter.
    async fn type_line(&self, name: &str, text: &str) -> Result<()> {
        self.0.send_text(name, text, false).await?;
        tokio::time::sleep(self.1).await;
        self.0.send_key(name, "Enter").await
    }
}

// ---- dashboard changes

/// A lane's changes from the dashboard, `/watch` or the watcher import, over its map.toml block.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LaneOverride {
    pub watch: Option<bool>,
    pub actions: Option<bool>,
    pub clear_enabled: Option<bool>,
    pub status_prompt: Option<String>,
    pub clear_when: Option<LaneClearWhen>,
    pub ask_when: Option<LaneAskWhen>,
    pub stall_when: Option<LaneStallWhen>,
}

impl LaneOverride {
    fn apply(&self, base: &LaneConfig) -> LaneConfig {
        let mut c = base.clone();
        if let Some(v) = &self.clear_when {
            c.clear_when = v.clone();
        }
        if let Some(v) = &self.ask_when {
            c.ask_when = v.clone();
        }
        if let Some(v) = &self.stall_when {
            c.stall_when = v.clone();
        }
        if let Some(v) = self.clear_enabled {
            c.clear_when.enabled = v;
        }
        if let Some(v) = self.watch {
            c.watch = v;
        }
        if let Some(v) = self.actions {
            c.actions = v;
        }
        if let Some(v) = &self.status_prompt {
            c.status_prompt = v.clone();
        }
        c.ask_when.max_sessions_per_round = c.ask_when.max_sessions_per_round.max(1);
        c
    }
}

/// `PATCH /api/lanes/{name}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanePatch {
    pub watch: Option<bool>,
    pub actions: Option<bool>,
    pub clear_enabled: Option<bool>,
    pub status_prompt: Option<String>,
}

// ---- the manager

#[derive(Debug, Clone, Default)]
struct Snapshot {
    sessions: Vec<LaneSession>,
    status: Option<LaneStatusFile>,
    tasks: Option<LaneTasks>,
    git: Option<LaneGit>,
    checked_at: Option<DateTime<Utc>>,
    error: Option<String>,
    report: String,
}

/// The last capture of a lane session (ANSI stripped) and when it was taken.
type Capture = (String, DateTime<Utc>);

/// Lanes live in the hub: their settings, the last tick's view, the last captures.
pub struct Lanes {
    base: Vec<LaneConfig>,
    /// MAP-owned lanes (`manager.rs`), rebuilt from the database.
    owned: Mutex<Vec<LaneConfig>>,
    overrides: Mutex<BTreeMap<String, LaneOverride>>,
    snapshots: Mutex<BTreeMap<String, Snapshot>>,
    screens: Mutex<HashMap<(String, String), Capture>>,
    terms: Mutex<HashMap<String, Arc<dyn Terminal>>>,
    ticked: AtomicBool,
    /// One tick at a time (the loop, or a test).
    busy: tokio::sync::Mutex<()>,
}

impl Lanes {
    pub fn new(config: &Config) -> Self {
        Self {
            base: config.lanes.clone(),
            owned: Mutex::default(),
            overrides: Mutex::default(),
            snapshots: Mutex::default(),
            screens: Mutex::default(),
            terms: Mutex::default(),
            ticked: AtomicBool::new(false),
            busy: tokio::sync::Mutex::new(()),
        }
    }

    /// Reads the dashboard's changes (after a restart).
    pub async fn load(&self, store: &crate::store::Store) {
        let Ok(rows) = store.lane_overrides().await else { return };
        let mut map = self.overrides.lock().unwrap();
        for (lane, data) in rows {
            if let Ok(o) = serde_json::from_str::<LaneOverride>(&data) {
                map.insert(lane, o);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.base.is_empty() && self.owned.lock().unwrap().is_empty()
    }

    /// The `[[lanes]]` blocks of map.toml with the dashboard's changes (no MAP-owned lane).
    pub fn base_configs(&self) -> Vec<LaneConfig> {
        let o = self.overrides.lock().unwrap();
        self.base.iter().map(|b| o.get(&b.name).map_or_else(|| b.clone(), |o| o.apply(b))).collect()
    }

    /// Replaces the MAP-owned lanes (`manager::sync_lanes`).
    pub fn set_owned(&self, lanes: Vec<LaneConfig>) {
        let names: std::collections::HashSet<String> = lanes.iter().map(|l| l.name.clone()).collect();
        *self.owned.lock().unwrap() = lanes;
        // A lane that is gone leaves no stale snapshot behind.
        self.snapshots.lock().unwrap().retain(|k, _| names.contains(k) || self.base.iter().any(|b| &b.name == k));
    }

    fn base_of(&self, name: &str) -> Option<LaneConfig> {
        self.base.iter().find(|b| b.name == name).cloned().or_else(|| self.owned.lock().unwrap().iter().find(|b| b.name == name).cloned())
    }

    /// The first tick has finished.
    pub fn ticked(&self) -> bool {
        self.ticked.load(Ordering::Relaxed)
    }

    /// Every lane with the dashboard's changes applied, in map.toml order, then MAP's own lanes.
    pub fn configs(&self) -> Vec<LaneConfig> {
        let owned = self.owned.lock().unwrap().clone();
        let o = self.overrides.lock().unwrap();
        self.base.iter().chain(owned.iter()).map(|b| o.get(&b.name).map_or_else(|| b.clone(), |o| o.apply(b))).collect()
    }

    pub fn config(&self, name: &str) -> Option<LaneConfig> {
        self.configs().into_iter().find(|c| c.name == name)
    }

    /// `name` is a lane, or a tmux session one lane watches (then also that session).
    pub fn resolve(&self, name: &str) -> Option<(LaneConfig, Option<String>)> {
        let configs = self.configs();
        if let Some(c) = configs.iter().find(|c| c.name == name) {
            return Some((c.clone(), None));
        }
        configs.into_iter().find(|c| c.session_match.iter().any(|g| glob(g, name))).map(|c| (c, Some(name.to_string())))
    }

    fn term(&self, socket: &str, key_delay_ms: u64) -> LaneTerm {
        let mut terms = self.terms.lock().unwrap();
        let t = terms.entry(socket.to_string()).or_insert_with(|| Arc::new(Tmux::new((!socket.is_empty()).then(|| socket.to_string()))));
        LaneTerm(t.clone(), StdDuration::from_millis(key_delay_ms))
    }

    /// Tests: a fake (or private-socket) terminal for `socket`.
    #[cfg(test)]
    pub fn set_terminal(&self, socket: &str, term: Arc<dyn Terminal>) {
        self.terms.lock().unwrap().insert(socket.to_string(), term);
    }

    async fn save_override(&self, hub: &Hub, name: &str, change: impl FnOnce(&mut LaneOverride)) -> Result<LaneConfig> {
        let base = self.base_of(name).ok_or_else(|| anyhow!("no lane `{name}`"))?;
        let data = {
            let mut map = self.overrides.lock().unwrap();
            let o = map.entry(name.to_string()).or_default();
            change(o);
            serde_json::to_string(o)?
        };
        hub.store.set_lane_override(name, &data).await?;
        Ok(self.config(name).unwrap_or(base))
    }

    pub async fn update(&self, hub: &Hub, name: &str, patch: LanePatch) -> Result<LaneConfig> {
        let before = self.config(name).ok_or_else(|| anyhow!("no lane `{name}`"))?;
        let cfg = self
            .save_override(hub, name, |o| {
                if patch.watch.is_some() {
                    o.watch = patch.watch;
                }
                if patch.actions.is_some() {
                    o.actions = patch.actions;
                }
                if patch.clear_enabled.is_some() {
                    o.clear_enabled = patch.clear_enabled;
                }
                if let Some(p) = &patch.status_prompt {
                    o.status_prompt = Some(p.trim().to_string());
                }
            })
            .await?;
        let mut changes = vec![];
        if before.watch != cfg.watch {
            changes.push(if cfg.watch { "watching on ✅" } else { "watching off ⏸" });
        }
        if before.actions != cfg.actions {
            changes.push(if cfg.actions { "actions on" } else { "actions off (watch and notify only)" });
        }
        if before.clear_when.enabled != cfg.clear_when.enabled {
            changes.push(if cfg.clear_when.enabled { "auto /clear on" } else { "auto /clear off" });
        }
        if before.status_prompt != cfg.status_prompt {
            changes.push("status prompt changed");
        }
        if !changes.is_empty() {
            note(hub, name, None, "watch", Level::Info, format!("{name}: {}", changes.join(" · "))).await;
        }
        Ok(cfg)
    }

    fn save_screen(&self, lane: &str, session: &str, raw: &str, at: DateTime<Utc>) {
        let text = strip(raw);
        let lines: Vec<&str> = text.trim_end().lines().collect();
        let kept = tail_of(&lines, SCREEN_KEEP).join("\n");
        self.screens.lock().unwrap().insert((lane.to_string(), session.to_string()), (kept, at));
    }

    /// The last capture of `session` (default: the lane's first session), its last `lines` lines.
    pub fn screen(&self, lane: &str, session: Option<&str>, lines: usize) -> LaneScreen {
        let screens = self.screens.lock().unwrap();
        let session = session.map(String::from).or_else(|| {
            let snaps = self.snapshots.lock().unwrap();
            snaps.get(lane).and_then(|s| s.sessions.first()).map(|s| s.name.clone())
        });
        let found = session.as_ref().and_then(|s| screens.get(&(lane.to_string(), s.clone())));
        let text = found.map(|(t, _)| {
            let all: Vec<&str> = t.lines().collect();
            tail_of(&all, lines.clamp(1, SCREEN_KEEP)).join("\n")
        });
        LaneScreen { lane: lane.into(), session, text: text.unwrap_or_default(), at: found.map(|(_, at)| *at) }
    }

    pub async fn info(&self, hub: &Hub, cfg: &LaneConfig, now: DateTime<Utc>) -> Result<LaneInfo> {
        let snap = self.snapshots.lock().unwrap().get(&cfg.name).cloned().unwrap_or_default();
        let ttl = Duration::minutes(TELL_TTL_MINS);
        let tells = hub.store.lane_tells(Some(&cfg.name), now - ttl, ttl).await?;
        let history = hub.store.lane_history(&cfg.name, 30).await?;
        let status_age_mins = snap.status.as_ref().and_then(status_at).map(|at| (now - at).num_minutes());
        let stopped = cfg.owned.is_some() && !hub.term.exists(&cfg.name).await.unwrap_or(false);
        Ok(LaneInfo {
            project: cfg.owned.as_ref().map(|o| o.project.clone()),
            owned: cfg.owned.as_ref().map(|o| o.kind.clone()),
            stopped,
            name: cfg.name.clone(),
            socket: cfg.socket.clone(),
            session_match: cfg.session_match.clone(),
            path: cfg.path.as_ref().map(|p| p.display().to_string()),
            watch: cfg.watch,
            actions: cfg.actions,
            clear_when: cfg.clear_when.clone(),
            ask_when: cfg.ask_when.clone(),
            stall_when: cfg.stall_when.clone(),
            status_prompt: cfg.status_prompt.clone(),
            sessions: snap.sessions,
            status: snap.status,
            status_age_mins,
            tasks: snap.tasks,
            git: snap.git,
            tells,
            history,
            checked_at: snap.checked_at,
            error: snap.error,
        })
    }

    pub async fn infos(&self, hub: &Hub) -> Result<Vec<LaneInfo>> {
        let now = Utc::now();
        let mut out = vec![];
        for cfg in self.configs() {
            out.push(self.info(hub, &cfg, now).await?);
        }
        Ok(out)
    }

    /// The full report: every lane as the last tick saw it (a muted lane is one line).
    pub fn report(&self, now: DateTime<Utc>) -> String {
        let snaps = self.snapshots.lock().unwrap();
        let mut parts = vec![format!("🕐 lanes · {}", now.with_timezone(&Local).format("%d/%m/%Y %H:%M"))];
        for cfg in self.configs() {
            if !cfg.watch {
                parts.push(format!("🤖 {}\n   ⏸ paused via /watch — send /watch to watch it again", cfg.name));
                continue;
            }
            match snaps.get(&cfg.name) {
                Some(s) if !s.report.is_empty() => parts.push(s.report.clone()),
                _ => parts.push(format!("🤖 {}\n   (not checked yet)", cfg.name)),
            }
        }
        parts.join(&format!("\n\n{}\n\n", "─".repeat(28)))
    }
}

/// A `lane.*` event (the hub's log, the dashboard and Telegram) plus the lane's history.
async fn note(hub: &Hub, lane: &str, session: Option<&str>, kind: &str, level: Level, message: impl AsRef<str>) {
    let message = message.as_ref();
    if let Err(err) = hub.store.insert_lane_action(lane, session, kind, message).await {
        tracing::warn!(lane, "lane history: {err:#}");
    }
    hub.event(level, &format!("lane.{kind}"), Some(lane), None, message).await;
}

fn who(lane: &str, session: &str) -> String {
    if lane == session { lane.to_string() } else { format!("{lane} › {session}") }
}

fn fmt_tell(t: &LaneTell, now: DateTime<Utc>) -> String {
    let what = match t.verb {
        TellVerb::Clear => "clear context".to_string(),
        TellVerb::Ask => "task/status".to_string(),
        TellVerb::Pause => format!("pause {} min", t.mins.unwrap_or(60)),
    };
    let note = if t.note.is_empty() { String::new() } else { format!(" — {}", t.note) };
    let session = if t.session.is_empty() { "any session" } else { t.session.as_str() };
    format!("{session}: {what}{note} (queued {} min ago)", (now - t.at).num_minutes())
}

fn state_label(s: LaneState) -> &'static str {
    match s {
        LaneState::Busy => "🔄 busy",
        LaneState::Waiting => "❓ waiting for you",
        LaneState::PendingInput => "✍️ unsent text in input box",
        LaneState::Idle => "⏸ idle",
        LaneState::RateLimited => "🚦 usage limit",
        LaneState::Missing => "⚠️ session not found",
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct LaneMem {
    done_notified: String,
}

async fn load_memory(hub: &Hub, lane: &str) -> (LaneMem, HashMap<String, SessionMem>) {
    let mut sessions = HashMap::new();
    let mut own = LaneMem::default();
    for (session, data) in hub.store.lane_memory(lane).await.unwrap_or_default() {
        if session.is_empty() {
            own = serde_json::from_str(&data).unwrap_or_default();
        } else if let Ok(m) = serde_json::from_str(&data) {
            sessions.insert(session, m);
        }
    }
    (own, sessions)
}

async fn save_memory(hub: &Hub, lane: &str, own: &LaneMem, sessions: &HashMap<String, SessionMem>, before: &HashMap<String, SessionMem>) {
    let mut rows = vec![(String::new(), serde_json::to_string(own).unwrap_or_default())];
    for (s, m) in sessions {
        if before.get(s) != Some(m) {
            rows.push((s.clone(), serde_json::to_string(m).unwrap_or_default()));
        }
    }
    for (session, data) in rows {
        if let Err(err) = hub.store.set_lane_memory(lane, &session, &data).await {
            tracing::warn!(lane, "lane memory: {err:#}");
        }
    }
}

pub async fn run(hub: Arc<Hub>) {
    let interval = StdDuration::from_secs(hub.config.lane_watch.interval_secs.max(5));
    loop {
        // MAP-owned lanes come and go, so the loop runs even with no `[[lanes]]` block.
        if !hub.lanes.is_empty() {
            tick(&hub, Utc::now()).await;
        }
        tokio::time::sleep(interval).await;
    }
}

/// One round over every lane.
pub async fn tick(hub: &Arc<Hub>, now: DateTime<Utc>) {
    let _one = hub.lanes.busy.lock().await;
    for cfg in hub.lanes.configs() {
        let snap = tick_lane(hub, &cfg, now).await;
        hub.lanes.snapshots.lock().unwrap().insert(cfg.name.clone(), snap);
    }
    hub.lanes.ticked.store(true, Ordering::Relaxed);
}

/// One lane now, out of turn (a Telegram `/ask` should not wait for the next round).
pub async fn tick_one(hub: &Arc<Hub>, name: &str, now: DateTime<Utc>) {
    let _one = hub.lanes.busy.lock().await;
    if let Some(cfg) = hub.lanes.config(name) {
        let snap = tick_lane(hub, &cfg, now).await;
        hub.lanes.snapshots.lock().unwrap().insert(cfg.name.clone(), snap);
    }
}

/// A tell for `session` (or for any session of the lane) with `verb`, not yet used.
fn take_tell(tells: &[LaneTell], session: &str, verb: TellVerb) -> Option<LaneTell> {
    tells.iter().find(|t| t.verb == verb && t.session == session).or_else(|| tells.iter().find(|t| t.verb == verb && t.session.is_empty())).cloned()
}

async fn drop_tell(hub: &Hub, tells: &mut Vec<LaneTell>, t: &LaneTell) {
    tells.retain(|x| !(x.verb == t.verb && x.session == t.session));
    if let Err(err) = hub.store.delete_lane_tell(&t.lane, &t.session, t.verb, Some(t.at)).await {
        tracing::warn!(lane = t.lane, "dropping tell: {err:#}");
    }
}

async fn tick_lane(hub: &Arc<Hub>, cfg: &LaneConfig, now: DateTime<Utc>) -> Snapshot {
    let lanes = &hub.lanes;
    let local = local_offset();
    let lw = &hub.config.lane_watch;
    let lane = cfg.name.as_str();
    let mut snap = Snapshot { checked_at: Some(now), ..Default::default() };
    let mut out = vec![format!("🤖 {lane}")];
    let watching = cfg.watch;
    if !watching {
        out.push("   ⏸ paused via /watch — send /watch to watch it again".into());
    }

    if let Some(path) = &cfg.path {
        snap.tasks = task_facts(path, &cfg.tasks_dir);
        snap.status = status_facts(path, &cfg.status_file);
        snap.git = git_facts(path).await;
    }
    let status = snap.status.clone();
    let no_todo = snap.tasks.as_ref().is_some_and(|t| t.todo == 0);

    // MAP's own lanes run on MAP's tmux server (the hub's terminal); the others on the socket their block names.
    let term = if cfg.owned.is_some() { LaneTerm(hub.term.clone(), StdDuration::from_millis(lw.key_delay_ms)) } else { lanes.term(&cfg.socket, lw.key_delay_ms) };
    let live = match term.list().await {
        Ok(l) => l,
        Err(err) => {
            snap.error = Some(format!("tmux: {err:#}"));
            out.push(format!("   ⚠️ could not list tmux sessions: {err:#}"));
            snap.report = out.join("\n");
            return snap;
        }
    };
    let mut sessions: Vec<TermSession> = live.into_iter().filter(|s| cfg.session_match.iter().any(|g| glob(g, &s.name))).collect();
    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    if sessions.is_empty() {
        out.push(format!("   ⚠️ no session matching {}", cfg.session_match.join(", ")));
    }

    let (mut own, mut mem) = load_memory(hub, lane).await;
    let before = mem.clone();
    let ttl = Duration::minutes(TELL_TTL_MINS);
    let mut tells = hub.store.lane_tells(Some(lane), now - ttl, ttl).await.unwrap_or_default();
    let mut ask_budget = cfg.ask_when.max_sessions_per_round;
    let mut any_pause: Option<LaneTell> = None;

    for ts in &sessions {
        let name = ts.name.as_str();
        let smem = mem.entry(name.to_string()).or_default();
        let first = term.capture(name).await;
        if lw.confirm_secs > 0 {
            tokio::time::sleep(StdDuration::from_secs(lw.confirm_secs)).await;
        }
        let second = term.capture(name).await;
        let (Ok(first), Ok(second)) = (first, second) else {
            out.push(format!("   ▸ {name} · {}", state_label(LaneState::Missing)));
            snap.sessions.push(LaneSession {
                name: name.into(),
                state: LaneState::Missing,
                why: "tmux session not found".into(),
                attached: false,
                context_pct: None,
                context_tokens: None,
                context_window: None,
                context_source: None,
                clear_hint: false,
                rate_limit_line: None,
                rate_limit_resets: None,
                unchanged_mins: 0,
                stall: None,
                paused_until: smem.pause_until,
                last_clear: smem.last_clear,
                last_ask: smem.last_ask,
                sub_agents: vec![],
            });
            continue;
        };
        let (raw, why, mut pane) = classify(&first, &second, now, local);
        lanes.save_screen(lane, name, &second, now);
        // The current CLI no longer prints `N% context used`: the transcript stands in (and feeds clear_when).
        let ctx = read_context(cfg, &pane);
        pane.context_pct = ctx.pct;
        let attached = ts.attached_clients > 0;
        let shown = if raw == Raw::Idle && !pane.rate_limit_line.is_empty() {
            LaneState::RateLimited
        } else {
            match raw {
                Raw::Busy => LaneState::Busy,
                Raw::Waiting => LaneState::Waiting,
                Raw::PendingInput => LaneState::PendingInput,
                Raw::Idle => LaneState::Idle,
            }
        };

        let mut head = format!("   ▸ {name} · {}", state_label(shown));
        match (pane.context_pct, ctx.tokens) {
            (Some(pct), Some(tokens)) => head.push_str(&format!(" · ctx {} ({pct}%)", fmt_tokens(tokens, cfg.context_window))),
            (Some(pct), None) => head.push_str(&format!(" · ctx {pct}%")),
            _ => {}
        }
        out.push(head);
        if let Some(s) = &status {
            let mut line = format!("     📌 card {} · {}", s.card.as_deref().unwrap_or("?"), s.state.as_deref().unwrap_or("?"));
            if let Some(nc) = s.need_clear {
                line.push_str(if nc { " · asks for a context clear" } else { " · can continue without clearing" });
            }
            let age = mins_since(status_at(s), now);
            if age >= 90.0 {
                line.push_str(&if age >= NEVER / 10.0 { " (no time in the status file)".to_string() } else { format!(" (written {} min ago — older than the screen)", age as i64) });
            }
            out.push(line);
            if let Some(n) = s.note.as_deref().filter(|n| !n.is_empty()) {
                out.push(format!("     {n}"));
            }
        }
        if !pane.rate_limit_line.is_empty() {
            match pane.rate_limit_resets {
                Some(r) => {
                    let left = (r - now).num_minutes();
                    let at = r.with_timezone(&Local).format("%H:%M");
                    out.push(format!("     🚦 usage limit hit — quota resets {at}{}", if r > now { format!(" (in {} min · skipping this round)", left + 1) } else { " (time reached)".into() }));
                }
                None => out.push("     🚦 usage limit hit — could not read the reset time off the screen".into()),
            }
            if watching && smem.rate_limit_line.as_deref() != Some(pane.rate_limit_line.as_str()) && smem.rate_limit_line.is_none() {
                let when = pane.rate_limit_resets.map_or("an unknown time".to_string(), |r| r.with_timezone(&Local).format("%H:%M").to_string());
                note(hub, lane, Some(name), "rate_limited", Level::Warning, format!("🚦 {}: usage limit hit — parked until {when}", who(lane, name))).await;
            }
            smem.rate_limit_line = Some(pane.rate_limit_line.clone());
        } else {
            smem.rate_limit_line = None;
        }
        if !pane.agents.is_empty() {
            out.push(format!("     sub-agent: {}", pane.agents.iter().map(|a| trim_agent_line(a)).collect::<Vec<_>>().join(" · ")));
        }
        if raw != Raw::Idle {
            out.push(format!("     {why}"));
        }

        let hash = {
            let digest = Sha256::digest(normalize(&pane.lines.join("\n")).as_bytes());
            digest.iter().take(8).map(|b| format!("{b:02x}")).collect::<String>()
        };
        if smem.pane_hash != hash {
            smem.pane_hash = hash;
            smem.pane_since = Some(now);
        }
        let stale = mins_since(smem.pane_since, now);

        // A pane stuck where MAP cannot act: louder over time, again every `repeat_mins`, reset when it moves.
        let stall = stall_alert(raw, &pane, stale, &cfg.stall_when, now);
        if stall.is_empty() {
            smem.stall_paged = None;
        } else {
            out.push(format!("     {stall}"));
            if watching && mins_since(smem.stall_paged, now) >= f64::from(cfg.stall_when.repeat_mins) {
                smem.stall_paged = Some(now);
                note(hub, lane, Some(name), "stall", Level::Warning, format!("{}: {stall}", who(lane, name))).await;
            }
        }

        // Tells beat every automatic rule but one: the session must be idle (typing over a turn loses work).
        let (cmd_pause, cmd_clear, cmd_ask) = if watching {
            (take_tell(&tells, name, TellVerb::Pause), take_tell(&tells, name, TellVerb::Clear), take_tell(&tells, name, TellVerb::Ask))
        } else {
            (None, None, None)
        };
        if let Some(p) = &cmd_pause {
            let mins = p.mins.unwrap_or(60);
            smem.pause_until = Some(now + Duration::minutes(i64::from(mins)));
            out.push(format!("     ⏳ tell accepted: leaving this session alone for {mins} min"));
            note(hub, lane, Some(name), "tell", Level::Info, format!("⏳ {}: paused for {mins} min (tell){}", who(lane, name), if p.note.is_empty() { String::new() } else { format!(" — {}", p.note) })).await;
            if p.session.is_empty() {
                any_pause = Some(p.clone());
            } else {
                drop_tell(hub, &mut tells, p).await;
            }
        }
        let paused = smem.pause_until.is_some_and(|t| t > now);
        if paused && cmd_clear.is_none() && cmd_ask.is_none() && cmd_pause.is_none() {
            out.push("     ⏳ (paused — skipping this round)".into());
        }
        // Before the reset, typing only hits the limit again: queued tells wait.
        let quota_wait = !pane.rate_limit_line.is_empty() && pane.rate_limit_resets.is_some_and(|r| r > now);

        let can_type = watching && cfg.actions && !attached;
        if watching && cfg.actions && attached {
            out.push("     👤 a terminal is attached — MAP does not type".into());
        }

        // Clear before asking: a clear after the prompt would swallow the answer.
        let mut just_cleared = false;
        if can_type {
            let why = match &cmd_clear {
                // A tell waits while the limit line is on screen, even after the reset: resume first, clear later.
                Some(c) if raw == Raw::Idle && pane.rate_limit_line.is_empty() => format!("a queued tell{}", if c.note.is_empty() { String::new() } else { format!(" ({})", c.note) }),
                _ if paused => String::new(),
                _ => decide_clear(&cfg.clear_when, raw, smem, &pane, stale, status.as_ref(), now),
            };
            if !why.is_empty() {
                match do_clear(&term, cfg, name, lw.clear_settle_secs, ctx.transcript.as_ref(), now, local).await {
                    Ok((ok, after)) => {
                        smem.last_clear = Some(now);
                        if let Some(c) = &cmd_clear {
                            drop_tell(hub, &mut tells, c).await;
                        }
                        let what = if ok { "context cleared" } else { "/clear sent but the screen did not change" };
                        out.push(format!("     🧹 {what} because {why}"));
                        note(hub, lane, Some(name), "clear", if ok { Level::Success } else { Level::Warning }, format!("🧹 {}: {what} because {why}", who(lane, name))).await;
                        pane = after;
                        just_cleared = ok;
                    }
                    Err(err) => out.push(format!("     ⚠️ /clear failed: {err:#}")),
                }
            }
        }

        let mut asked = false;
        if can_type && ask_budget > 0 {
            let mut pick: Option<(String, &str, String, Option<LaneTell>)> = None;
            if let Some(c) = &cmd_ask
                && raw == Raw::Idle
                && !quota_wait
            {
                let prompt = if c.note.is_empty() { cfg.status_prompt.clone() } else { c.note.clone() };
                pick = Some(("a queued tell".into(), "ask", prompt, Some(c.clone())));
            } else if !paused && let Some((reason, kind)) = decide_ask(&cfg.ask_when, raw, smem, stale, status.as_ref(), no_todo, &pane, now) {
                pick = Some((reason, if kind == AskKind::Resume { "resume" } else { "ask" }, cfg.status_prompt.clone(), None));
            }
            // Just cleared as asked: an empty screen waits; set it going in the same round.
            if pick.is_none() && just_cleared {
                pick = Some(("just cleared context".into(), "ask", cfg.status_prompt.clone(), None));
            }
            if let Some((reason, kind, prompt, tell)) = pick {
                let line = one_line(&prompt);
                if line.is_empty() {
                    out.push(format!("     (would prompt because {reason}, but the lane has no status_prompt)"));
                } else {
                    match term.type_line(name, &line).await {
                        Ok(()) => {
                            asked = true;
                            smem.last_ask = Some(now);
                            ask_budget -= 1;
                            if let Some(t) = &tell {
                                drop_tell(hub, &mut tells, t).await;
                            }
                            out.push(format!("     💬 asked because {reason}"));
                            let icon = if kind == "resume" { "▶️" } else { "💬" };
                            note(hub, lane, Some(name), kind, Level::Info, format!("{icon} {}: prompted because {reason}", who(lane, name))).await;
                        }
                        Err(err) => out.push(format!("     ⚠️ typing the prompt failed: {err:#}")),
                    }
                }
            }
        }

        if raw == Raw::Idle && !asked {
            let tail = screen_tail(&pane, 3);
            for l in tail.lines().filter(|l| !l.trim().is_empty()) {
                out.push(format!("     {}", l.trim()));
            }
        }

        snap.sessions.push(LaneSession {
            name: name.into(),
            state: shown,
            why,
            attached,
            context_pct: pane.context_pct,
            context_tokens: if just_cleared { None } else { ctx.tokens },
            context_window: ctx.tokens.map(|_| cfg.context_window),
            context_source: if just_cleared { pane.context_pct.map(|_| "pane".to_string()) } else { ctx.source.map(String::from) },
            clear_hint: pane.clear_hint,
            rate_limit_line: (!pane.rate_limit_line.is_empty()).then(|| pane.rate_limit_line.clone()),
            rate_limit_resets: pane.rate_limit_resets,
            unchanged_mins: stale as i64,
            stall: (!stall.is_empty()).then_some(stall),
            paused_until: smem.pause_until,
            last_clear: smem.last_clear,
            last_ask: smem.last_ask,
            sub_agents: pane.agents.iter().map(|a| trim_agent_line(a)).collect(),
        });
    }
    if let Some(p) = &any_pause {
        drop_tell(hub, &mut tells, p).await;
    }

    // Tells still waiting (session busy, or no such session): visible every round, never silently lost.
    if !tells.is_empty() {
        out.push(format!("   📮 queued tells: {}", tells.iter().map(|t| fmt_tell(t, now)).collect::<Vec<_>>().join(" · ")));
    }
    if let Some(g) = &snap.git {
        let subject: String = g.subject.chars().take(52).collect();
        let mut line = format!("   git: {} {} \"{subject}\" ({})", g.branch, g.sha, g.when);
        if g.dirty > 0 {
            line.push_str(&format!(" · dirty {} files", g.dirty));
        }
        if let Some(a) = g.ahead.filter(|a| *a > 0) {
            line.push_str(&format!(" · unpushed {a} commit"));
        }
        out.push(line);
    }
    if let Some(t) = &snap.tasks {
        out.push(format!(
            "   tasks: todo {} / todo-human {} / done {}{}",
            t.todo,
            t.todo_human,
            t.done,
            t.last_done.as_deref().map_or(String::new(), |d| format!(" · latest: {d}"))
        ));
    }

    // A finished card cannot wait for the hourly report: once per card (card + time written).
    if watching {
        let sig = done_signature(status.as_ref());
        if !sig.is_empty() && sig != own.done_notified {
            let s = status.as_ref().unwrap();
            let todo = snap.tasks.as_ref().map_or(String::new(), |t| format!(" · todo {}", t.todo));
            let n = s.note.as_deref().filter(|n| !n.is_empty()).map_or(String::new(), |n| format!(" — {n}"));
            note(hub, lane, None, "card_done", Level::Success, format!("✅ {lane}: card {} finished{n}{todo}", s.card.as_deref().unwrap_or("?"))).await;
            own.done_notified = sig;
        }
    }
    save_memory(hub, lane, &own, &mem, &before).await;
    snap.report = out.join("\n");
    snap
}

/// `81.6k / 200k`.
pub fn fmt_tokens(tokens: u64, window: u64) -> String {
    let k = |n: u64| {
        let v = n as f64 / 1000.0;
        if v >= 100.0 || (v - v.round()).abs() < 0.05 { format!("{:.0}k", v) } else { format!("{v:.1}k") }
    };
    format!("{} / {}", k(tokens), k(window))
}

/// Types `/clear`, waits, and checks it took: the CLI's hint is gone, and the context is low again: the pane's
/// meter, or (when the context came from the transcript) a new transcript file or fewer tokens in it.
async fn do_clear(term: &LaneTerm, cfg: &LaneConfig, name: &str, settle_secs: u64, before: Option<&TranscriptUsage>, now: DateTime<Utc>, local: FixedOffset) -> Result<(bool, Pane)> {
    term.type_line(name, "/clear").await?;
    tokio::time::sleep(StdDuration::from_secs(settle_secs)).await;
    let after = parse_pane(&term.capture(name).await?, now, local);
    let pane_low = after.context_pct.is_none_or(|p| p < 50);
    let ok = !after.clear_hint
        && match (after.context_pct, before) {
            (Some(_), _) | (None, None) => pane_low,
            (None, Some(b)) => {
                let now_t = cfg.path.as_deref().and_then(|p| transcript_usage(&claude_dir_of(cfg), p));
                transcript_cleared(Some(b), now_t.as_ref())
            }
        };
    Ok((ok, after))
}

// ---- tells

/// Queues `verb` for the lane (or the lane session) `name`; replaces a tell with the same session and verb.
pub async fn tell(hub: &Hub, name: &str, verb: TellVerb, note_text: Option<&str>, mins: Option<u32>, session: Option<&str>, now: DateTime<Utc>) -> Result<LaneTell> {
    let (cfg, from_name) = hub.lanes.resolve(name).ok_or_else(|| anyhow!("no lane watches `{name}`"))?;
    let mut session = session.map(str::trim).filter(|s| !s.is_empty()).map(String::from).or(from_name);
    if let Some(s) = &session
        && !cfg.session_match.iter().any(|g| glob(g, s))
    {
        bail!("lane {} does not watch session `{s}`", cfg.name);
    }
    if session.is_none() {
        // One session in the lane: that one. Else whichever is idle first.
        let snaps = hub.lanes.snapshots.lock().unwrap();
        if let Some(only) = snaps.get(&cfg.name).filter(|s| s.sessions.len() == 1).map(|s| s.sessions[0].name.clone()) {
            session = Some(only);
        }
    }
    if verb == TellVerb::Pause && mins.is_some_and(|m| m == 0 || m > 24 * 60) {
        bail!("mins must be 1-1440");
    }
    let t = LaneTell {
        lane: cfg.name.clone(),
        session: session.unwrap_or_default(),
        verb,
        note: one_line(note_text.unwrap_or("")).chars().take(4000).collect(),
        mins: if verb == TellVerb::Pause { Some(mins.unwrap_or(60)) } else { None },
        at: now,
        expires_at: now + Duration::minutes(TELL_TTL_MINS),
    };
    hub.store.put_lane_tell(&t).await?;
    let what = match t.verb {
        TellVerb::Clear => "clear context".to_string(),
        TellVerb::Ask => "task/status".to_string(),
        TellVerb::Pause => format!("pause {} min", t.mins.unwrap_or(60)),
    };
    let target = if t.session.is_empty() { format!("{} (any session)", cfg.name) } else { who(&cfg.name, &t.session) };
    let note_part = if t.note.is_empty() { String::new() } else { format!(" — {}", t.note) };
    let msg = format!("📮 {target}: queued {what}{note_part}");
    note(hub, &cfg.name, (!t.session.is_empty()).then_some(t.session.as_str()), "tell", Level::Info, msg).await;
    Ok(t)
}

// ---- importing agent-watcher's config.json

/// Overlays the keys of `src` that `cur` has, one at a time (a key of the wrong type is skipped).
fn merge<T: Serialize + serde::de::DeserializeOwned + Clone>(cur: &T, src: Option<&Value>) -> T {
    let Some(Value::Object(src)) = src else { return cur.clone() };
    let mut out = cur.clone();
    for (k, v) in src {
        let Ok(Value::Object(mut obj)) = serde_json::to_value(&out) else { break };
        if !obj.contains_key(k) {
            continue;
        }
        obj.insert(k.clone(), v.clone());
        if let Ok(next) = serde_json::from_value(Value::Object(obj)) {
            out = next;
        }
    }
    out
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}

/// Fills lane settings (status prompt, thresholds) and the Telegram chat and users from agent-watcher's
/// `config.json`, read only. Never the token. The file must be inside the user's home.
pub async fn import_watcher(hub: &Hub, path: &str) -> Result<LaneImport> {
    let home = std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")?;
    import_watcher_in(hub, path, &home).await
}

/// [`import_watcher`] with `home` given (tests).
pub async fn import_watcher_in(hub: &Hub, path: &str, home: &Path) -> Result<LaneImport> {
    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    let file = std::fs::canonicalize(crate::config::expand_home(Path::new(path.trim()))).with_context(|| format!("{path} not found"))?;
    if !file.starts_with(&home) || file == home {
        bail!("the watcher config must be inside {}", home.display());
    }
    if !file.is_file() || std::fs::metadata(&file)?.len() > 4 << 20 {
        bail!("{} is not a config file", file.display());
    }
    let v: Value = serde_json::from_str(&std::fs::read_to_string(&file)?).context("the watcher config is not JSON")?;
    let mut updated = vec![];
    let mut skipped = vec![];
    for t in v.get("targets").and_then(Value::as_array).cloned().unwrap_or_default() {
        let key = t.get("key").and_then(Value::as_str).unwrap_or("?").to_string();
        let tname = t.get("name").and_then(Value::as_str).unwrap_or("").to_string();
        let tpath = t.get("path").and_then(Value::as_str).map(PathBuf::from);
        let mut globs: Vec<String> = t.get("session_match").and_then(Value::as_array).map(|a| a.iter().filter_map(|g| g.as_str().map(String::from)).collect()).unwrap_or_default();
        if let Some(s) = t.get("session").and_then(Value::as_str) {
            globs.push(s.to_string());
        }
        let found = hub.lanes.configs().into_iter().find(|c| {
            c.path.as_deref().zip(tpath.as_deref()).is_some_and(|(a, b)| same_dir(a, b))
                || c.session_match.iter().any(|g| globs.contains(g) || globs.iter().any(|tg| glob(tg, g)))
                || c.name == key
                || c.name == tname
        });
        let Some(cfg) = found else {
            skipped.push(format!("{key}: no [[lanes]] block matches its path or session_match"));
            continue;
        };
        let prompt = t.get("status_prompt").and_then(Value::as_str).map(str::trim).filter(|p| !p.is_empty()).map(String::from);
        let clear_when = merge(&cfg.clear_when, t.get("clear_when"));
        let ask_when = merge(&cfg.ask_when, t.get("ask_when"));
        let stall_when = merge(&cfg.stall_when, t.get("stall_when"));
        hub.lanes
            .save_override(hub, &cfg.name, |o| {
                if prompt.is_some() {
                    o.status_prompt = prompt.clone();
                }
                o.clear_enabled = None;
                o.clear_when = Some(clear_when);
                o.ask_when = Some(ask_when);
                o.stall_when = Some(stall_when);
            })
            .await?;
        note(hub, &cfg.name, None, "watch", Level::Info, format!("{}: settings imported from the agent-watcher target `{key}`", cfg.name)).await;
        updated.push(cfg.name);
    }
    let tg = v.get("telegram");
    let chat_id = tg.and_then(|t| t.get("chat_id")).and_then(Value::as_i64);
    let allowed: Vec<i64> = tg.and_then(|t| t.get("allowed_user_ids")).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_i64).collect()).unwrap_or_default();
    if chat_id.is_some() || !allowed.is_empty() {
        hub.telegram.set_chat(hub, chat_id, (!allowed.is_empty()).then(|| allowed.clone())).await?;
    }
    Ok(LaneImport { updated, skipped, chat_id, allowed_user_ids: allowed })
}

#[cfg(test)]
pub(crate) mod tests;
