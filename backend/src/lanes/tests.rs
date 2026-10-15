//! Lane rules: the agent-watcher's selftest-ratelimit.py cases, the clear/ask matrix, tells, and ticks on a fake
//! terminal. `integration_private_tmux` drives a real tmux server on a private socket.

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};

use super::*;
use crate::config::Config;
use crate::hub::Hub;
use crate::model::{LaneClearWhen, LaneStatusFile};
use crate::store::Store;
use crate::terminal::fake::{FakeSession, FakeTerminal};

fn ict() -> FixedOffset {
    FixedOffset::east_opt(7 * 3600).unwrap()
}

/// 2026-09-14 12:23 +07 (05:23 UTC): three minutes after the reset in [`REAL`].
fn now() -> DateTime<Utc> {
    ict().with_ymd_and_hms(2026, 9, 14, 12, 23, 0).unwrap().with_timezone(&Utc)
}

const REAL: &str = "  ⎿  You've hit your session limit · resets 5:20am (UTC)";

fn at(s: &str) -> Option<DateTime<Utc>> {
    Some(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
}

fn mins(m: i64) -> Option<DateTime<Utc>> {
    Some(now() + Duration::minutes(m))
}

fn pane_at(resets: Option<DateTime<Utc>>) -> Pane {
    Pane { rate_limit_line: REAL.into(), rate_limit_resets: resets, ..Default::default() }
}

fn status(state: &str, need_clear: Option<bool>, age_mins: i64) -> LaneStatusFile {
    LaneStatusFile {
        card: Some("252".into()),
        state: Some(state.into()),
        need_clear,
        note: None,
        at: Some((now() - Duration::minutes(age_mins)).with_timezone(&ict()).to_rfc3339()),
    }
}

fn ask_rules() -> LaneAskWhen {
    LaneAskWhen { cooldown_mins: 60, idle_stale_mins: 30, resume_cooldown_mins: 10, ..Default::default() }
}

fn mem_asked(m: i64) -> SessionMem {
    SessionMem { last_ask: mins(m), ..Default::default() }
}

#[test]
fn parse_reset_reads_the_clis_time_with_the_fifteen_minute_rule() {
    let n = now();
    assert_eq!(parse_reset(REAL, n, ict()), at("2026-09-14T12:20:00+07:00"), "3 min past: today, just reset");
    let six = ict().with_ymd_and_hms(2026, 9, 14, 6, 0, 0).unwrap().with_timezone(&Utc);
    assert_eq!(parse_reset(REAL, six, ict()), at("2026-09-14T12:20:00+07:00"), "still ahead");
    let late = ict().with_ymd_and_hms(2026, 9, 14, 15, 20, 0).unwrap().with_timezone(&Utc);
    assert_eq!(parse_reset(REAL, late, ict()), at("2026-09-15T12:20:00+07:00"), "3 h past: tomorrow");
    assert_eq!(parse_reset("resets 11:00pm", n, ict()), at("2026-09-14T23:00:00+07:00"), "no zone: local");
    assert_eq!(parse_reset("resets 12:00am (UTC)", n, ict()), at("2026-09-15T07:00:00+07:00"));
    assert_eq!(parse_reset("resets 12:00pm (UTC)", n, ict()), at("2026-09-14T19:00:00+07:00"));
    assert_eq!(parse_reset("resets at 9pm", n, ict()), at("2026-09-14T21:00:00+07:00"));
    assert_eq!(parse_reset("resets 99:99", n, ict()), None);
    assert_eq!(parse_reset("limit reached, sorry", n, ict()), None);
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn find_rate_limit_needs_the_phrase_and_the_time_together() {
    assert!(!find_rate_limit(&strings(&[REAL])).is_empty());
    assert!(!find_rate_limit(&strings(&["You've hit your session limit ·", "resets 5:20am (UTC)"])).is_empty(), "wrapped");
    assert!(!find_rate_limit(&strings(&["Claude usage limit reached. Your limit will reset at 5pm (Asia/Bangkok)"])).is_empty());
    assert!(find_rate_limit(&strings(&["You've hit your session limit"])).is_empty(), "no time");
    assert!(find_rate_limit(&strings(&["the cache resets 5:20am (UTC)"])).is_empty(), "no limit phrase");
    assert!(find_rate_limit(&strings(&["● I hit a rate limit calling the API and added a retry"])).is_empty(), "agent talk");
    assert!(find_rate_limit(&strings(&["You've hit your session limit", "", "resets 5:20am (UTC)"])).is_empty(), "two lines apart");
}

#[test]
fn decide_ask_waits_for_the_reset_then_resumes() {
    let r = ask_rules();
    let w = status("working", None, 0);
    let ask = |state, mem: &SessionMem, stale, no_todo, pane: &Pane| decide_ask(&r, state, mem, stale, Some(&w), no_todo, pane, now());
    assert!(ask(Raw::Idle, &mem_asked(-90), 90.0, false, &pane_at(mins(40))).is_none(), "quota not back yet");
    // Stale below idle_stale_mins on purpose: the resume rule alone must fire.
    let got = ask(Raw::Idle, &mem_asked(-90), 5.0, false, &pane_at(mins(-2))).unwrap();
    assert_eq!(got.1, AskKind::Resume);
    assert_eq!(ask(Raw::Idle, &mem_asked(-53), 60.0, false, &pane_at(mins(-2))).unwrap().1, AskKind::Resume, "beats the 60 min cooldown (2026-09-14)");
    assert!(ask(Raw::Idle, &mem_asked(0), 60.0, false, &pane_at(mins(-2))).is_none(), "just woken: no repeat");
    assert_eq!(ask(Raw::Idle, &mem_asked(-90), 90.0, true, &pane_at(mins(-2))).unwrap().1, AskKind::Resume, "an empty todo/ still resumes");
    assert!(ask(Raw::Busy, &mem_asked(-90), 90.0, false, &pane_at(mins(-2))).is_none());
    assert_eq!(ask(Raw::Idle, &mem_asked(-90), 90.0, false, &Pane::default()).unwrap().1, AskKind::Stale, "no limit: the usual rules");
}

#[test]
fn decide_ask_done_stale_cooldown_and_no_todo() {
    let r = ask_rules();
    let done = status("done", Some(false), 5);
    let free = Pane::default();
    let ask = |mem: &SessionMem, stale, s: Option<&LaneStatusFile>, no_todo| decide_ask(&r, Raw::Idle, mem, stale, s, no_todo, &free, now());
    assert_eq!(ask(&SessionMem::default(), 1.0, Some(&done), false).unwrap().1, AskKind::Done, "done: prompt at once");
    assert!(ask(&mem_asked(-5), 1.0, Some(&done), false).is_none(), "resume cooldown");
    assert_eq!(ask(&mem_asked(-15), 1.0, Some(&done), false).unwrap().1, AskKind::Done, "past the short cooldown");
    assert!(ask(&SessionMem::default(), 99.0, Some(&done), true).is_none(), "no card left: no prompt at all");
    let old = status("done", None, 300);
    assert!(ask(&SessionMem::default(), 1.0, Some(&old), false).is_none(), "an old status file is ignored");
    assert_eq!(ask(&SessionMem::default(), 31.0, None, false).unwrap().1, AskKind::Stale);
    assert!(ask(&mem_asked(-30), 31.0, None, false).is_none(), "cooldown");
    assert!(ask(&SessionMem::default(), 31.0, Some(&status("blocked", None, 1)), true).is_none());
    let off = LaneAskWhen { idle_stale_mins: 0, ..ask_rules() };
    assert!(decide_ask(&off, Raw::Idle, &SessionMem::default(), 999.0, None, false, &free, now()).is_none(), "0 = never for staleness");
    for state in [Raw::Busy, Raw::Waiting, Raw::PendingInput] {
        assert!(decide_ask(&r, state, &SessionMem::default(), 99.0, Some(&done), false, &free, now()).is_none());
    }
}

#[test]
fn stall_alert_only_where_map_cannot_act() {
    let r = LaneStallWhen::default();
    let free = Pane::default();
    let n = now();
    assert!(stall_alert(Raw::Waiting, &free, 47.0, &r, n).contains("dialog stuck"));
    assert!(stall_alert(Raw::Waiting, &free, 5.0, &r, n).is_empty(), "just appeared");
    assert!(stall_alert(Raw::PendingInput, &free, 30.0, &r, n).contains("unsent text"));
    assert!(stall_alert(Raw::Idle, &free, 300.0, &r, n).is_empty(), "idle is decide_ask's job");
    assert!(stall_alert(Raw::Busy, &free, 300.0, &r, n).is_empty(), "a long build is quiet");
    assert!(stall_alert(Raw::Idle, &pane_at(mins(40)), 40.0, &r, n).is_empty(), "waiting for the quota is normal");
    assert!(stall_alert(Raw::Idle, &pane_at(mins(-2)), 40.0, &r, n).is_empty(), "just woken: give it time");
    assert!(stall_alert(Raw::Idle, &pane_at(mins(-30)), 40.0, &r, n).contains("still parked"), "woken and did not move");
    assert!(stall_alert(Raw::Waiting, &free, 47.0, &LaneStallWhen { stall_mins: 90, repeat_mins: 120 }, n).is_empty(), "configurable");
}

fn clear_rules() -> LaneClearWhen {
    LaneClearWhen { enabled: true, cooldown_mins: 120, idle_mins: 20, context_pct: 85, only_when_done: false, status_max_age_mins: 180 }
}

#[test]
fn decide_clear_matrix() {
    let r = clear_rules();
    let n = now();
    let wants = status("done", Some(true), 1);
    let hint = Pane { clear_hint: true, context_pct: Some(99), ..Default::default() };
    let clear = |r: &LaneClearWhen, state, mem: &SessionMem, pane: &Pane, stale, s: Option<&LaneStatusFile>| decide_clear(r, state, mem, pane, stale, s, n);

    let parked = Pane { clear_hint: true, context_pct: Some(99), ..pane_at(mins(40)) };
    assert!(clear(&r, Raw::Idle, &SessionMem::default(), &parked, 90.0, Some(&wants)).is_empty(), "never while parked, even if asked");
    assert!(!clear(&r, Raw::Idle, &SessionMem::default(), &hint, 90.0, Some(&wants)).is_empty(), "free: the usual rules");
    assert!(clear(&LaneClearWhen { enabled: false, ..r.clone() }, Raw::Idle, &SessionMem::default(), &hint, 90.0, Some(&wants)).is_empty());
    for state in [Raw::Busy, Raw::Waiting, Raw::PendingInput] {
        assert!(clear(&r, state, &SessionMem::default(), &hint, 90.0, Some(&wants)).is_empty());
    }
    let recent = SessionMem { last_clear: mins(-30), ..Default::default() };
    assert!(clear(&r, Raw::Idle, &recent, &hint, 90.0, Some(&wants)).is_empty(), "cooldown beats the agent's request");
    // need_clear: true skips idle_mins; false wins over the CLI's hint.
    assert!(clear(&r, Raw::Idle, &SessionMem::default(), &Pane::default(), 1.0, Some(&wants)).contains("agent asked"));
    let keep = status("done", Some(false), 1);
    assert!(clear(&r, Raw::Idle, &SessionMem::default(), &hint, 90.0, Some(&keep)).is_empty());
    // No status file: the CLI's hint or meter after idle_mins.
    assert!(clear(&r, Raw::Idle, &SessionMem::default(), &hint, 10.0, None).is_empty(), "not idle long enough");
    assert!(clear(&r, Raw::Idle, &SessionMem::default(), &Pane { clear_hint: true, ..Default::default() }, 25.0, None).contains("CLI suggests"));
    assert!(clear(&r, Raw::Idle, &SessionMem::default(), &Pane { context_pct: Some(90), ..Default::default() }, 25.0, None).contains("context at 90%"));
    assert!(clear(&r, Raw::Idle, &SessionMem::default(), &Pane { context_pct: Some(40), ..Default::default() }, 25.0, None).is_empty());
    // only_when_done: never mid-card, whatever the meter says or the agent asks.
    let done_only = LaneClearWhen { only_when_done: true, ..r.clone() };
    let mid = status("working", Some(true), 1);
    assert!(clear(&done_only, Raw::Idle, &SessionMem::default(), &hint, 90.0, Some(&mid)).is_empty(), "mid-card");
    assert!(clear(&done_only, Raw::Idle, &SessionMem::default(), &hint, 90.0, None).is_empty(), "no status file");
    assert!(clear(&done_only, Raw::Idle, &SessionMem::default(), &hint, 90.0, Some(&status("done", Some(true), 500))).is_empty(), "stale status");
    assert!(!clear(&done_only, Raw::Idle, &SessionMem::default(), &hint, 90.0, Some(&wants)).is_empty());
    assert!(!clear(&done_only, Raw::Idle, &SessionMem::default(), &hint, 90.0, Some(&status("done", None, 1))).is_empty(), "done, no need_clear: hint decides");
}

const B: &str = "────────────────────────────────────────";

/// A Claude Code pane: `body`, then the status line, the input box with `input`, and the footer.
fn claude(body: &str, status_line: &str, input: &str, footer: &str) -> String {
    format!("{body}\n\n{status_line}\n\n{B}\n❯ {input}\n{B}\n  {footer}\n")
}

#[test]
fn pane_reading_context_hint_input_and_state() {
    let n = now();
    let idle = claude("● Card 464 is done and pushed.", "✻ Cooked for 3m 2s", "", "new task? /clear to save 52k tokens · 87% context used");
    let p = parse_pane(&idle, n, ict());
    assert_eq!((p.context_pct, p.clear_hint, p.input_text.as_str()), (Some(87), true, ""));
    assert_eq!(classify(&idle, &idle, n, ict()).0, Raw::Idle);
    assert_eq!(screen_tail(&p, 3), "● Card 464 is done and pushed.");

    // The faint suggestion is not typing; real text is.
    let hint = claude("● ok", "✻ Baked for 12s", "\x1b[2mTry \"fix lint\"\x1b[0m", "? for shortcuts");
    assert_eq!(classify(&hint, &hint, n, ict()).0, Raw::Idle);
    let typed = claude("● ok", "✻ Baked for 12s", "deploy now", "? for shortcuts");
    let (state, why, _) = classify(&typed, &typed, n, ict());
    assert_eq!(state, Raw::PendingInput);
    assert!(why.contains("deploy now"));

    let busy = claude("● Reading files", "✻ Pondering… (12s · ↓ 1.2k tokens · esc to interrupt)", "", "? for shortcuts");
    assert_eq!(classify(&busy, &busy, n, ict()).0, Raw::Busy);
    // Timers ticking are not movement; new text is.
    let later = idle.replace("3m 2s", "3m 9s");
    assert_eq!(classify(&idle, &later, n, ict()).0, Raw::Idle);
    let moved = idle.replace("pushed.", "pushed. Next:");
    assert_eq!(classify(&idle, &moved, n, ict()).0, Raw::Busy);

    let dialog = claude("Do you want to make this edit?\n❯ 1. Yes\n  2. No", "", "", "");
    assert_eq!(classify(&dialog, &dialog, n, ict()).0, Raw::Waiting);
    // A numbered list in an answer is not a dialog.
    let list = claude("● Plan:\n  1. read\n  2. write", "✻ Cooked for 1m", "", "? for shortcuts");
    assert_eq!(classify(&list, &list, n, ict()).0, Raw::Idle);

    let parked = claude(&format!("● Exploring…\n{REAL}\n   /login to switch to an API usage-billed account."), "", "", "? for shortcuts");
    let p = parse_pane(&parked, n, ict());
    assert!(!p.rate_limit_line.is_empty());
    assert_eq!(p.rate_limit_resets, at("2026-09-14T12:20:00+07:00"));

    let agents = format!("{}  ● reviewer · ↓ 3.4k tokens · 2m\n", claude("● x", "✻ Cooked for 1m", "", "? for shortcuts"));
    assert_eq!(parse_pane(&agents, n, ict()).agents.len(), 1);
    assert_eq!(trim_agent_line("●   reviewer   · ↓ 3.4k tokens · 2m"), "● reviewer · 2m");
}

#[test]
fn glob_and_signature() {
    assert!(glob("groove*", "groove-agos") && glob("groove*", "groove") && !glob("groove*", "my-groove"));
    assert!(glob("lane-?", "lane-a") && !glob("lane-?", "lane-ab") && glob("exact", "exact"));
    assert_eq!(done_signature(Some(&status("working", None, 0))), "");
    assert!(done_signature(Some(&status("done", None, 0))).starts_with("252|"));
    assert_eq!(done_signature(None), "");
}

#[test]
fn status_file_is_lenient() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".scratch")).unwrap();
    let f = dir.path().join(".scratch/agent-status.json");
    std::fs::write(&f, r#"{"card": 68, "state": "done", "need_clear": "yes", "note": null, "at": "2026-09-14T12:00:00+07:00", "by": "codex"}"#).unwrap();
    let s = status_facts(dir.path(), ".scratch/agent-status.json").unwrap();
    assert_eq!((s.card.as_deref(), s.state.as_deref(), s.need_clear), (Some("68"), Some("done"), None));
    std::fs::write(&f, "not json").unwrap();
    assert!(status_facts(dir.path(), ".scratch/agent-status.json").is_none());
    std::fs::write(&f, "[1]").unwrap();
    assert!(status_facts(dir.path(), ".scratch/agent-status.json").is_none());

    assert!(task_facts(dir.path(), "tasks").is_none(), "no todo/: not a card lane");
    for d in ["todo", "todo-human", "done"] {
        std::fs::create_dir_all(dir.path().join("tasks").join(d)).unwrap();
    }
    std::fs::write(dir.path().join("tasks/todo/101-a.md"), "").unwrap();
    std::fs::write(dir.path().join("tasks/todo/notes.txt"), "").unwrap();
    std::fs::write(dir.path().join("tasks/done/100-b.md"), "").unwrap();
    let t = task_facts(dir.path(), "tasks").unwrap();
    assert_eq!((t.todo, t.todo_human, t.done, t.last_done.as_deref()), (1, 0, 1, Some("100-b")));
}

// ---- ticks on a fake terminal

pub(crate) async fn test_hub(extra: &str) -> (Arc<Hub>, Arc<FakeTerminal>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[lane_watch]\nconfirm_secs = 0\nclear_settle_secs = 0\nkey_delay_ms = 0\n{extra}",
        dir.path().display()
    );
    let config = Config::parse(&text, Path::new("/")).unwrap();
    let term = Arc::new(FakeTerminal::default());
    let hub = Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default()));
    hub.load_runtime_settings().await;
    hub.lanes.set_terminal("", term.clone());
    (hub, term, dir)
}

fn lane_toml(dir: &Path, extra: &str) -> String {
    format!(
        "[[lanes]]\nname = \"groove-agos\"\nsession_match = [\"groove*\"]\npath = \"{}\"\nstatus_prompt = \"pick the next card\"\n[lanes.clear_when]\nenabled = true\n{extra}",
        dir.display()
    )
}

fn add_session(term: &FakeTerminal, name: &str, pane: &str, attached: usize) {
    term.sessions.lock().unwrap().insert(name.into(), FakeSession { pane: pane.into(), attached, ..Default::default() });
}

fn write_status(dir: &Path, state: &str, need_clear: bool, at: DateTime<Utc>) {
    std::fs::create_dir_all(dir.join(".scratch")).unwrap();
    let body = serde_json::json!({ "card": "464", "state": state, "need_clear": need_clear, "note": "pushed", "at": at.to_rfc3339() });
    std::fs::write(dir.join(".scratch/agent-status.json"), body.to_string()).unwrap();
}

fn idle_done() -> String {
    claude("● Card 464 is done.", "✻ Cooked for 3m 2s", "", "new task? /clear to save 52k tokens · 70% context used")
}

fn cleared() -> String {
    claude("", "", "", "? for shortcuts")
}

fn busy() -> String {
    claude("● Working", "✻ Pondering… (esc to interrupt)", "", "? for shortcuts")
}

async fn kinds(hub: &Hub, kind: &str) -> usize {
    hub.store.events(500, None).await.unwrap().iter().filter(|e| e.kind == kind).count()
}

#[tokio::test]
async fn clear_then_prompt_only_when_idle_and_nobody_attached() {
    let proj = tempfile::tempdir().unwrap();
    let (hub, term, _d) = test_hub(&lane_toml(proj.path(), "")).await;
    let t0 = Utc::now();
    write_status(proj.path(), "done", true, t0);

    add_session(&term, "groove-agos", &busy(), 0);
    tick(&hub, t0).await;
    assert!(term.sent("groove-agos").is_empty(), "busy: never typed");
    assert_eq!(kinds(&hub, "lane.card_done").await, 1, "the done notice goes out at once, whatever the pane");

    // Idle but someone is attached: still nothing.
    term.sessions.lock().unwrap().get_mut("groove-agos").unwrap().attached = 1;
    term.set_pane("groove-agos", &idle_done());
    tick(&hub, t0 + Duration::minutes(1)).await;
    assert!(term.sent("groove-agos").is_empty(), "attached: never typed");

    // Unsent text on the input line: still nothing.
    term.sessions.lock().unwrap().get_mut("groove-agos").unwrap().attached = 0;
    term.set_pane("groove-agos", &idle_done().replace("❯ \n", "❯ hold on\n"));
    tick(&hub, t0 + Duration::minutes(2)).await;
    assert!(term.sent("groove-agos").is_empty(), "typing: never typed");

    term.set_pane("groove-agos", &idle_done());
    term.on_keys("groove-agos", &[&cleared(), &busy()]);
    tick(&hub, t0 + Duration::minutes(3)).await;
    assert_eq!(term.sent("groove-agos"), ["/clear", "<Enter>", "pick the next card", "<Enter>"]);
    assert_eq!(kinds(&hub, "lane.clear").await, 1);
    assert_eq!(kinds(&hub, "lane.ask").await, 1);
    assert_eq!(kinds(&hub, "lane.card_done").await, 1, "once per card");

    // Done and idle again: cooldowns hold; the same card is not announced twice.
    term.set_pane("groove-agos", &idle_done());
    tick(&hub, t0 + Duration::minutes(5)).await;
    assert_eq!(term.sent("groove-agos").len(), 4);
    assert_eq!(kinds(&hub, "lane.card_done").await, 1);

    let info = hub.lanes.infos(&hub).await.unwrap().remove(0);
    assert_eq!(info.sessions[0].state, LaneState::Idle);
    assert_eq!(info.status.unwrap().card.as_deref(), Some("464"));
    assert!(info.history.iter().any(|a| a.kind == "clear"));
    assert!(hub.lanes.screen("groove-agos", None, 20).text.contains("Card 464"));
    assert!(hub.lanes.report(Utc::now()).contains("card 464"));
}

#[tokio::test]
async fn actions_off_and_watch_off_never_type() {
    let proj = tempfile::tempdir().unwrap();
    let (hub, term, _d) = test_hub(&lane_toml(proj.path(), "")).await;
    let t0 = Utc::now();
    write_status(proj.path(), "done", true, t0);
    add_session(&term, "groove-agos", &idle_done(), 0);
    hub.lanes.update(&hub, "groove-agos", LanePatch { actions: Some(false), ..Default::default() }).await.unwrap();
    tick(&hub, t0).await;
    assert!(term.sent("groove-agos").is_empty());
    assert_eq!(kinds(&hub, "lane.card_done").await, 1, "watched: still notified");

    hub.lanes.update(&hub, "groove-agos", LanePatch { actions: Some(true), watch: Some(false), ..Default::default() }).await.unwrap();
    write_status(proj.path(), "done", true, t0 + Duration::minutes(1));
    tick(&hub, t0 + Duration::minutes(2)).await;
    assert!(term.sent("groove-agos").is_empty());
    assert_eq!(kinds(&hub, "lane.card_done").await, 1, "muted: no notice");
    assert!(hub.lanes.report(Utc::now()).contains("paused via /watch"));
    assert_eq!(kinds(&hub, "lane.watch").await, 2);
}

#[tokio::test]
async fn never_clear_while_rate_limited_and_resume_after_reset() {
    let proj = tempfile::tempdir().unwrap();
    let (hub, term, _d) = test_hub(&lane_toml(proj.path(), "")).await;
    let t0 = Utc::now();
    write_status(proj.path(), "done", true, t0);
    let reset = (t0 + Duration::minutes(30)).with_timezone(&chrono::Local);
    let line = format!("  ⎿  You've hit your session limit · resets {}", reset.format("%-I:%M%P"));
    let parked = claude(&format!("● Exploring\n{line}"), "", "", "new task? /clear · 90% context used");
    add_session(&term, "groove-agos", &parked, 0);
    tick(&hub, t0).await;
    assert!(term.sent("groove-agos").is_empty(), "parked: no clear, no prompt");
    assert_eq!(kinds(&hub, "lane.rate_limited").await, 1);
    let s = &hub.lanes.infos(&hub).await.unwrap()[0].sessions[0];
    assert_eq!(s.state, LaneState::RateLimited);

    // A tell queued while parked waits.
    tell(&hub, "groove-agos", TellVerb::Clear, Some("card done"), None, None, t0).await.unwrap();
    tick(&hub, t0 + Duration::minutes(5)).await;
    assert!(term.sent("groove-agos").is_empty());
    assert_eq!(hub.store.lane_tells(None, t0 - Duration::hours(3), Duration::hours(3)).await.unwrap().len(), 1);

    // After the reset: resume (not clear) first, beating the tell.
    let later = t0 + Duration::minutes(32);
    tick(&hub, later).await;
    let sent = term.sent("groove-agos");
    assert_eq!(sent, ["pick the next card", "<Enter>"], "resume, never /clear while the line is on screen");
    assert_eq!(kinds(&hub, "lane.resume").await, 1);
    assert_eq!(kinds(&hub, "lane.rate_limited").await, 1, "announced once");
}

#[tokio::test]
async fn tells_expire_overwrite_and_pause_blocks_only_automatic_moves() {
    let proj = tempfile::tempdir().unwrap();
    let (hub, term, _d) = test_hub(&lane_toml(proj.path(), "")).await;
    let t0 = Utc::now();
    add_session(&term, "groove-agos", &busy(), 0);
    tick(&hub, t0).await;

    // Overwrite: the same verb again replaces the first.
    tell(&hub, "groove-agos", TellVerb::Ask, Some("first"), None, None, t0).await.unwrap();
    tell(&hub, "groove-agos", TellVerb::Ask, Some("do card 188"), None, None, t0 + Duration::seconds(5)).await.unwrap();
    let q = hub.store.lane_tells(Some("groove-agos"), t0 - Duration::hours(3), Duration::hours(3)).await.unwrap();
    assert_eq!(q.len(), 1);
    assert_eq!((q[0].note.as_str(), q[0].session.as_str()), ("do card 188", "groove-agos"), "one session: resolved to it");
    assert!(tell(&hub, "nope", TellVerb::Ask, None, None, None, t0).await.is_err());
    assert!(tell(&hub, "groove-agos", TellVerb::Pause, None, Some(0), None, t0).await.is_err());

    // Expiry: three hours later it is gone.
    let q = hub.store.lane_tells(Some("groove-agos"), t0 + Duration::minutes(181) - Duration::hours(3), Duration::hours(3)).await.unwrap();
    assert!(q.is_empty(), "expired");

    // Pause, then the lane is done and idle: no automatic clear or prompt.
    write_status(proj.path(), "done", true, t0);
    term.set_pane("groove-agos", &idle_done());
    tell(&hub, "groove-agos", TellVerb::Pause, None, Some(90), None, t0).await.unwrap();
    tick(&hub, t0 + Duration::minutes(1)).await;
    assert!(term.sent("groove-agos").is_empty(), "paused");
    let s = &hub.lanes.infos(&hub).await.unwrap()[0];
    assert!(s.sessions[0].paused_until.is_some());
    assert!(s.tells.is_empty(), "the pause tell is used up");
    tick(&hub, t0 + Duration::minutes(2)).await;
    assert!(term.sent("groove-agos").is_empty(), "still paused");

    // A direct clear still runs while paused (by session name, as map-tell sends it), then the status prompt.
    tell(&hub, "groove-agos", TellVerb::Clear, Some("card 464 done"), None, Some("groove-agos"), t0 + Duration::minutes(2)).await.unwrap();
    term.on_keys("groove-agos", &[&cleared(), &busy()]);
    tick(&hub, t0 + Duration::minutes(3)).await;
    assert_eq!(term.sent("groove-agos"), ["/clear", "<Enter>", "pick the next card", "<Enter>"]);
    assert!(hub.store.lane_tells(None, t0 - Duration::hours(3), Duration::hours(3)).await.unwrap().is_empty());
    assert!(kinds(&hub, "lane.tell").await >= 3);
}

#[tokio::test]
async fn one_prompt_per_lane_per_round_and_no_todo_stops_the_prompt() {
    let proj = tempfile::tempdir().unwrap();
    let (hub, term, _d) = test_hub(&lane_toml(proj.path(), "[lanes.ask_when]\nidle_stale_mins = 30\n")).await;
    let t0 = Utc::now();
    let idle = claude("● waiting", "✻ Cooked for 1m", "", "? for shortcuts");
    add_session(&term, "groove-1", &idle, 0);
    add_session(&term, "groove-2", &idle, 0);
    tick(&hub, t0).await;
    tick(&hub, t0 + Duration::minutes(31)).await;
    let n = term.sent("groove-1").len() + term.sent("groove-2").len();
    assert_eq!(n, 2, "one session prompted (text + Enter)");

    // No card left in todo/: no stale prompt, but the lane is still read.
    for d in ["todo", "done"] {
        std::fs::create_dir_all(proj.path().join("tasks").join(d)).unwrap();
    }
    tick(&hub, t0 + Duration::minutes(200)).await;
    assert_eq!(term.sent("groove-1").len() + term.sent("groove-2").len(), 2);
    assert_eq!(hub.lanes.infos(&hub).await.unwrap()[0].tasks.as_ref().unwrap().todo, 0);
}

#[tokio::test]
async fn stall_alert_repeats_every_repeat_mins_not_every_tick() {
    let proj = tempfile::tempdir().unwrap();
    let (hub, term, _d) = test_hub(&lane_toml(proj.path(), "")).await;
    let t0 = Utc::now();
    let dialog = claude("Do you want to make this edit?\n❯ 1. Yes\n  2. No", "", "", "");
    add_session(&term, "groove-agos", &dialog, 0);
    for m in [0, 10, 25, 30, 60, 140, 150] {
        tick(&hub, t0 + Duration::minutes(m)).await;
    }
    assert_eq!(kinds(&hub, "lane.stall").await, 2, "at 25 min and again at 145+");
    assert!(term.sent("groove-agos").is_empty());
}

#[tokio::test]
async fn import_watcher_fills_settings_and_refuses_outside_home() {
    let proj = tempfile::tempdir().unwrap();
    let (hub, _term, _d) = test_hub(&lane_toml(proj.path(), "")).await;
    let home = tempfile::tempdir().unwrap();
    let cfg = serde_json::json!({
        "telegram": { "env_file": "/somewhere/.env", "chat_id": -5168825006i64, "allowed_user_ids": [786511637] },
        "targets": [
            { "key": "groove", "name": "groove-clinic", "session_match": ["groove*"], "path": proj.path(),
              "ask_when": { "on_anomaly": false, "idle_stale_mins": 30, "cooldown_mins": 60, "max_sessions_per_round": 1 },
              "stall_when": { "stall_mins": 20, "repeat_mins": 120 },
              "clear_when": { "enabled": true, "idle_mins": 30, "cooldown_mins": 30, "context_pct": 15, "only_when_done": true },
              "status_prompt": "LONG STATUS PROMPT" },
            { "key": "other", "session_match": ["other*"], "path": "/nowhere" }
        ]
    });
    let file = home.path().join("config.json");
    std::fs::write(&file, cfg.to_string()).unwrap();
    let got = import_watcher_in(&hub, file.to_str().unwrap(), home.path()).await.unwrap();
    assert_eq!(got.updated, ["groove-agos"]);
    assert_eq!(got.skipped.len(), 1);
    assert_eq!(got.chat_id, Some(-5168825006));
    let c = hub.lanes.config("groove-agos").unwrap();
    assert_eq!(c.status_prompt, "LONG STATUS PROMPT");
    assert_eq!((c.clear_when.context_pct, c.clear_when.status_max_age_mins, c.ask_when.cooldown_mins), (15, 180, 60));
    assert_eq!(hub.telegram.info().allowed_user_ids, [786511637]);

    let outside = tempfile::tempdir().unwrap();
    let f2 = outside.path().join("config.json");
    std::fs::write(&f2, "{}").unwrap();
    assert!(import_watcher_in(&hub, f2.to_str().unwrap(), home.path()).await.is_err(), "outside home");
    assert!(import_watcher_in(&hub, "/etc/hostname", home.path()).await.is_err());
}

// ---- a real tmux server on a private socket

/// Kills only the private test server, also when the test fails.
struct PrivateTmux(String);

impl PrivateTmux {
    fn cmd(&self) -> std::process::Command {
        let mut c = std::process::Command::new("tmux");
        c.env_remove("TMUX").env_remove("TMUX_PANE").arg("-L").arg(&self.0);
        c
    }
}

impl Drop for PrivateTmux {
    fn drop(&mut self) {
        let _ = self.cmd().arg("kill-server").output();
        // tmux may leave the socket file behind.
        let base = std::env::var("TMUX_TMPDIR").unwrap_or_else(|_| "/tmp".into());
        let _ = std::fs::remove_file(Path::new(&base).join(format!("tmux-{}", unsafe { libc::getuid() })).join(&self.0));
    }
}

/// A shell that draws a Claude-like pane: busy until `idle` exists in its folder, then idle with a card done and the
/// CLI's `/clear` hint; `/clear` draws an empty pane, anything else typed goes busy. Every line read is logged.
const FAKE_CLAUDE: &str = r#"#!/usr/bin/env bash
dir="$1"
stty -echo 2>/dev/null
B='────────────────────────────────────────'
state=busy; drawn=""
draw() {
  printf '\033[2J\033[H'
  for i in $(seq 1 14); do printf '\n'; done
  case "$state" in
    busy) printf '● Working on card 7\n\n✻ Pondering… (esc to interrupt)\n\n%s\n❯ \n%s\n  ? for shortcuts' "$B" "$B" ;;
    idle) printf '● Card 7 is done and pushed.\n\n✻ Cooked for 1m 2s\n\n%s\n❯ \n%s\n  new task? /clear to save 40k tokens · 62%% context used' "$B" "$B" ;;
    cleared) printf '\n\n\n\n%s\n❯ \n%s\n  ? for shortcuts · 0%% context used' "$B" "$B" ;;
  esac
}
while :; do
  if [ "$state" = busy ] && [ -f "$dir/idle" ]; then state=idle; fi
  if [ "$state" != "$drawn" ]; then draw; drawn="$state"; fi
  if IFS= read -r -t 1 line; then
    printf '%s\n' "$line" >> "$dir/typed.log"
    case "$line" in
      /clear) state=cleared ;;
      *) state=busy; rm -f "$dir/idle" ;;
    esac
  fi
done
"#;

fn typed(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("typed.log")).unwrap_or_default().lines().map(String::from).collect()
}

#[tokio::test]
async fn integration_private_tmux() {
    if std::process::Command::new("tmux").arg("-V").output().is_err() || std::process::Command::new("bash").arg("--version").output().is_err() {
        eprintln!("tmux or bash missing: skipped");
        return;
    }
    let socket = format!("map-lanes-test-{}", std::process::id());
    let tmux = PrivateTmux(socket.clone());
    let proj = tempfile::tempdir().unwrap();
    let script = proj.path().join("fake-claude.sh");
    std::fs::write(&script, FAKE_CLAUDE).unwrap();
    let conf = proj.path().join("tmux.conf");
    std::fs::write(&conf, "set -g history-limit 0\nset -g status off\n").unwrap();
    let out = tmux
        .cmd()
        .args(["-f", conf.to_str().unwrap(), "new-session", "-d", "-s", "lane-a", "-x", "120", "-y", "30"])
        .arg(format!("bash {} {}", script.display(), proj.path().display()))
        .output()
        .unwrap();
    assert!(out.status.success(), "tmux: {}", String::from_utf8_lossy(&out.stderr));

    let tg = crate::telegram::fake::FakeTelegram::start().await;
    let env = proj.path().join("tg.env");
    std::fs::write(&env, "MAP_LANES_TEST_TOKEN=123456:TESTTOKENtesttokenTESTTOKEN\n").unwrap();
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "[workspace]\ndir = \"{d}\"\n[server]\ndata_dir = \"{d}/data\"\n[lane_watch]\nconfirm_secs = 1\nclear_settle_secs = 1\nkey_delay_ms = 300\n\
         [telegram]\nenabled = true\ntoken_env = \"MAP_LANES_TEST_TOKEN\"\nenv_file = \"{env}\"\nchat_id = 42\napi_base = \"{base}\"\ncommands = false\n\
         [[lanes]]\nname = \"lane-a\"\nsocket = \"{socket}\"\npath = \"{p}\"\nstatus_prompt = \"STATUS PROMPT GO\"\n[lanes.clear_when]\nenabled = true\n",
        d = dir.path().display(),
        env = env.display(),
        base = tg.base,
        p = proj.path().display(),
    );
    let config = Config::parse(&text, Path::new("/")).unwrap();
    let hub = Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default()));
    hub.load_runtime_settings().await;
    hub.telegram.timing.lock().unwrap().report_check_ms = 3_600_000;
    crate::telegram::run(hub.clone()).await;
    write_status(proj.path(), "done", true, Utc::now());

    // Busy: nothing is typed, the card notice goes out once.
    tokio::time::sleep(StdDuration::from_millis(800)).await;
    tick(&hub, Utc::now()).await;
    assert!(typed(proj.path()).is_empty(), "typed into a busy pane: {:?}", typed(proj.path()));
    assert_eq!(hub.lanes.infos(&hub).await.unwrap()[0].sessions[0].state, LaneState::Busy);

    // Idle: /clear, then the status prompt in the same round.
    std::fs::write(proj.path().join("idle"), "").unwrap();
    tokio::time::sleep(StdDuration::from_millis(2500)).await;
    tick(&hub, Utc::now()).await;
    assert_eq!(typed(proj.path()), ["/clear", "STATUS PROMPT GO"]);

    // Busy again: nothing more, and no second card notice.
    tokio::time::sleep(StdDuration::from_millis(1500)).await;
    tick(&hub, Utc::now()).await;
    assert_eq!(typed(proj.path()).len(), 2);
    tokio::time::sleep(StdDuration::from_millis(500)).await;
    let done: Vec<String> = tg.texts().into_iter().filter(|t| t.starts_with("✅") && t.contains("card 464 finished")).collect();
    assert_eq!(done.len(), 1, "the done notice once: {:?}", tg.texts());
    assert!(tg.texts().iter().any(|t| t.contains("context cleared")));
    assert!(tg.tokens().iter().all(|t| t == "123456:TESTTOKENtesttokenTESTTOKEN"));
    drop(tmux);
}
