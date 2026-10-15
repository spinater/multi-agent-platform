//! Telegram against a fake Bot API: chunking, retries, redaction, the allowed-user filter, the offset, `/watch`,
//! and the 409 back-off.

use std::sync::Arc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use serde_json::{Value, json};

use super::fake::FakeTelegram;
use super::*;
use crate::hub::Hub;
use crate::lanes::tests::test_hub;

const TOKEN: &str = "123456789:AAEhBOweik9ai2o4tAAEhBOweik9ai2o4tX";

async fn hub_with(base: &str, extra: &str) -> (Arc<Hub>, tempfile::TempDir, tempfile::TempDir) {
    let files = tempfile::tempdir().unwrap();
    let env = files.path().join("bot.env");
    std::fs::write(&env, format!("# bot\nexport MAP_TG_TEST_TOKEN=\"{TOKEN}\"\n")).unwrap();
    let toml = format!(
        "[telegram]\nenabled = true\ntoken_env = \"MAP_TG_TEST_TOKEN\"\nenv_file = \"{}\"\nchat_id = -100\napi_base = \"{}\"\n{extra}\n\
         [[lanes]]\nname = \"groove-agos\"\nsession_match = [\"groove*\"]\n[[lanes]]\nname = \"manager-agos\"\nactions = false\n",
        env.display(),
        base
    );
    let (hub, _term, dir) = test_hub(&toml).await;
    *hub.telegram.timing.lock().unwrap() = Timing { retry_ms: 10, error_backoff_ms: 20, conflict_backoff_ms: 10_000, conflict_max_ms: 20_000, poll_secs: 0, report_check_ms: 3_600_000 };
    (hub, dir, files)
}

#[test]
fn chunks_split_on_lines_and_cut_long_lines() {
    assert_eq!(split_chunks("short", 10), ["short"]);
    let text = "aaaa\nbbbb\ncccc";
    assert_eq!(split_chunks(text, 10), ["aaaa\nbbbb", "cccc"]);
    let long = "x".repeat(25);
    let parts = split_chunks(&format!("head\n{long}"), 10);
    assert!(parts.iter().all(|p| p.chars().count() <= 10), "{parts:?}");
    assert_eq!(parts.concat().replace('\n', ""), format!("head{long}"));
    // Characters, not bytes: Thai text is not cut mid-character.
    let thai = "การ์ด ".repeat(2000);
    let parts = split_chunks(&thai, CHUNK);
    assert!(parts.len() >= 3 && parts.iter().all(|p| p.chars().count() <= CHUNK));
}

#[test]
fn redaction_removes_the_token_and_anything_shaped_like_one() {
    let url = format!("error sending request for url (https://api.telegram.org/bot{TOKEN}/sendMessage)");
    let out = redact(&url, Some(TOKEN));
    assert!(!out.contains(TOKEN) && !out.contains("AAEhBOweik9"), "{out}");
    let other = redact("https://api.telegram.org/bot987654321:ZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZZ/getUpdates", None);
    assert!(!other.contains("ZZZZ"), "{other}");
    assert_eq!(redact("chat -5168825006 ok", None), "chat -5168825006 ok");
}

#[test]
fn allowed_users_then_the_chat() {
    assert!(allowed(&[1, 2], -5, Some(2), Some(99)), "an allowed user anywhere");
    assert!(!allowed(&[1, 2], -5, Some(3), Some(-5)), "others in the chat are ignored");
    assert!(!allowed(&[1], -5, None, Some(-5)));
    assert!(allowed(&[], -5, Some(3), Some(-5)), "nobody listed: anyone in the chat");
    assert!(!allowed(&[], -5, Some(3), Some(77)), "but not another chat");
}

#[test]
fn env_file_and_report_schedule() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join(".env");
    std::fs::write(&f, "OTHER=1\nTELOXIDE_TOKEN=\"abc:def\"\n").unwrap();
    assert_eq!(read_env_file(&f, "TELOXIDE_TOKEN").as_deref(), Some("abc:def"));
    assert_eq!(read_env_file(&f, "MISSING"), None);

    let at = |h, m| Local.with_ymd_and_hms(2026, 10, 15, h, m, 0).unwrap();
    assert!(report_due(at(10, 3), None, 45), "first ever");
    let last = Some(at(9, 45).with_timezone(&Utc));
    assert!(!report_due(at(10, 0), last, 45));
    assert!(report_due(at(10, 45), last, 45), "on the minute");
    assert!(report_due(at(10, 56), last, 45), "over 70 min");
    assert!(!report_due(at(10, 45), Some(at(10, 45).with_timezone(&Utc)), 45), "once per window");
}

#[tokio::test]
async fn send_retries_three_times_and_never_leaks_the_token() {
    let tg = FakeTelegram::start().await;
    let (hub, _d, _f) = hub_with(&tg.base, "").await;
    tg.reply("sendMessage", 500, json!({ "ok": false, "description": "boom" }));
    tg.reply("sendMessage", 502, json!({ "ok": false }));
    hub.telegram.send("hello").await.unwrap();
    let calls = tg.calls("sendMessage");
    assert_eq!(calls.len(), 3);
    assert_eq!(calls[2], json!({ "chat_id": -100, "text": "hello", "disable_web_page_preview": true }));
    assert_eq!(hub.telegram.info().last_send_ok, Some(true));

    for _ in 0..3 {
        tg.reply("sendMessage", 400, json!({ "ok": false, "error_code": 400, "description": format!("Bad Request: token {TOKEN} rejected") }));
    }
    assert!(hub.telegram.send("again").await.is_err());
    let info = hub.telegram.info();
    assert_eq!(info.last_send_ok, Some(false));
    let err = info.last_error.unwrap();
    assert!(err.contains("Bad Request") && !err.contains(TOKEN), "{err}");
    let json = serde_json::to_string(&hub.telegram.info()).unwrap();
    assert!(!json.contains(TOKEN) && json.contains("\"token_set\":true"));

    // Long text goes in pieces.
    let before = tg.calls("sendMessage").len();
    hub.telegram.send(&"line\n".repeat(2000)).await.unwrap();
    assert!(tg.calls("sendMessage").len() - before >= 3);
}

#[tokio::test]
async fn transport_errors_are_redacted_too() {
    // Nothing listens on this port: reqwest's error would carry the URL, and the URL the token.
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = dead.local_addr().unwrap().port();
    drop(dead);
    let (hub, _d, _f) = hub_with(&format!("http://127.0.0.1:{port}"), "").await;
    let err = hub.telegram.send("x").await.unwrap_err();
    assert!(!err.contains(TOKEN) && !err.contains("AAEhB"), "{err}");
}

fn update(id: i64, from: i64, text: &str) -> Value {
    json!({ "update_id": id, "message": { "message_id": 5, "from": { "id": from }, "chat": { "id": -100 }, "text": text } })
}

#[tokio::test]
async fn offset_is_persisted_and_watch_toggles_a_lane() {
    let tg = FakeTelegram::start().await;
    let (hub, _d, _f) = hub_with(&tg.base, "allowed_user_ids = [7]").await;
    tg.reply("getUpdates", 200, json!({ "ok": true, "result": [update(41, 7, "/watch@map_bot"), update(42, 8, "/watch")] }));
    assert_eq!(poll_once(&hub, TOKEN).await.unwrap(), 2);
    assert_eq!(hub.store.runtime_setting("telegram.offset").await.unwrap().as_deref(), Some("43"));
    let sent = tg.calls("sendMessage");
    assert_eq!(sent.len(), 1, "user 8 is ignored silently");
    let rows = sent[0]["reply_markup"]["inline_keyboard"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0][0]["callback_data"], "t:groove-agos");
    assert!(sent[0]["text"].as_str().unwrap().contains("✅ watching · groove-agos"));

    // The next poll starts from the stored offset.
    poll_once(&hub, TOKEN).await.unwrap();
    assert_eq!(tg.calls("getUpdates")[1]["offset"], 43);

    let press = json!({ "update_id": 43, "callback_query": { "id": "cb1", "from": { "id": 7 }, "data": "t:groove-agos",
        "message": { "message_id": 9, "chat": { "id": -100 } } } });
    tg.reply("getUpdates", 200, json!({ "ok": true, "result": [press] }));
    poll_once(&hub, TOKEN).await.unwrap();
    assert!(!hub.lanes.config("groove-agos").unwrap().watch);
    assert_eq!(tg.calls("answerCallbackQuery")[0]["text"], "watching off ⏸");
    let edit = &tg.calls("editMessageText")[0];
    assert_eq!(edit["message_id"], 9);
    assert!(edit["text"].as_str().unwrap().contains("⏸ off · groove-agos"));
    assert_eq!(hub.store.runtime_setting("telegram.offset").await.unwrap().as_deref(), Some("44"));

    // /status answers with the report; the muted lane is one line.
    tg.reply("getUpdates", 200, json!({ "ok": true, "result": [update(44, 7, "/status")] }));
    poll_once(&hub, TOKEN).await.unwrap();
    let last = tg.texts().pop().unwrap();
    assert!(last.contains("groove-agos") && last.contains("paused via /watch"), "{last}");
    assert!(tg.tokens().iter().all(|t| t == TOKEN));
}

#[tokio::test]
async fn a_409_warns_once_and_backs_off() {
    let tg = FakeTelegram::start().await;
    let (hub, _d, _f) = hub_with(&tg.base, "").await;
    for _ in 0..50 {
        tg.reply("getUpdates", 409, json!({ "ok": false, "error_code": 409, "description": "Conflict: terminated by other getUpdates request" }));
    }
    let task = tokio::spawn(listen(hub.clone()));
    tokio::time::sleep(Duration::from_millis(600)).await;
    task.abort();
    assert_eq!(tg.calls("getUpdates").len(), 1, "no spinning while another process polls");
    assert_eq!(tg.calls("setMyCommands").len(), 1);
    let reset = tg.calls("sendMessage");
    assert_eq!(reset[0]["reply_markup"], json!({ "remove_keyboard": true }));
    let events = hub.store.events(50, None).await.unwrap();
    let conflicts: Vec<_> = events.iter().filter(|e| e.kind == "telegram.conflict").collect();
    assert_eq!(conflicts.len(), 1);
    assert!(conflicts[0].message.contains("another process is polling this bot token"));
    assert!(!hub.telegram.info().polling);
}

#[tokio::test]
async fn notices_follow_the_lane_mute_and_task_events() {
    let tg = FakeTelegram::start().await;
    let (hub, _d, _f) = hub_with(&tg.base, "").await;
    let ev = |kind: &str, session: Option<&str>, task: Option<i64>, msg: &str| Event {
        id: 1,
        at: Utc::now(),
        level: Level::Info,
        kind: kind.into(),
        session: session.map(String::from),
        task_id: task,
        message: msg.into(),
    };
    assert!(notice(&hub, &ev("lane.card_done", Some("groove-agos"), None, "✅ card 1 finished")).is_some());
    hub.lanes.update(&hub, "groove-agos", crate::lanes::LanePatch { watch: Some(false), ..Default::default() }).await.unwrap();
    assert!(notice(&hub, &ev("lane.card_done", Some("groove-agos"), None, "✅ card 1 finished")).is_none(), "muted");
    assert!(notice(&hub, &ev("lane.tell", Some("manager-agos"), None, "x")).is_none(), "not a notice");
    assert_eq!(notice(&hub, &ev("task.failed", None, Some(4), "Failed: verify")).unwrap(), "❌ MAP · task #4 failed: verify");
    assert!(notice(&hub, &ev("task.held", None, Some(5), "Reflex: #5 is unclear")).unwrap().contains("held by intake"));
    assert!(notice(&hub, &ev("plan.created", None, None, "Plan #2 … waiting for approval")).is_some());
    assert!(notice(&hub, &ev("task.done", None, Some(3), "Task #3 done: x")).is_some());
    assert!(notice(&hub, &ev("session.started", None, None, "x")).is_none());

    // The hourly report waits for the first lane tick, then goes out once per window.
    assert!(!report_if_due(&hub, Local::now()).await);
    crate::lanes::tick(&hub, Utc::now()).await;
    assert!(report_if_due(&hub, Local::now()).await);
    assert!(!report_if_due(&hub, Local::now()).await);
    assert!(hub.telegram.info().last_report_at.is_some());
}
