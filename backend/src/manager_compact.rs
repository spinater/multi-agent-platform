//! Compressing a project's manager (Hermes Agent's `/compress`). The manager runs for days and every turn sends its
//! whole conversation again, so MAP watches its context: the tokens of the last reply in the manager's Claude Code
//! transcript (input plus cache reads and writes). Above [`LIMIT`] (or on the owner's Compress) MAP types `/compact`
//! into it, which has the CLI summarise the conversation and carry on from the summary. Only while the manager is
//! idle, nobody is attached and the provider has `/compact` (Claude Code, Codex).
//!
//! `GET /projects/{slug}/manager/compact` reports the context; `POST` compresses now.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use serde::Serialize;
use serde_json::Value;
use ts_rs::TS;

use crate::hub::Hub;
use crate::model::{Level, SessionState};

/// Context tokens past which the manager is compressed unasked.
pub const LIMIT: u64 = 120_000;
/// The tail of a transcript read for its last reply.
const TAIL: u64 = 4 << 20;
const PROVIDERS: [&str; 2] = ["claude", "codex"];

/// The manager's context as the Manager panel shows it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ManagerContextInfo {
    /// Tokens of the last reply's context; `None` when no transcript was found (not Claude, or not started).
    #[ts(type = "number | null")]
    pub tokens: Option<u64>,
    #[ts(type = "number")]
    pub limit: u64,
    /// Why Compress cannot run now; `None` when it can.
    pub blocked: Option<String>,
}

/// The context tokens of the last assistant reply in a transcript's tail.
pub fn last_context(tail: &str) -> Option<u64> {
    tail.lines().rev().filter(|l| l.contains("\"usage\"")).find_map(|l| {
        let v: Value = serde_json::from_str(l).ok()?;
        let u = v.pointer("/message/usage")?;
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        Some(n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"))
    })
}

/// The newest transcript in `dir` whose session is named `name` (MAP starts the manager with `--name`).
fn transcript(dir: &Path, name: &str) -> Option<PathBuf> {
    let needle = format!("\"agentName\":\"{name}\"");
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "jsonl"))
        .filter_map(|p| Some((std::fs::metadata(&p).ok()?.modified().ok()?, p)))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    files.into_iter().map(|f| f.1).take(20).find(|p| {
        let mut head = String::new();
        std::fs::File::open(p).ok().and_then(|f| f.take(64 << 10).read_to_string(&mut head).ok()).is_some() && head.contains(&needle)
    })
}

fn tail(file: &Path) -> Option<String> {
    let mut f = std::fs::File::open(file).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(TAIL))).ok()?;
    let mut bytes = vec![];
    f.read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// The manager's context tokens, from its transcript.
pub async fn tokens(hub: &Hub, slug: &str) -> Option<u64> {
    let project = hub.store.project(slug).await.ok().flatten()?;
    let workspace = hub.workspace_of(&project);
    // `claude_transcripts` follows the folder MAP's Claude really uses (its isolated home in isolated mode).
    let roots = [hub.config.skills.claude_transcripts.clone()];
    let name = crate::manager::session_name(hub, slug);
    tokio::task::spawn_blocking(move || {
        roots.iter().filter_map(|r| crate::skills::transcript_dir(r, &workspace)).find_map(|d| transcript(&d, &name)).and_then(|f| tail(&f)).and_then(|t| last_context(&t))
    })
    .await
    .ok()
    .flatten()
}

/// Why the manager cannot be compressed now.
async fn blocked(hub: &Hub, slug: &str) -> Option<String> {
    let name = crate::manager::session_name(hub, slug);
    let Ok(session) = hub.managed(&name).await else { return Some("the manager is not running".into()) };
    if !PROVIDERS.contains(&session.provider.as_str()) {
        return Some(format!("`{}` has no /compact", session.provider));
    }
    if session.attached {
        return Some("someone is attached to the manager".into());
    }
    (session.state != SessionState::Idle).then(|| "the manager is busy; try when it is idle".into())
}

pub async fn info(hub: &Hub, slug: &str) -> ManagerContextInfo {
    ManagerContextInfo { tokens: tokens(hub, slug).await, limit: LIMIT, blocked: blocked(hub, slug).await }
}

/// Types `/compact` into the manager.
pub async fn compact(hub: &Hub, slug: &str, why: &str) -> Result<()> {
    if let Some(why) = blocked(hub, slug).await {
        bail!("{why}");
    }
    let name = crate::manager::session_name(hub, slug);
    hub.send_prompt(&name, "/compact").await?;
    hub.event(Level::Info, "manager.compact", Some(&name), None, format!("{slug}: manager compressed ({why})")).await;
    Ok(())
}

/// Compresses each running manager past [`LIMIT`], checked every few minutes.
pub async fn run(hub: Arc<Hub>) {
    loop {
        tokio::time::sleep(Duration::from_secs(300)).await;
        for p in hub.store.projects().await.unwrap_or_default().into_iter().filter(|p| p.settings.manager.enabled) {
            if let Some(t) = tokens(&hub, &p.slug).await.filter(|t| *t > LIMIT)
                && blocked(&hub, &p.slug).await.is_none()
                && let Err(err) = compact(&hub, &p.slug, &format!("context {t} tokens, over {LIMIT}")).await
            {
                tracing::warn!("{}: manager not compressed: {err:#}", p.slug);
            }
        }
    }
}

type ApiResult<T> = std::result::Result<Json<T>, (StatusCode, String)>;

pub async fn get_api(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<ManagerContextInfo> {
    Ok(Json(info(&hub, &slug).await))
}

pub async fn post_api(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<ManagerContextInfo> {
    compact(&hub, &slug, "the owner pressed Compress").await.map_err(|e| (StatusCode::CONFLICT, format!("{e:#}")))?;
    Ok(Json(info(&hub, &slug).await))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_last_reply_context_and_finds_the_manager_transcript() {
        let tail = [
            r#"{"type":"assistant","message":{"usage":{"input_tokens":10,"cache_read_input_tokens":1000,"cache_creation_input_tokens":5}}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":3,"cache_read_input_tokens":90000,"cache_creation_input_tokens":200}}}"#,
            r#"{"type":"user","message":{"content":"hi"}}"#,
        ]
        .join("\n");
        assert_eq!(last_context(&tail), Some(90203));
        assert_eq!(last_context("no usage here"), None);
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jsonl"), r#"{"type":"agent-name","agentName":"map-lane-x"}"#).unwrap();
        std::fs::write(dir.path().join("b.jsonl"), r#"{"type":"agent-name","agentName":"map-mgr-shop"}"#).unwrap();
        assert_eq!(transcript(dir.path(), "map-mgr-shop").unwrap().file_name().unwrap(), "b.jsonl");
        assert!(transcript(dir.path(), "map-mgr-other").is_none());
    }
}
