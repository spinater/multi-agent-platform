//! The manager: its session and prompt, the token, the supervisor's restarts and back-off, persistent lanes, and the
//! manager token's allowed/forbidden matrix through the real router. `integration_private_tmux` runs a fake provider
//! on a private tmux socket.

use std::path::Path;
use std::sync::Arc;

use chrono::{Duration, Utc};

use super::*;
use crate::model::{ManagerSettings, OpenLane, SessionState};
use crate::plans::tests::{hub, project};

async fn enable(hub: &Arc<Hub>, slug: &str) {
    let mut p = hub.store.project(slug).await.unwrap().unwrap();
    p.settings.manager = ManagerSettings { enabled: true, provider: Some("claude".into()), model: None };
    hub.store.save_project(&p).await.unwrap();
}

async fn kinds(hub: &Hub, kind: &str) -> usize {
    hub.store.events(500, None).await.unwrap().iter().filter(|e| e.kind == kind).count()
}

#[tokio::test]
async fn manager_session_has_the_role_token_prompt_and_a_named_conversation() {
    // Isolated: the transcript below goes in the test's own Claude folder, never the real one.
    let (hub, term, dir) = hub("[agents_home]\nmode = \"isolated\"").await;
    let folder = dir.path().join("app");
    let slug = project(&hub, &folder, "App").await;
    let s = start(&hub, &slug).await.unwrap();
    assert_eq!((s.name.as_str(), s.role, s.task_id, s.project.as_deref(), s.provider.as_str()), ("map-mgr-app", Role::Supervisor, None, Some("app"), "claude"));
    let script = term.sessions.lock().unwrap()["map-mgr-app"].script.clone();
    let mt = hub.manager_token(&slug).unwrap();
    assert!(script.contains(&format!("export MAP_MANAGER_TOKEN={mt}")), "{script}");
    assert_ne!(mt, hub.project_token(&slug).unwrap(), "the manager token is its own kind");
    assert!(script.contains(&format!("export MAP_PROJECT_TOKEN={}", hub.project_token(&slug).unwrap())));
    assert!(script.contains("--session-id "), "a conversation MAP can resume: {script}");
    assert!(!script.contains("MAP_RESULT_FILE"), "not a worker");
    let prompt = std::fs::read_to_string(hub.run_dir("map-mgr-app").join("prompt.md")).unwrap();
    assert!(prompt.starts_with("# Project manager\n") && prompt.contains("## This session") && prompt.contains("- Project: App (`app`"));
    assert!(prompt.contains("$MAP_MANAGER_TOKEN") && prompt.contains("/approve") && prompt.contains("/lanes"));
    assert!(!prompt.contains(&mt), "the token is never in the prompt");

    // Killed: the next start resumes the same conversation once its transcript exists.
    let id = hub.store.runtime_setting(&conversation_key("map-mgr-app")).await.unwrap().unwrap();
    assert!(script.contains(&id));
    let home = hub.provider_home("claude").unwrap();
    assert!(home.starts_with(dir.path()), "{}", home.display());
    let tdir = home.join("projects").join("-x");
    std::fs::create_dir_all(&tdir).unwrap();
    let transcript = tdir.join(format!("{id}.jsonl"));
    std::fs::write(&transcript, "{}\n").unwrap();
    term.exit("map-mgr-app");
    let again = start(&hub, &slug).await.unwrap();
    assert_eq!(again.name, "map-mgr-app");
    let script = term.sessions.lock().unwrap()["map-mgr-app"].script.clone();
    assert!(script.contains(&format!("--resume {id}")), "{script}");
    let prompt = std::fs::read_to_string(hub.run_dir("map-mgr-app").join("prompt.md")).unwrap();
    assert!(prompt.starts_with("MAP restarted this session"), "a resumed manager is not re-instructed: {prompt}");
    std::fs::remove_file(&transcript).unwrap();
    let _ = std::fs::remove_dir(&tdir);
}

#[tokio::test]
async fn supervisor_restarts_then_backs_off_after_three_failures() {
    let (hub, term, dir) = hub("").await;
    let slug = project(&hub, &dir.path().join("app"), "App").await;
    let other = project(&hub, &dir.path().join("off"), "Off").await;
    enable(&hub, &slug).await;
    let t0 = Utc::now();
    supervise(&hub, t0).await;
    assert!(term.sessions.lock().unwrap().contains_key("map-mgr-app"), "started at boot");
    assert!(!term.sessions.lock().unwrap().contains_key(&format!("map-mgr-{other}")), "a manager that is off is not started");
    assert_eq!(info(&hub, &slug).await.unwrap().state, "starting");
    supervise(&hub, t0 + Duration::seconds(15)).await;
    assert_eq!(kinds(&hub, "manager.started").await, 1, "running: left alone");

    for i in 1..=3 {
        term.exit("map-mgr-app");
        supervise(&hub, t0 + Duration::minutes(i)).await;
    }
    assert_eq!(kinds(&hub, "manager.started").await, 3, "restarted twice, then stopped");
    assert_eq!(kinds(&hub, "manager.restarting").await, 2);
    assert_eq!(kinds(&hub, "manager.stopped").await, 1);
    let i = info(&hub, &slug).await.unwrap();
    assert_eq!(i.state, "stopped");
    assert!(i.warning.unwrap().contains("3 failures"));
    supervise(&hub, t0 + Duration::minutes(5)).await;
    assert!(!term.sessions.lock().unwrap().contains_key("map-mgr-app"), "stopped: no more restarts");

    // The owner's Start clears the back-off.
    start_now(&hub, &slug).await.unwrap();
    assert_eq!(info(&hub, &slug).await.unwrap().warning, None);
    // Failures far apart never add up to three.
    for i in 0..4 {
        term.exit("map-mgr-app");
        supervise(&hub, t0 + Duration::minutes(20 + i * 11)).await;
    }
    assert_ne!(info(&hub, &slug).await.unwrap().state, "stopped");
}

#[tokio::test]
async fn configure_registers_the_manager_as_an_owned_lane_and_turning_off_closes_it() {
    let (hub, term, dir) = hub("").await;
    let slug = project(&hub, &dir.path().join("app"), "App").await;
    let i = configure(&hub, &slug, ManagerSettings { enabled: true, provider: Some("claude".into()), model: Some("opus".into()) }, false).await.unwrap();
    assert_eq!((i.settings.enabled, i.session.as_ref().map(|s| s.model.as_str())), (true, Some("opus")));
    let lane = hub.lanes.config("map-mgr-app").unwrap();
    assert_eq!(lane.owned, Some(OwnedLane { project: slug.clone(), kind: "manager".into() }));
    assert!(!lane.clear_when.enabled && lane.ask_when.idle_stale_mins == 0 && lane.actions);
    assert!(configure(&hub, &slug, ManagerSettings { enabled: true, provider: Some("nope".into()), model: None }, false).await.is_err());

    term.sessions.lock().unwrap().get_mut("map-mgr-app").unwrap().attached = 1;
    let err = configure(&hub, &slug, ManagerSettings::default(), false).await.unwrap_err();
    assert!(format!("{err:#}").contains("attached"));
    let off = configure(&hub, &slug, ManagerSettings::default(), true).await.unwrap();
    assert_eq!(off.state, "off");
    assert!(!term.sessions.lock().unwrap().contains_key("map-mgr-app"));
    assert!(hub.lanes.config("map-mgr-app").is_none(), "off: no lane");
}

#[tokio::test]
async fn persistent_lanes_open_persist_stop_and_close() {
    let (hub, term, dir) = hub("").await;
    let slug = project(&hub, &dir.path().join("app"), "App").await;
    let body = OpenLane { name: "docs".into(), prompt: Some("Keep the docs in sync.".into()), status_prompt: Some("next card".into()), ..Default::default() };
    let rec = open_lane(&hub, &slug, body.clone(), "manager").await.unwrap();
    assert_eq!((rec.lane.as_str(), rec.provider.as_str(), rec.opened_by.as_str()), ("map-lane-app-docs", "claude", "manager"));
    assert!(open_lane(&hub, &slug, body.clone(), "owner").await.is_err(), "names are unique");
    assert!(open_lane(&hub, &slug, OpenLane { name: "bad name!".into(), ..Default::default() }, "owner").await.is_err());
    let cwd = term.sessions.lock().unwrap()["map-lane-app-docs"].cwd.clone();
    assert_eq!(cwd, dir.path().join("app"));
    let s = hub.store.session("map-lane-app-docs").await.unwrap().unwrap();
    assert_eq!((s.role, s.project.as_deref()), (Role::Manual, Some("app")));
    let cfg = hub.lanes.config("map-lane-app-docs").unwrap();
    assert_eq!((cfg.status_prompt.as_str(), cfg.owned.as_ref().unwrap().kind.as_str()), ("next card", "lane"));

    // After a restart of MAP: rebuilt from the database.
    hub.lanes.set_owned(vec![]);
    sync_lanes(&hub).await;
    assert!(hub.lanes.config("map-lane-app-docs").is_some());
    let lanes = project_lanes(&hub, &slug).await.unwrap();
    assert_eq!((lanes.len(), lanes[0].stopped, lanes[0].project.as_deref()), (1, false, Some("app")));

    // Gone: shown stopped, not relaunched by itself; Start brings it back.
    term.exit("map-lane-app-docs");
    crate::lanes::tick(&hub, Utc::now()).await;
    assert!(project_lanes(&hub, &slug).await.unwrap()[0].stopped);
    assert!(!term.sessions.lock().unwrap().contains_key("map-lane-app-docs"));
    start_lane(&hub, &slug, "docs").await.unwrap();
    assert!(!project_lanes(&hub, &slug).await.unwrap()[0].stopped);

    // A tell is typed only when idle, by the lane tick, into MAP's own terminal.
    term.set_pane("map-lane-app-docs", "● done\n\n──────────────\n❯ \n──────────────\n  ? for shortcuts\n");
    crate::lanes::tell(&hub, "map-lane-app-docs", crate::model::TellVerb::Ask, Some("check the API docs"), None, None, Utc::now()).await.unwrap();
    crate::lanes::tick(&hub, Utc::now()).await;
    assert!(term.sent("map-lane-app-docs").iter().any(|t| t == "check the API docs"), "{:?}", term.sent("map-lane-app-docs"));

    // Close: refused while attached, unless forced.
    term.sessions.lock().unwrap().get_mut("map-lane-app-docs").unwrap().attached = 1;
    assert_eq!(close_lane(&hub, &slug, "docs", false).await.unwrap(), EndDecision::KeepAttached);
    assert_eq!(close_lane(&hub, &slug, "docs", true).await.unwrap(), EndDecision::End);
    assert!(hub.store.owned_lanes(Some(&slug)).await.unwrap().is_empty());
    assert!(hub.lanes.config("map-lane-app-docs").is_none());
    assert_eq!(hub.store.session("map-lane-app-docs").await.unwrap().unwrap().state, SessionState::Exited);
}

#[test]
fn manager_scope_matrix() {
    use axum::http::Method;
    use crate::api::manager_scope;
    let allowed = [
        (Method::GET, "/api/projects/app"),
        (Method::GET, "/api/projects/app/overview"),
        (Method::GET, "/api/projects/app/orchestrator"),
        (Method::GET, "/api/projects/app/manager"),
        (Method::GET, "/api/projects/app/tasks"),
        (Method::GET, "/api/projects/app/tasks/12"),
        (Method::GET, "/api/projects/app/tasks/12/trace"),
        (Method::POST, "/api/projects/app/tasks"),
        (Method::PATCH, "/api/projects/app/tasks/12"),
        (Method::DELETE, "/api/projects/app/tasks/12"),
        (Method::GET, "/api/projects/app/plans"),
        (Method::POST, "/api/projects/app/plans"),
        (Method::POST, "/api/projects/app/plans/3/approve"),
        (Method::POST, "/api/projects/app/plans/3/reject"),
        (Method::POST, "/api/projects/app/pause"),
        (Method::POST, "/api/projects/app/resume"),
        (Method::GET, "/api/projects/app/knowledge"),
        (Method::POST, "/api/projects/app/knowledge/accept"),
        (Method::POST, "/api/projects/app/knowledge/dismiss"),
        (Method::GET, "/api/projects/app/knowledge/search"),
        (Method::GET, "/api/projects/app/decisions"),
        (Method::GET, "/api/projects/app/decisions/summary"),
        (Method::GET, "/api/projects/app/lanes"),
        (Method::POST, "/api/projects/app/lanes"),
        (Method::POST, "/api/projects/app/lanes/docs/tell"),
        (Method::POST, "/api/projects/app/lanes/docs/start"),
        (Method::GET, "/api/projects/app/lanes/docs/screen"),
        (Method::DELETE, "/api/projects/app/lanes/docs"),
        (Method::GET, "/api/projects/app/git"),
        (Method::POST, "/api/projects/app/git/fetch"),
        (Method::POST, "/api/projects/app/git/push"),
        (Method::POST, "/api/projects/app/decide"),
        (Method::GET, "/api/projects/app/activity"),
    ];
    for (m, p) in &allowed {
        assert_eq!(manager_scope(m, p), Some("app"), "{m} {p}");
    }
    let forbidden = [
        (Method::PUT, "/api/projects/app/manager"),
        (Method::POST, "/api/projects/app/manager/start"),
        (Method::DELETE, "/api/projects/app"),
        (Method::PATCH, "/api/projects/app"),
        (Method::POST, "/api/projects/app/git/pull"),
        (Method::POST, "/api/projects/app/git/remote"),
        (Method::POST, "/api/projects/app/init"),
        (Method::POST, "/api/projects/app/reflex/restart"),
        (Method::PUT, "/api/projects/app/repos/code/file"),
        (Method::POST, "/api/projects/app/trust"),
        (Method::GET, "/api/projects/app/tasks/12x"),
        (Method::GET, "/api/projects"),
        (Method::GET, "/api/projects/defaults"),
        (Method::GET, "/api/tasks"),
        (Method::POST, "/api/tasks"),
        (Method::POST, "/api/plans/3/approve"),
        (Method::GET, "/api/settings"),
        (Method::GET, "/api/telegram"),
        (Method::POST, "/api/telegram/test"),
        (Method::GET, "/api/providers/claude/login"),
        (Method::POST, "/api/providers/claude/update"),
        (Method::POST, "/api/orchestrator"),
        (Method::GET, "/api/lanes"),
        (Method::POST, "/api/sessions"),
        (Method::GET, "/api/git/access"),
        (Method::GET, "/api/fs/roots"),
        (Method::GET, "/api/sessions/map-mgr-app/trace"),
        (Method::GET, "/api/projects/app/activity/x"),
    ];
    for (m, p) in &forbidden {
        assert_eq!(manager_scope(m, p), None, "{m} {p}");
    }
}

/// The router on a loopback port, with `api_token = "secret"`.
async fn serve(hub: Arc<Hub>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = crate::api::router(hub);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/api")
}

#[tokio::test]
async fn manager_token_opens_its_project_only() {
    let (hub, _term, dir) = hub("").await;
    let a = project(&hub, &dir.path().join("aa"), "Alpha").await;
    let b = project(&hub, &dir.path().join("bb"), "Beta").await;
    let base = serve(hub.clone()).await;
    let http = reqwest::Client::new();
    let tok = hub.manager_token(&a).unwrap();
    let call = |m: reqwest::Method, path: &str, body: Option<serde_json::Value>, token: &str| {
        let mut r = http.request(m, format!("{base}{path}")).bearer_auth(token);
        if let Some(b) = body {
            r = r.json(&b);
        }
        r.send()
    };
    let get = |p: &str, t: &str| call(reqwest::Method::GET, p, None, t);
    assert_eq!(get(&format!("/projects/{a}/overview"), &tok).await.unwrap().status(), 200);
    assert_eq!(get(&format!("/projects/{b}/overview"), &tok).await.unwrap().status(), 403, "another project");
    assert_eq!(get("/settings", &tok).await.unwrap().status(), 403, "global settings");
    assert_eq!(get("/telegram", &tok).await.unwrap().status(), 403);
    assert_eq!(get("/tasks", &tok).await.unwrap().status(), 403);
    assert_eq!(get(&format!("/projects/{a}/overview"), "wrong").await.unwrap().status(), 401);
    assert_eq!(get(&format!("/projects/{a}/overview"), &hub.project_token(&a).unwrap()).await.unwrap().status(), 403, "a worker's project token is not a manager's");
    assert_eq!(call(reqwest::Method::DELETE, &format!("/projects/{a}"), None, &tok).await.unwrap().status(), 403, "never delete the project");
    assert_eq!(call(reqwest::Method::PUT, &format!("/projects/{a}/manager"), Some(serde_json::json!({"enabled": true})), &tok).await.unwrap().status(), 403);

    // Create, edit, cancel, delete a task; pause and resume.
    let r = call(reqwest::Method::POST, &format!("/projects/{a}/tasks"), Some(serde_json::json!({"title": "T", "prompt": "Goal: x"})), &tok).await.unwrap();
    assert_eq!(r.status(), 200);
    let t: crate::model::Task = r.json().await.unwrap();
    assert_eq!(t.project.as_deref(), Some(a.as_str()));
    assert!(kinds(&hub, "task.created").await >= 1);
    let r = call(reqwest::Method::PATCH, &format!("/projects/{a}/tasks/{}", t.id), Some(serde_json::json!({"title": "T2"})), &tok).await.unwrap();
    assert_eq!(r.status(), 200);
    let r = call(reqwest::Method::PATCH, &format!("/projects/{a}/tasks/{}", t.id), Some(serde_json::json!({"action": "merge"})), &tok).await.unwrap();
    assert_eq!(r.status(), 403, "merging is the owner's");
    assert_eq!(get(&format!("/projects/{b}/tasks/{}", t.id), "secret").await.unwrap().status(), 404, "the task is not b's");
    let r = call(reqwest::Method::PATCH, &format!("/projects/{a}/tasks/{}", t.id), Some(serde_json::json!({"action": "cancel"})), &tok).await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(call(reqwest::Method::DELETE, &format!("/projects/{a}/tasks/{}", t.id), None, &tok).await.unwrap().status(), 200);
    assert_eq!(call(reqwest::Method::POST, &format!("/projects/{a}/pause"), Some(serde_json::json!({})), &tok).await.unwrap().status(), 200);
    assert!(hub.store.project(&a).await.unwrap().unwrap().settings.paused);
    assert_eq!(call(reqwest::Method::POST, &format!("/projects/{a}/resume"), Some(serde_json::json!({})), &tok).await.unwrap().status(), 200);
    // A lane: opened by the manager, closing with force is the owner's.
    let r = call(reqwest::Method::POST, &format!("/projects/{a}/lanes"), Some(serde_json::json!({"name": "docs"})), &tok).await.unwrap();
    assert_eq!(r.status(), 201);
    assert_eq!(call(reqwest::Method::DELETE, &format!("/projects/{a}/lanes/docs?force=true"), None, &tok).await.unwrap().status(), 403);
    assert_eq!(call(reqwest::Method::DELETE, &format!("/projects/{a}/lanes/docs"), None, &tok).await.unwrap().status(), 200);
    // The owner's token opens the same routes.
    assert_eq!(get(&format!("/projects/{b}/overview"), "secret").await.unwrap().status(), 200);
    let _ = Path::new("/");
}

/// A fake provider (a shell that waits for input) on a private tmux socket: the manager starts there with the
/// supervisor role and its manager token, comes back after being killed, and the token is refused elsewhere.
#[tokio::test]
async fn integration_private_tmux() {
    if std::process::Command::new("tmux").arg("-V").output().is_err() {
        eprintln!("tmux missing: skipped");
        return;
    }
    let socket = format!("map-mgr-test-{}", std::process::id());
    let tmux = crate::lanes::tests::PrivateTmux(socket.clone());
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("fake-claude.sh");
    // Records its environment's token kinds (never the values) and its arguments, then waits.
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\n[ -n \"$MAP_MANAGER_TOKEN\" ] && [ \"$MAP_MANAGER_TOKEN\" != \"$MAP_PROJECT_TOKEN\" ] && echo manager-token >> {d}/env.log\necho \"$@\" | head -c 200 >> {d}/args.log; echo >> {d}/args.log\nwhile read -r line; do echo \"$line\" >> {d}/typed.log; done\n",
            d = dir.path().display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&fake, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let text = format!(
        "[projects]\npath = \"{d}\"\n[workspace]\ndir = \"{d}\"\ntmux_socket = \"{socket}\"\n[server]\ndata_dir = \"{d}/data\"\napi_token = \"secret\"\n[agents_home]\nmode = \"isolated\"\ndir = \"{d}/homes\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"{f}\"\ncredentials = []\nstartup_secs = 0\n[templates]\ndirs = []\n",
        d = dir.path().display(),
        f = fake.display()
    );
    let config = crate::config::Config::parse(&text, Path::new("/")).unwrap();
    config.create_agent_homes().unwrap();
    let term: Arc<dyn crate::terminal::Terminal> = Arc::new(crate::terminal::tmux::Tmux::new(Some(socket.clone())));
    let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), term.clone());
    let a = project(&hub, &dir.path().join("aa"), "Alpha").await;
    let b = project(&hub, &dir.path().join("bb"), "Beta").await;
    enable(&hub, &a).await;
    sync_lanes(&hub).await;

    supervise(&hub, Utc::now()).await;
    let name = session_name(&hub, &a);
    assert!(term.exists(&name).await.unwrap(), "started on the private socket");
    let s = hub.store.session(&name).await.unwrap().unwrap();
    assert_eq!((s.role, s.task_id), (Role::Supervisor, None));
    let mut seen = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if std::fs::read_to_string(dir.path().join("env.log")).is_ok_and(|l| l.contains("manager-token")) {
            seen = true;
            break;
        }
    }
    assert!(seen, "the manager token (its own kind) is in the session's environment");

    // Killed on the private socket only: the supervisor brings it back.
    term.kill(&name).await.unwrap();
    assert!(!term.exists(&name).await.unwrap());
    supervise(&hub, Utc::now()).await;
    assert!(term.exists(&name).await.unwrap(), "restarted");
    assert_eq!(kinds(&hub, "manager.restarting").await, 1);

    // The manager's token from inside: its project yes, another no.
    let base = serve(hub.clone()).await;
    let tok = hub.manager_token(&a).unwrap();
    let http = reqwest::Client::new();
    assert_eq!(http.get(format!("{base}/projects/{a}/overview")).bearer_auth(&tok).send().await.unwrap().status(), 200);
    assert_eq!(http.get(format!("{base}/projects/{b}/overview")).bearer_auth(&tok).send().await.unwrap().status(), 403);
    term.kill(&name).await.unwrap();
    drop(tmux);
}
