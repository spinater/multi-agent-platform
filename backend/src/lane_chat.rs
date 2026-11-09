//! Chat with a lane from the dashboard, as from Claude's phone app: the conversation read from the lane's Claude Code
//! transcript (the owner's words, the agent's replies, one line per tool call), and a message box that types into the
//! lane's tmux session. A Claude Code lane takes a message while it works and runs it next; an open question or
//! permission dialog is answered with its keys, from the screen shown under the chat. Owner token only (`/api/lanes`).
//!
//! The transcript is the newest `*.jsonl` in the lane folder's Claude project directory whose `agent-name` is the
//! session's name, so two lanes in one folder never read each other's. A lane with none (Codex, a plain shell) shows
//! its screen only. Text is masked (`trace::SECRETS`) and cut, as everywhere MAP shows a session.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration as StdDuration, SystemTime};

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

use crate::config::LaneConfig;
use crate::hub::Hub;
use crate::lanes::{self, LaneTerm, Raw};

/// Messages sent back: the newest.
const MESSAGES: usize = 80;
/// The transcript's tail read for them.
const TAIL: u64 = 3 * 1024 * 1024;
/// A message's text kept; a tool call's line.
const TEXT_CHARS: usize = 6000;
const TOOL_CHARS: usize = 160;
/// Transcripts looked at for the session's name, newest first.
const CANDIDATES: usize = 30;
/// The screen sent with the chat, in lines.
const SCREEN_LINES: usize = 30;
const MAX_INPUT: usize = 20_000;
/// Keys the chat may press: the session console's but C-c (twice quits the CLI), and a dialog's numbered options.
const KEYS: &[&str] = &["Escape", "Enter", "Up", "Down", "Tab", "BTab", "y", "n", "1", "2", "3", "4", "5"];

#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct ChatMessage {
    pub id: String,
    pub at: Option<DateTime<Utc>>,
    /// `owner`, `agent` or `tool`.
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LaneChat {
    pub lane: String,
    /// The tmux session shown; `None` when no session of the lane is running.
    pub session: Option<String>,
    /// Every running session of the lane, to switch between.
    pub sessions: Vec<String>,
    /// `busy`, `idle`, `waiting` (a dialog is open), `pending` (unsent text on the input line) or `missing`.
    pub state: String,
    pub why: String,
    pub attached: bool,
    /// The transcript's file name; `None` for a lane without one.
    pub transcript: Option<String>,
    pub messages: Vec<ChatMessage>,
    /// The bottom of the screen: how a dialog is answered, and all a lane without a transcript has.
    pub screen: String,
}

static COMMAND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<command-name>([^<]*)</command-name>").unwrap());
static ARGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<command-args>([^<]*)</command-args>").unwrap());
static PASTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"</?pasted_content[^>]*>").unwrap());

type Cached = (SystemTime, u64, Vec<ChatMessage>);
static CACHE: LazyLock<Mutex<HashMap<PathBuf, Cached>>> = LazyLock::new(Default::default);

fn cut(text: &str, chars: usize) -> String {
    let mut out: String = text.trim().chars().take(chars).collect();
    if text.trim().chars().count() > chars {
        out.push('…');
    }
    for (re, with) in crate::trace::SECRETS.iter() {
        out = re.replace_all(&out, *with).into_owned();
    }
    out
}

/// A tool call as one line: its name and the input that says what it did.
fn tool_line(item: &Value) -> String {
    let name = item.get("name").and_then(Value::as_str).unwrap_or("tool");
    let input = item.get("input");
    let pick = ["description", "command", "file_path", "pattern", "path", "url", "query", "prompt", "skill"]
        .iter()
        .find_map(|k| input.and_then(|i| i.get(*k)).and_then(Value::as_str))
        .unwrap_or("");
    cut(&format!("{name} {}", pick.split_whitespace().collect::<Vec<_>>().join(" ")), TOOL_CHARS)
}

/// The owner's words in a user entry: their text, a slash command as typed; `None` for tool results and CLI notes.
fn owner_text(content: &Value) -> Option<String> {
    let text = match content {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().filter(|i| i.get("type").and_then(Value::as_str) == Some("text")).filter_map(|i| i.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    if let Some(c) = COMMAND.captures(&text) {
        let args = ARGS.captures(&text).map(|a| a[1].trim().to_string()).unwrap_or_default();
        return Some(format!("{} {args}", c[1].trim()).trim().to_string());
    }
    let unwrapped = PASTED.replace_all(&text, "");
    let t = unwrapped.trim();
    (!t.is_empty() && !t.starts_with('<') && !t.starts_with("[Request interrupted")).then(|| t.to_string())
}

/// The chat in transcript lines, oldest first.
pub fn messages(text: &str) -> Vec<ChatMessage> {
    let mut out = vec![];
    for line in text.lines() {
        // Tool output is most of a transcript and never shown: skip it before parsing.
        if line.contains("\"tool_result\"") && !line.contains("\"type\":\"text\"") {
            continue;
        }
        let Ok(e) = serde_json::from_str::<Value>(line) else { continue };
        if e.get("isSidechain").and_then(Value::as_bool) == Some(true) || e.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let at = e.get("timestamp").and_then(Value::as_str).and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc));
        let id = e.get("uuid").and_then(Value::as_str).unwrap_or("").to_string();
        let Some(content) = e.pointer("/message/content") else { continue };
        match e.get("type").and_then(Value::as_str) {
            Some("user") => {
                if let Some(t) = owner_text(content) {
                    out.push(ChatMessage { id, at, role: "owner".into(), text: cut(&t, TEXT_CHARS) });
                }
            }
            Some("assistant") => {
                for (k, item) in content.as_array().into_iter().flatten().enumerate() {
                    let (role, text) = match item.get("type").and_then(Value::as_str) {
                        Some("text") => ("agent", cut(item.get("text").and_then(Value::as_str).unwrap_or(""), TEXT_CHARS)),
                        Some("tool_use") => ("tool", tool_line(item)),
                        _ => continue,
                    };
                    if !text.is_empty() {
                        out.push(ChatMessage { id: format!("{id}#{k}"), at, role: role.into(), text });
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// The last `len` bytes of `file` (the first line of a cut tail is partial and fails to parse).
fn tail(file: &Path, len: u64) -> Option<String> {
    let mut f = std::fs::File::open(file).ok()?;
    let size = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(size.saturating_sub(len))).ok()?;
    let mut buf = vec![];
    f.read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// The session's name in a transcript: its last `agent-name` record, in its tail or else its head.
fn agent_name(file: &Path) -> Option<String> {
    let find = |text: &str| {
        text.lines()
            .rev()
            .filter(|l| l.contains("\"agent-name\""))
            .find_map(|l| serde_json::from_str::<Value>(l).ok()?.get("agentName")?.as_str().map(String::from))
    };
    if let Some(n) = tail(file, 512 * 1024).as_deref().and_then(find) {
        return Some(n);
    }
    let mut head = vec![0; 256 * 1024];
    let n = std::fs::File::open(file).ok()?.read(&mut head).ok()?;
    find(&String::from_utf8_lossy(&head[..n]))
}

/// The newest transcript in `dir` named `session`.
pub fn transcript_in(dir: &Path, session: &str) -> Option<PathBuf> {
    let mut files: Vec<(PathBuf, SystemTime)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|e| Some((e.path(), e.metadata().ok()?.modified().ok()?)))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.1));
    files.into_iter().take(CANDIDATES).map(|f| f.0).find(|f| agent_name(f).as_deref() == Some(session))
}

fn transcript(cfg: &LaneConfig, session: &str) -> Option<PathBuf> {
    let path = cfg.path.as_deref()?;
    let base = lanes::claude_dir_of(cfg);
    let dir = lanes::claude_project_dir(&base, path);
    transcript_in(&dir, session).or_else(|| transcript_in(&lanes::claude_project_dir(&base, &std::fs::canonicalize(path).ok()?), session))
}

/// The chat of `file`, parsed again only when it changed.
fn chat_of(file: &Path) -> Vec<ChatMessage> {
    let Ok(meta) = std::fs::metadata(file) else { return vec![] };
    let key = (meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), meta.len());
    if let Some((m, l, msgs)) = CACHE.lock().unwrap().get(file)
        && (*m, *l) == key
    {
        return msgs.clone();
    }
    let mut msgs = tail(file, TAIL).map(|t| messages(&t)).unwrap_or_default();
    msgs.drain(..msgs.len().saturating_sub(MESSAGES));
    CACHE.lock().unwrap().insert(file.to_path_buf(), (key.0, key.1, msgs.clone()));
    msgs
}

fn term_of(hub: &Hub, cfg: &LaneConfig) -> LaneTerm {
    let delay = hub.config.lane_watch.key_delay_ms;
    if cfg.owned.is_some() { LaneTerm(hub.term.clone(), StdDuration::from_millis(delay)) } else { hub.lanes.term(&cfg.socket, delay) }
}

/// The lane's running sessions, sorted, and whether each has a client attached.
async fn sessions(term: &LaneTerm, cfg: &LaneConfig) -> anyhow::Result<Vec<(String, bool)>> {
    let mut s: Vec<(String, bool)> = term.list().await?.into_iter().filter(|s| cfg.session_match.iter().any(|g| lanes::glob(g, &s.name))).map(|s| (s.name, s.attached_clients > 0)).collect();
    s.sort();
    Ok(s)
}

fn state_name(raw: Raw) -> &'static str {
    match raw {
        Raw::Busy => "busy",
        Raw::Waiting => "waiting",
        Raw::PendingInput => "pending",
        Raw::Idle => "idle",
    }
}

pub async fn chat(hub: &Hub, cfg: &LaneConfig, want: Option<&str>) -> anyhow::Result<LaneChat> {
    let term = term_of(hub, cfg);
    let running = sessions(&term, cfg).await?;
    let pick = want.and_then(|w| running.iter().find(|s| s.0 == w)).or(running.first()).cloned();
    let mut out = LaneChat {
        lane: cfg.name.clone(),
        session: pick.as_ref().map(|s| s.0.clone()),
        sessions: running.iter().map(|s| s.0.clone()).collect(),
        state: "missing".into(),
        why: "no session of this lane is running".into(),
        attached: pick.as_ref().is_some_and(|s| s.1),
        transcript: None,
        messages: vec![],
        screen: String::new(),
    };
    let Some((name, _)) = pick else { return Ok(out) };
    let raw = term.capture(&name).await?;
    let (state, why, pane) = lanes::classify(&raw, &raw, Utc::now(), lanes::local_offset());
    (out.state, out.why) = (state_name(state).into(), why);
    out.screen = lanes::screen_tail(&pane, SCREEN_LINES);
    if let Some(file) = transcript(cfg, &name) {
        out.transcript = file.file_name().map(|f| f.to_string_lossy().into_owned());
        out.messages = chat_of(&file);
    }
    Ok(out)
}

/// What the owner sends: a message, or one key.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatSend {
    pub session: Option<String>,
    pub text: Option<String>,
    pub key: Option<String>,
}

#[derive(Debug)]
pub enum SendError {
    Bad(String),
    Conflict(String),
    Failed(anyhow::Error),
}

/// Types the message (then Enter) or presses the key in the lane's session. A message is refused while a dialog is
/// open (answer it with its keys) or the input line holds unsent text (it would be sent along).
pub async fn send(hub: &Hub, cfg: &LaneConfig, body: &ChatSend) -> Result<LaneChat, SendError> {
    let term = term_of(hub, cfg);
    let running = sessions(&term, cfg).await.map_err(SendError::Failed)?;
    let want = body.session.as_deref().filter(|s| !s.is_empty());
    let Some((name, _)) = want.map_or(running.first(), |w| running.iter().find(|s| s.0 == w)).cloned() else {
        return Err(SendError::Conflict("no session of this lane is running".into()));
    };
    match (body.text.as_deref().map(str::trim).filter(|t| !t.is_empty()), body.key.as_deref()) {
        (Some(text), None) => {
            if text.chars().count() > MAX_INPUT {
                return Err(SendError::Bad(format!("a message is at most {MAX_INPUT} characters")));
            }
            let raw = term.capture(&name).await.map_err(SendError::Failed)?;
            let (state, why, _) = lanes::classify(&raw, &raw, Utc::now(), lanes::local_offset());
            if matches!(state, Raw::Waiting | Raw::PendingInput) {
                return Err(SendError::Conflict(format!("not sent: {why}")));
            }
            term.type_line(&name, text).await.map_err(SendError::Failed)?;
            hub.event(crate::model::Level::Info, "lane.chat", Some(&name), None, format!("The owner sent {} characters from the dashboard", text.chars().count())).await;
        }
        (None, Some(key)) if KEYS.contains(&key) => term.0.send_key(&name, key).await.map_err(SendError::Failed)?,
        (None, Some(_)) => return Err(SendError::Bad(format!("key must be one of {}", KEYS.join(", ")))),
        _ => return Err(SendError::Bad("send a non-empty text or a key".into())),
    }
    // Let the TUI draw what was sent before reading it back.
    tokio::time::sleep(StdDuration::from_millis(400)).await;
    chat(hub, cfg, Some(&name)).await.map_err(SendError::Failed)
}

type ApiResult<T> = std::result::Result<Json<T>, (StatusCode, String)>;

#[derive(Deserialize)]
pub struct ChatQuery {
    session: Option<String>,
}

fn lane(hub: &Hub, name: &str) -> Result<LaneConfig, (StatusCode, String)> {
    hub.lanes.config(name).ok_or((StatusCode::NOT_FOUND, "lane not found".into()))
}

/// `GET /api/lanes/{name}/chat?session=`.
pub async fn get_api(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Query(q): Query<ChatQuery>) -> ApiResult<LaneChat> {
    let cfg = lane(&hub, &name)?;
    chat(&hub, &cfg, q.session.as_deref()).await.map(Json).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")))
}

/// `POST /api/lanes/{name}/chat` with `{text}` or `{key}` (and `session`).
pub async fn post_api(State(hub): State<Arc<Hub>>, UrlPath(name): UrlPath<String>, Json(body): Json<ChatSend>) -> ApiResult<LaneChat> {
    let cfg = lane(&hub, &name)?;
    send(&hub, &cfg, &body).await.map(Json).map_err(|e| match e {
        SendError::Bad(m) => (StatusCode::BAD_REQUEST, m),
        SendError::Conflict(m) => (StatusCode::CONFLICT, m),
        SendError::Failed(err) => (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(v: Value) -> String {
        v.to_string()
    }

    #[test]
    fn the_chat_is_the_owners_words_the_agents_replies_and_one_line_per_tool() {
        let t = [
            line(serde_json::json!({"type":"agent-name","agentName":"developer-x"})),
            line(serde_json::json!({"type":"user","uuid":"u1","timestamp":"2026-11-09T08:00:00Z","message":{"content":"fix the branch filter"}})),
            line(serde_json::json!({"type":"assistant","uuid":"a1","timestamp":"2026-11-09T08:00:05Z","message":{"content":[
                {"type":"thinking","thinking":"hmm"},
                {"type":"text","text":"Looking at it."},
                {"type":"tool_use","name":"Bash","input":{"command":"grep -rn branch_id src/\n  | head","description":"Find the filter"}}]}})),
            line(serde_json::json!({"type":"user","uuid":"u2","message":{"content":[{"type":"tool_result","content":"src/a.rs:1"}]}})),
            line(serde_json::json!({"type":"user","uuid":"u3","message":{"content":"<command-name>/clear</command-name>\n<command-args></command-args>"}})),
            line(serde_json::json!({"type":"user","uuid":"u4","isMeta":true,"message":{"content":"a reminder"}})),
            line(serde_json::json!({"type":"assistant","uuid":"s1","isSidechain":true,"message":{"content":[{"type":"text","text":"sub-agent"}]}})),
            line(serde_json::json!({"type":"user","uuid":"u5","message":{"content":"<pasted_content id=\"1\">the log</pasted_content> read this"}})),
            "{partial".into(),
        ]
        .join("\n");
        let m = messages(&t);
        let got: Vec<(&str, &str)> = m.iter().map(|m| (m.role.as_str(), m.text.as_str())).collect();
        assert_eq!(got, vec![("owner", "fix the branch filter"), ("agent", "Looking at it."), ("tool", "Bash Find the filter"), ("owner", "/clear"), ("owner", "the log read this")]);
        assert_eq!(m[2].id, "a1#2");
        assert!(m[0].at.is_some());
    }

    #[test]
    fn a_secret_is_masked_and_a_long_reply_cut() {
        let t = line(serde_json::json!({"type":"assistant","uuid":"a","message":{"content":[{"type":"text","text":format!("token ghp_{} {}", "a".repeat(36), "word ".repeat(2000))}]}}));
        let m = messages(&t);
        assert!(!m[0].text.contains(&"a".repeat(36)), "{}", &m[0].text[..80]);
        assert!(m[0].text.chars().count() <= TEXT_CHARS + 1 && m[0].text.ends_with('…'));
    }

    #[test]
    fn the_transcript_is_the_newest_one_named_for_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let write = |f: &str, name: &str| std::fs::write(dir.path().join(f), format!("{}\n", line(serde_json::json!({"type":"agent-name","agentName":name})))).unwrap();
        write("old.jsonl", "developer-x");
        std::thread::sleep(StdDuration::from_millis(20));
        write("other.jsonl", "manager-x");
        assert_eq!(transcript_in(dir.path(), "developer-x").unwrap().file_name().unwrap(), "old.jsonl");
        assert_eq!(transcript_in(dir.path(), "manager-x").unwrap().file_name().unwrap(), "other.jsonl");
        assert!(transcript_in(dir.path(), "nobody").is_none());
    }
}
