//! Past sessions in a project's knowledge (Hermes Agent's session search): what the owner and the agents said in the
//! Claude Code sessions opened in the project's folder (its manager, its lanes), indexed as MAP memory
//! (`map://sessions/<id>`) so `knowledge/search` finds "what did we decide about X". Only the words are kept: the
//! owner's messages and the agent's replies, never tool calls or their output, each message cut and masked
//! (`trace::SECRETS`), and the newest part of a long session. Agents reach it only with RAG on, like all knowledge.
//!
//! Transcripts are read from the owner's Claude folder (`[skills] claude_transcripts`) and MAP's isolated Claude home;
//! a file is parsed again only when it changed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::SystemTime;

use chrono::{DateTime, Duration, Utc};
use serde_json::Value;

use crate::hub::Hub;

/// Sessions indexed per project: the newest, changed within [`DAYS`].
const SESSIONS: usize = 40;
const DAYS: i64 = 30;
const MESSAGE_CHARS: usize = 600;
/// A session's text kept, the newest part.
const SESSION_CHARS: usize = 40_000;

/// A session as a memory document: path, title, body, last message.
pub type SessionDoc = (String, String, String, DateTime<Utc>);

type Cached = (SystemTime, u64, Option<SessionDoc>);
static CACHE: LazyLock<Mutex<HashMap<PathBuf, Cached>>> = LazyLock::new(Default::default);

fn masked(text: &str) -> String {
    let mut out: String = text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MESSAGE_CHARS).collect();
    for (re, with) in crate::trace::SECRETS.iter() {
        out = re.replace_all(&out, *with).into_owned();
    }
    out
}

/// The words of one message: a person's text, or an agent's text blocks; `None` for tool traffic and MAP's own notes.
fn words(entry: &Value) -> Option<(&'static str, String)> {
    if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) || entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let role = match entry.get("type").and_then(Value::as_str)? {
        "user" => "owner",
        "assistant" => "agent",
        _ => return None,
    };
    let text = match entry.pointer("/message/content")? {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().filter(|i| i.get("type").and_then(Value::as_str) == Some("text")).filter_map(|i| i.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join("\n"),
        _ => return None,
    };
    let t = text.trim();
    (!t.is_empty() && !t.starts_with('<') && !t.starts_with("[Request interrupted")).then(|| (role, masked(t)))
}

/// One transcript as a document; `None` when it holds no words.
pub fn parse(reader: impl BufRead, id: &str) -> Option<SessionDoc> {
    let (mut name, mut lines, mut first, mut last) = (None::<String>, Vec::<String>::new(), None, None);
    for line in reader.lines().map_while(Result::ok) {
        // Tool output is most of a transcript: skip it before parsing.
        if line.contains("\"tool_result\"") && !line.contains("\"text\"") {
            continue;
        }
        let Ok(entry) = serde_json::from_str::<Value>(&line) else { continue };
        if entry.get("type").and_then(Value::as_str) == Some("agent-name") {
            name = entry.get("agentName").and_then(Value::as_str).map(String::from);
            continue;
        }
        let Some((who, text)) = words(&entry) else { continue };
        let at = entry.get("timestamp").and_then(Value::as_str).and_then(|t| DateTime::parse_from_rfc3339(t).ok()).map(|t| t.with_timezone(&Utc));
        if let Some(at) = at {
            first.get_or_insert(at);
            last = Some(at);
        }
        let when = at.map(|a| a.format(" %Y-%m-%d %H:%M").to_string()).unwrap_or_default();
        lines.push(format!("**{who}**{when}: {text}"));
    }
    let last = last?;
    let mut size = 0;
    let keep = lines.iter().rev().take_while(|l| {
        size += l.chars().count() + 1;
        size <= SESSION_CHARS
    });
    let mut kept: Vec<&String> = keep.collect();
    kept.reverse();
    let who = name.unwrap_or_else(|| id.chars().take(8).collect());
    let span = format!("{} – {}", first.unwrap_or(last).format("%Y-%m-%d"), last.format("%Y-%m-%d"));
    let title = format!("Session {who} · {span}");
    let body = format!("# {title}\n\nWhat the owner and the agent said in session `{who}` (tool calls left out).\n\n{}\n", kept.iter().map(|l| l.as_str()).collect::<Vec<_>>().join("\n\n"));
    Some((format!("map://sessions/{id}"), title, body, last))
}

fn read(file: &Path) -> Option<SessionDoc> {
    let meta = std::fs::metadata(file).ok()?;
    let (mtime, len) = (meta.modified().ok()?, meta.len());
    if let Some((m, l, doc)) = CACHE.lock().unwrap().get(file)
        && (*m, *l) == (mtime, len)
    {
        return doc.clone();
    }
    let id = file.file_stem()?.to_string_lossy().to_string();
    let doc = std::fs::File::open(file).ok().and_then(|f| parse(BufReader::new(f), &id));
    CACHE.lock().unwrap().insert(file.to_path_buf(), (mtime, len, doc.clone()));
    doc
}

/// The newest transcripts of sessions opened in `workspace`, under each of `roots`.
fn files(roots: &[PathBuf], workspace: &Path) -> Vec<PathBuf> {
    let since = SystemTime::now() - std::time::Duration::from_secs(DAYS as u64 * 86_400);
    let mut found: Vec<(SystemTime, PathBuf)> = roots
        .iter()
        .filter_map(|r| crate::skills::transcript_dir(r, workspace))
        .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .filter(|(m, _)| *m >= since)
        .collect();
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    found.into_iter().take(SESSIONS).map(|(_, p)| p).collect()
}

/// The project's past sessions as memory documents.
pub async fn docs(hub: &Hub, slug: &str) -> Vec<SessionDoc> {
    let Ok(Some(project)) = hub.store.project(slug).await else { return vec![] };
    let workspace = hub.workspace_of(&project);
    let mut roots = vec![hub.config.skills.claude_transcripts.clone()];
    if hub.config.agents_home.isolated() {
        roots.push(hub.config.agents_home.dir().join("claude/projects"));
    }
    let cutoff = Utc::now() - Duration::days(DAYS);
    tokio::task::spawn_blocking(move || files(&roots, &workspace).iter().filter_map(|f| read(f)).filter(|d| d.3 >= cutoff).collect()).await.unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_words_and_drops_tools_and_secrets() {
        let jsonl = [
            r#"{"type":"agent-name","agentName":"manager-shop"}"#,
            r#"{"type":"user","message":{"content":"<command-name>/clear</command-name>"},"timestamp":"2026-10-28T09:00:00Z"}"#,
            r#"{"type":"user","message":{"content":"We keep refunds off. token sk-abcdefghijklmnopqrstuvwx"},"timestamp":"2026-10-28T09:01:00Z"}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}},{"type":"text","text":"Noted: refunds stay off."}]},"timestamp":"2026-10-28T09:02:00Z"}"#,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","content":"secret file list"}]},"timestamp":"2026-10-28T09:03:00Z"}"#,
            r#"{"type":"assistant","isSidechain":true,"message":{"content":[{"type":"text","text":"subagent chatter"}]},"timestamp":"2026-10-28T09:04:00Z"}"#,
        ]
        .join("\n");
        let (path, title, body, at) = parse(jsonl.as_bytes(), "abc123").unwrap();
        assert_eq!(path, "map://sessions/abc123");
        assert_eq!(title, "Session manager-shop · 2026-10-28 – 2026-10-28");
        assert!(body.contains("**owner** 2026-10-28 09:01: We keep refunds off. token ***"), "{body}");
        assert!(body.contains("**agent** 2026-10-28 09:02: Noted: refunds stay off."));
        assert!(!body.contains("clear") && !body.contains("secret file list") && !body.contains("subagent") && !body.contains("sk-abc"));
        assert_eq!(at.to_rfc3339(), "2026-10-28T09:02:00+00:00");
        assert!(parse(r#"{"type":"mode"}"#.as_bytes(), "x").is_none());
    }

    #[test]
    fn a_long_session_keeps_its_newest_part() {
        let line = |i: usize| format!(r#"{{"type":"user","message":{{"content":"message {i} {}"}},"timestamp":"2026-10-28T09:00:00Z"}}"#, "word ".repeat(110));
        let jsonl: Vec<String> = (0..200).map(line).collect();
        let (_, _, body, _) = parse(jsonl.join("\n").as_bytes(), "id").unwrap();
        assert!(body.contains("message 199") && !body.contains("message 0 "), "newest kept");
        assert!(body.chars().count() < SESSION_CHARS + 500);
    }
}
