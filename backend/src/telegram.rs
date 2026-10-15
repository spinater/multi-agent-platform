//! Telegram: the hourly lane report, immediate notices (a card finished, a lane stuck, cleared or prompted, a MAP
//! task done, failed or held, a plan waiting for approval), and the `/watch` and `/status` commands over a
//! getUpdates long-poll. Ported from agent-watcher (`tg_send`, `split_chunks`, `commands.py`, `run.sh`).
//!
//! The token is read from the environment (`token_env`, else that line of `env_file`) when needed and never stored,
//! logged or returned: every error is passed through [`redact`] first, because reqwest's errors carry the URL and
//! Telegram's URL carries the token.

use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use chrono::{DateTime, Local, Timelike, Utc};
use regex::Regex;
use serde_json::{Value, json};

use crate::config::{Config, TelegramConfig};
use crate::hub::Hub;
use crate::model::{Event, Level, PlanStatus, ServerMessage, TaskStatus, TelegramInfo};

/// Telegram's limit is 4096; leave room.
pub const CHUNK: usize = 3900;

const OFFSET_KEY: &str = "telegram.offset";
const CHAT_KEY: &str = "telegram.chat_id";
const USERS_KEY: &str = "telegram.allowed_user_ids";
const REPORT_KEY: &str = "telegram.last_report_at";

/// How long to wait between tries; tests shorten it.
#[derive(Debug, Clone)]
pub struct Timing {
    /// sendMessage retry n waits `n * retry_ms`.
    pub retry_ms: u64,
    /// After a failed getUpdates.
    pub error_backoff_ms: u64,
    /// After a 409 (another consumer of the token), doubling up to `conflict_max_ms`.
    pub conflict_backoff_ms: u64,
    pub conflict_max_ms: u64,
    /// getUpdates `timeout` (long-poll seconds).
    pub poll_secs: u64,
    /// How often the reporter looks at the clock.
    pub report_check_ms: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self { retry_ms: 2000, error_backoff_ms: 5000, conflict_backoff_ms: 30_000, conflict_max_ms: 600_000, poll_secs: 25, report_check_ms: 20_000 }
    }
}

#[derive(Debug, Default)]
struct State {
    last_send_ok: Option<bool>,
    last_error: Option<String>,
    last_sent_at: Option<DateTime<Utc>>,
    last_report_at: Option<DateTime<Utc>>,
    polling: bool,
}

/// A failed Bot API call; `message` is already redacted.
#[derive(Debug, Clone)]
pub struct TgError {
    pub code: Option<u16>,
    pub message: String,
}

impl std::fmt::Display for TgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

pub struct Telegram {
    cfg: TelegramConfig,
    chat_id: Mutex<i64>,
    allowed: Mutex<Vec<i64>>,
    state: Mutex<State>,
    pub timing: Mutex<Timing>,
    http: reqwest::Client,
}

static BOT_URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"bot\d+:[A-Za-z0-9_-]+").unwrap());
static TOKEN_SHAPE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{5,}:[A-Za-z0-9_-]{20,}").unwrap());

/// `text` with the token, and anything shaped like a bot token, removed.
pub fn redact(text: &str, token: Option<&str>) -> String {
    let mut out = text.to_string();
    if let Some(t) = token.filter(|t| !t.is_empty()) {
        out = out.replace(t, "<token>");
    }
    let out = BOT_URL.replace_all(&out, "bot<token>");
    TOKEN_SHAPE.replace_all(&out, "<token>").into_owned()
}

/// Splits on line breaks into pieces of at most `limit` characters (a longer line is cut).
pub fn split_chunks(text: &str, limit: usize) -> Vec<String> {
    if text.chars().count() <= limit {
        return vec![text.to_string()];
    }
    let mut chunks = vec![];
    let mut cur = String::new();
    let mut cur_len = 0;
    for line in text.split('\n') {
        let mut line: Vec<char> = line.chars().collect();
        if line.len() > limit && cur_len > 0 {
            // Keep the order (agent-watcher pushed the long line's pieces before what came above it).
            chunks.push(cur.trim_end_matches('\n').to_string());
            cur.clear();
            cur_len = 0;
        }
        while line.len() > limit {
            chunks.push(line[..limit].iter().collect());
            line.drain(..limit);
        }
        if cur_len + line.len() + 1 > limit {
            chunks.push(cur.trim_end_matches('\n').to_string());
            cur.clear();
            cur_len = 0;
        }
        cur.extend(line.iter());
        cur.push('\n');
        cur_len += line.len() + 1;
    }
    if !cur.trim().is_empty() {
        chunks.push(cur.trim_end_matches('\n').to_string());
    }
    chunks
}

/// The `VAR=value` line of an env file (`export VAR=…`, quotes stripped).
pub fn read_env_file(path: &Path, var: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line).trim();
        let v = line.strip_prefix(var)?.strip_prefix('=')?.trim();
        let v = v.trim_matches('"').trim_matches('\'').trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

/// Whether the hourly report is due: on `minute` (once), or whenever 70 minutes passed without one.
pub fn report_due(now: DateTime<Local>, last: Option<DateTime<Utc>>, minute: u32) -> bool {
    let Some(last) = last else { return true };
    let since = now.with_timezone(&Utc) - last;
    since >= chrono::Duration::minutes(70) || (now.minute() == minute && since >= chrono::Duration::minutes(50))
}

/// Commands are obeyed from `allowed` users; with none listed, from anyone in the configured chat.
pub fn allowed(allowed: &[i64], chat_id: i64, from: Option<i64>, chat: Option<i64>) -> bool {
    if !allowed.is_empty() {
        return from.is_some_and(|f| allowed.contains(&f));
    }
    chat_id == 0 || chat == Some(chat_id)
}

impl Telegram {
    pub fn new(config: &Config) -> Self {
        let cfg = config.telegram.clone();
        Self {
            chat_id: Mutex::new(cfg.chat_id),
            allowed: Mutex::new(cfg.allowed_user_ids.clone()),
            cfg,
            state: Mutex::default(),
            timing: Mutex::default(),
            http: reqwest::Client::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.cfg.enabled
    }

    /// The chat, users and last report time the dashboard (or a watcher import) set.
    pub async fn load(&self, store: &crate::store::Store) {
        if let Ok(Some(v)) = store.runtime_setting(CHAT_KEY).await
            && let Ok(id) = v.parse()
        {
            *self.chat_id.lock().unwrap() = id;
        }
        if let Ok(Some(v)) = store.runtime_setting(USERS_KEY).await
            && let Ok(ids) = serde_json::from_str(&v)
        {
            *self.allowed.lock().unwrap() = ids;
        }
        if let Ok(Some(v)) = store.runtime_setting(REPORT_KEY).await
            && let Ok(at) = DateTime::parse_from_rfc3339(&v)
        {
            self.state.lock().unwrap().last_report_at = Some(at.with_timezone(&Utc));
        }
    }

    pub async fn set_chat(&self, hub: &Hub, chat_id: Option<i64>, allowed: Option<Vec<i64>>) -> anyhow::Result<()> {
        if let Some(id) = chat_id {
            hub.store.set_runtime_setting(CHAT_KEY, Some(&id.to_string())).await?;
            *self.chat_id.lock().unwrap() = id;
        }
        if let Some(ids) = allowed {
            hub.store.set_runtime_setting(USERS_KEY, Some(&serde_json::to_string(&ids)?)).await?;
            *self.allowed.lock().unwrap() = ids;
        }
        Ok(())
    }

    pub fn chat_id(&self) -> i64 {
        *self.chat_id.lock().unwrap()
    }

    /// From the environment, else `env_file`. Never stored.
    fn token(&self) -> Option<String> {
        if let Ok(v) = std::env::var(&self.cfg.token_env)
            && !v.trim().is_empty()
        {
            return Some(v.trim().to_string());
        }
        read_env_file(self.cfg.env_file.as_deref()?, &self.cfg.token_env)
    }

    pub fn info(&self) -> TelegramInfo {
        let s = self.state.lock().unwrap();
        TelegramInfo {
            enabled: self.cfg.enabled,
            token_set: self.token().is_some(),
            token_env: self.cfg.token_env.clone(),
            env_file: self.cfg.env_file.as_ref().map(|p| p.display().to_string()),
            chat_id: self.chat_id(),
            allowed_user_ids: self.allowed.lock().unwrap().clone(),
            commands: self.cfg.commands,
            polling: s.polling,
            last_send_ok: s.last_send_ok,
            last_error: s.last_error.clone(),
            last_sent_at: s.last_sent_at,
            last_report_at: s.last_report_at,
            report_minute: self.cfg.report_minute,
        }
    }

    async fn call(&self, token: &str, method: &str, payload: &Value, timeout: Duration) -> Result<Value, TgError> {
        let url = format!("{}/bot{token}/{method}", self.cfg.api_base);
        let resp = self.http.post(&url).json(payload).timeout(timeout).send().await.map_err(|e| TgError {
            code: e.status().map(|s| s.as_u16()),
            message: redact(&format!("{method}: {}", e.without_url()), Some(token)),
        })?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if status.is_success() && body.get("ok").and_then(Value::as_bool) == Some(true) {
            return Ok(body.get("result").cloned().unwrap_or(Value::Null));
        }
        let code = body.get("error_code").and_then(Value::as_u64).map(|c| c as u16).or(Some(status.as_u16()));
        let desc = body.get("description").and_then(Value::as_str).unwrap_or(status.canonical_reason().unwrap_or("error"));
        Err(TgError { code, message: redact(&format!("{method}: {desc}"), Some(token)) })
    }

    fn record(&self, result: &Result<(), String>) {
        let mut s = self.state.lock().unwrap();
        s.last_send_ok = Some(result.is_ok());
        match result {
            Ok(()) => s.last_sent_at = Some(Utc::now()),
            Err(e) => s.last_error = Some(e.clone()),
        }
    }

    /// Plain text to `chat`, in pieces of at most [`CHUNK`] characters, each tried three times.
    pub async fn send_to(&self, chat: i64, text: &str) -> Result<(), String> {
        let result = self.send_inner(chat, text, None).await;
        self.record(&result);
        result
    }

    async fn send_inner(&self, chat: i64, text: &str, markup: Option<Value>) -> Result<(), String> {
        if !self.cfg.enabled {
            return Err("telegram is not enabled in map.toml".into());
        }
        let token = self.token().ok_or_else(|| format!("no token: set {} (or its line in telegram.env_file)", self.cfg.token_env))?;
        if chat == 0 {
            return Err("telegram.chat_id is not set".into());
        }
        let retry = self.timing.lock().unwrap().retry_ms;
        for chunk in split_chunks(text, CHUNK) {
            let mut payload = json!({ "chat_id": chat, "text": chunk, "disable_web_page_preview": true });
            if let Some(m) = &markup {
                payload["reply_markup"] = m.clone();
            }
            let mut last = String::new();
            let mut sent = false;
            for attempt in 0..3u64 {
                match self.call(&token, "sendMessage", &payload, Duration::from_secs(30)).await {
                    Ok(_) => {
                        sent = true;
                        break;
                    }
                    Err(e) => {
                        tracing::warn!("telegram send failed (attempt {}): {e}", attempt + 1);
                        last = e.message;
                        if attempt < 2 {
                            tokio::time::sleep(Duration::from_millis(retry * (attempt + 1))).await;
                        }
                    }
                }
            }
            if !sent {
                return Err(last);
            }
        }
        Ok(())
    }

    pub async fn send(&self, text: &str) -> Result<(), String> {
        self.send_to(self.chat_id(), text).await
    }

    fn set_polling(&self, on: bool) {
        self.state.lock().unwrap().polling = on;
    }
}

/// The report `/status` and the hourly round send: every lane, then one line about MAP's own tasks and plans.
pub async fn full_report(hub: &Hub) -> String {
    let mut text = hub.lanes.report(Utc::now());
    let tasks = hub.store.tasks().await.unwrap_or_default();
    let running = tasks.iter().filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Verifying | TaskStatus::Auditing | TaskStatus::Landing)).count();
    let pending = tasks.iter().filter(|t| t.status == TaskStatus::Pending).count();
    let review = tasks.iter().filter(|t| t.status == TaskStatus::Review).count();
    let drafts = hub.store.plans(None, 100).await.unwrap_or_default().iter().filter(|p| p.status == PlanStatus::Draft).count();
    if running + pending + review + drafts > 0 {
        text.push_str(&format!("\n\n🧭 MAP: running {running} · pending {pending} · review {review} · plans waiting {drafts}"));
    }
    text
}

/// The message for an event worth a notice, or `None`. A muted lane's events are dropped.
pub fn notice(hub: &Hub, e: &Event) -> Option<String> {
    match e.kind.as_str() {
        "lane.clear" | "lane.ask" | "lane.resume" | "lane.stall" | "lane.card_done" | "lane.rate_limited" => {
            let lane = e.session.as_deref()?;
            hub.lanes.config(lane).is_none_or(|c| c.watch).then(|| e.message.clone())
        }
        "task.done" => Some(format!("✅ MAP · {}", e.message)),
        "task.failed" => Some(format!("❌ MAP · task #{} failed: {}", e.task_id.unwrap_or(0), e.message.trim_start_matches("Failed: "))),
        "task.held" => Some(format!("✋ MAP · task #{} held by intake — {}", e.task_id.unwrap_or(0), e.message)),
        "plan.created" => Some(format!("📝 MAP · {}", e.message)),
        _ => None,
    }
}

pub async fn run(hub: Arc<Hub>) {
    if !hub.telegram.enabled() {
        return;
    }
    tokio::spawn(notifier(hub.clone()));
    tokio::spawn(reporter(hub.clone()));
    if hub.telegram.cfg.commands {
        tokio::spawn(listen(hub.clone()));
    }
}

async fn notifier(hub: Arc<Hub>) {
    let mut rx = hub.subscribe();
    loop {
        match rx.recv().await {
            Ok(ServerMessage::Event { event }) => {
                if let Some(text) = notice(&hub, &event)
                    && let Err(err) = hub.telegram.send(&text).await
                {
                    tracing::warn!("telegram notice not sent: {err}");
                }
            }
            Ok(_) => {}
            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => tracing::warn!("telegram notifier skipped {n} messages"),
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
        }
    }
}

async fn reporter(hub: Arc<Hub>) {
    loop {
        let wait = hub.telegram.timing.lock().unwrap().report_check_ms;
        tokio::time::sleep(Duration::from_millis(wait)).await;
        report_if_due(&hub, Local::now()).await;
    }
}

/// Sends the full report when [`report_due`]; lanes must have been read once.
pub async fn report_if_due(hub: &Hub, now: DateTime<Local>) -> bool {
    let tg = &hub.telegram;
    if hub.lanes.is_empty() || !hub.lanes.ticked() {
        return false;
    }
    let last = tg.state.lock().unwrap().last_report_at;
    if !report_due(now, last, tg.cfg.report_minute) {
        return false;
    }
    let at = now.with_timezone(&Utc);
    // Marked before sending: a failing send waits for the next window instead of retrying every check.
    tg.state.lock().unwrap().last_report_at = Some(at);
    let _ = hub.store.set_runtime_setting(REPORT_KEY, Some(&at.to_rfc3339())).await;
    let text = full_report(hub).await;
    if let Err(err) = tg.send(&text).await {
        tracing::warn!("telegram report not sent: {err}");
    }
    true
}

fn menu(hub: &Hub) -> (String, Value) {
    let lanes = hub.lanes.configs();
    let mut text = vec!["📋 watched lanes — tap to toggle".to_string()];
    let mut rows = vec![];
    for c in &lanes {
        text.push(format!("{} · {}", if c.watch { "✅ watching" } else { "⏸ off" }, c.name));
        rows.push(json!([{ "text": format!("{} {}", if c.watch { "✅" } else { "⏸" }, c.name), "callback_data": format!("t:{}", c.name) }]));
    }
    if lanes.is_empty() {
        text.push("(no lanes in map.toml)".into());
    }
    (text.join("\n"), json!({ "inline_keyboard": rows }))
}

/// One update: a command message or a button press. Errors are for this update only.
pub async fn handle_update(hub: &Hub, token: &str, upd: &Value) -> Result<(), TgError> {
    let tg = &hub.telegram;
    let msg = upd.get("message");
    let cb = upd.get("callback_query");
    let from = msg.or(cb).and_then(|m| m.pointer("/from/id")).and_then(Value::as_i64);
    let chat = msg.and_then(|m| m.pointer("/chat/id")).or_else(|| cb.and_then(|c| c.pointer("/message/chat/id"))).and_then(Value::as_i64);
    let allowed_ids = tg.allowed.lock().unwrap().clone();
    if !allowed(&allowed_ids, tg.chat_id(), from, chat) {
        return Ok(()); // silently: others in the chat should not learn there is anything to try
    }
    let short = Duration::from_secs(20);
    if let Some(m) = msg {
        let text = m.get("text").and_then(Value::as_str).unwrap_or("").trim();
        let cmd = text.split_whitespace().next().unwrap_or("").split('@').next().unwrap_or("");
        let Some(chat) = chat else { return Ok(()) };
        match cmd {
            "/watch" | "/start" | "/menu" => {
                let (text, keyboard) = menu(hub);
                tg.call(token, "sendMessage", &json!({ "chat_id": chat, "text": text, "reply_markup": keyboard }), short).await?;
            }
            "/status" => {
                let report = full_report(hub).await;
                if let Err(e) = tg.send_to(chat, &report).await {
                    return Err(TgError { code: None, message: e });
                }
            }
            _ => {}
        }
        return Ok(());
    }
    if let Some(cb) = cb {
        let id = cb.get("id").and_then(Value::as_str).unwrap_or("");
        let data = cb.get("data").and_then(Value::as_str).unwrap_or("");
        let Some(lane) = data.strip_prefix("t:").or_else(|| data.strip_prefix("toggle:")) else { return Ok(()) };
        let Some(cfg) = hub.lanes.config(lane) else {
            tg.call(token, "answerCallbackQuery", &json!({ "callback_query_id": id, "text": "unknown lane" }), short).await?;
            return Ok(());
        };
        let on = !cfg.watch;
        if let Err(err) = hub.lanes.update(hub, lane, crate::lanes::LanePatch { watch: Some(on), ..Default::default() }).await {
            return Err(TgError { code: None, message: format!("{err:#}") });
        }
        tg.call(token, "answerCallbackQuery", &json!({ "callback_query_id": id, "text": if on { "watching on ✅" } else { "watching off ⏸" } }), short).await?;
        if let (Some(chat), Some(mid)) = (chat, cb.pointer("/message/message_id").and_then(Value::as_i64)) {
            let (text, keyboard) = menu(hub);
            tg.call(token, "editMessageText", &json!({ "chat_id": chat, "message_id": mid, "text": text, "reply_markup": keyboard }), short).await?;
        }
    }
    Ok(())
}

/// One getUpdates round from the persisted offset; each update moves the offset on before it is handled, so a
/// failing update is never handled twice.
pub async fn poll_once(hub: &Hub, token: &str) -> Result<usize, TgError> {
    let tg = &hub.telegram;
    let offset: i64 = hub.store.runtime_setting(OFFSET_KEY).await.ok().flatten().and_then(|v| v.parse().ok()).unwrap_or(0);
    let poll = tg.timing.lock().unwrap().poll_secs;
    let payload = json!({ "offset": offset, "timeout": poll, "allowed_updates": ["message", "callback_query"] });
    let result = tg.call(token, "getUpdates", &payload, Duration::from_secs(poll + 10)).await?;
    let updates = result.as_array().cloned().unwrap_or_default();
    for upd in &updates {
        if let Some(id) = upd.get("update_id").and_then(Value::as_i64) {
            let _ = hub.store.set_runtime_setting(OFFSET_KEY, Some(&(id + 1).to_string())).await;
        }
        if let Err(err) = handle_update(hub, token, upd).await {
            tracing::warn!("telegram update skipped: {err}");
        }
    }
    Ok(updates.len())
}

/// The command listener: `setMyCommands` and a keyboard reset once, then getUpdates for ever. A 409 means another
/// process polls this token (Telegram allows one): warn once, back off, never spin.
pub async fn listen(hub: Arc<Hub>) {
    let tg = &hub.telegram;
    let mut announced = false;
    let mut conflict = false;
    let mut backoff = 0u64;
    loop {
        let timing = tg.timing.lock().unwrap().clone();
        let Some(token) = tg.token() else {
            tg.set_polling(false);
            tokio::time::sleep(Duration::from_millis(timing.error_backoff_ms.max(1000) * 12)).await;
            continue;
        };
        if !announced {
            let commands = json!({ "commands": [
                { "command": "watch", "description": "toggle watching per lane" },
                { "command": "status", "description": "full lane report now" },
            ]});
            let set = tg.call(&token, "setMyCommands", &commands, Duration::from_secs(20)).await;
            let reset = json!({
                "chat_id": tg.chat_id(),
                "text": "🧹 menu updated — send /watch to toggle watching per lane, /status for a report",
                "reply_markup": { "remove_keyboard": true },
            });
            if set.is_ok() && tg.chat_id() != 0 {
                let _ = tg.call(&token, "sendMessage", &reset, Duration::from_secs(20)).await;
            }
            announced = set.is_ok();
        }
        let started = std::time::Instant::now();
        match poll_once(&hub, &token).await {
            Ok(n) => {
                // A long-poll that came back at once with nothing (a proxy, a fake): do not spin.
                if n == 0 && started.elapsed() < Duration::from_millis(200) {
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                }
                tg.set_polling(true);
                if conflict {
                    hub.event(Level::Info, "telegram.polling", None, None, "Telegram commands: polling again").await;
                }
                conflict = false;
                backoff = 0;
            }
            Err(err) if err.code == Some(409) => {
                tg.set_polling(false);
                if !conflict {
                    hub.event(Level::Warning, "telegram.conflict", None, None, "another process is polling this bot token; MAP's /watch and /status wait until it stops (or set telegram.commands = false)").await;
                }
                conflict = true;
                backoff = if backoff == 0 { timing.conflict_backoff_ms } else { (backoff * 2).min(timing.conflict_max_ms) };
                tokio::time::sleep(Duration::from_millis(backoff)).await;
            }
            Err(err) => {
                tg.set_polling(false);
                tg.state.lock().unwrap().last_error = Some(err.message.clone());
                tracing::warn!("telegram getUpdates failed: {err}");
                tokio::time::sleep(Duration::from_millis(timing.error_backoff_ms)).await;
            }
        }
    }
}

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod tests;
