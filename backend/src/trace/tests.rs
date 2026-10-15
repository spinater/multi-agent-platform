//! Trace parsing (Claude and Codex fixtures), aggregation, secret masking, incremental reads, and the activity
//! graph of a full chain: a manager-created task → intake → a Claude agent → verify → audit → done → unblocks a
//! dependent task → a Codex agent.

use std::sync::Arc;

use chrono::Utc;
use serde_json::json;

use super::*;
use crate::model::{Level, ManagerSettings, Role, SessionState};
use crate::plans::tests::{hub, project};
use crate::store::NewTask;

fn tool(name: &str, input: Value) -> String {
    json!({ "type": "assistant", "timestamp": "2026-10-15T10:00:00Z", "message": { "role": "assistant", "content": [
        { "type": "text", "text": "ok" }, { "type": "tool_use", "id": "t", "name": name, "input": input }
    ] } })
    .to_string()
}

#[test]
fn masks_secrets_but_keeps_variables() {
    let m = mask("curl -H \"Authorization: Bearer xyz123secret\" $MAP_API_URL/projects/a/tasks");
    assert!(!m.contains("xyz123secret") && m.contains("Bearer ***") && m.contains("$MAP_API_URL"), "{m}");
    assert_eq!(mask("GITHUB_TOKEN=ghp_abcdefghijklmnop1234 gh pr list"), "GITHUB_TOKEN=*** gh pr list");
    assert!(!mask("mysql -u root:hunter2 db").contains("hunter2"));
    assert!(!mask("tool --api-key sk-abcdefghijklmnopqrstuv run").contains("sk-abc"));
    assert!(!mask("git clone https://me:pa55word@github.com/x/y").contains("pa55word"));
    assert!(!mask("echo 0123456789abcdef0123456789abcdef01").contains("0123456789abcdef"));
    assert!(mask(&"x ".repeat(400)).chars().count() <= 300);
}

#[test]
fn command_heads_group_invocations() {
    assert_eq!(command_head("cargo test --all"), "cargo test");
    assert_eq!(command_head("cd backend && cargo clippy --all-targets -- -D warnings"), "cargo clippy");
    assert_eq!(command_head("RUST_LOG=debug timeout 60 cargo run -- serve"), "cargo run");
    assert_eq!(command_head("bun run lint 2>&1 | tail"), "bun run lint");
    assert_eq!(command_head("bash scripts/verify.sh"), "bash scripts/verify.sh");
    assert_eq!(command_head("git commit -m 'x'"), "git commit");
    assert_eq!(command_head("curl -fsS \"$P/tasks/12\" -H \"$H\""), "curl $P/tasks/{id}");
    assert_eq!(command_head("curl -s http://127.0.0.1:4400/api/projects/app/overview?x=1"), "curl /api/projects/app/overview");
    assert_eq!(command_head("ls -la"), "ls");
    assert_eq!(command_head("/usr/bin/python3 tools/gen.py --out x"), "python3 tools/gen.py");
}

#[test]
fn claude_and_codex_fixtures_become_items() {
    let claude = [
        json!({ "type": "custom-title", "customTitle": "map-a", "sessionId": "s" }).to_string(),
        tool("Bash", json!({ "command": "cargo test" })),
        tool("Bash", json!({ "command": "cargo test -- lanes" })),
        tool("Bash", json!({ "command": "curl -H \"Authorization: Bearer abc.def\" $MAP_KNOWLEDGE_URL/search?q=refund" })),
        tool("Skill", json!({ "skill": "code-review" })),
        tool("Read", json!({ "file_path": "/w/app/.claude/skills/deploy/SKILL.md" })),
        tool("Read", json!({ "file_path": "/w/app/.docs/knowledge/billing.md" })),
        tool("Read", json!({ "file_path": "/w/app/src/main.rs" })),
        tool("Agent", json!({ "subagent_type": "Explore", "description": "find the API" })),
        tool("Edit", json!({ "file_path": "/w/app/src/main.rs" })),
        "{not json".to_string(),
    ]
    .join("\n");
    let mut items = vec![];
    aggregate(&mut items, parse_claude(&claude));
    let find = |k: &str, n: &str| items.iter().find(|i| i.kind == k && i.name == n).cloned();
    let t = find("command", "cargo test").unwrap();
    assert_eq!((t.count, t.samples.len()), (2, 2));
    assert!(find("skill", "code-review").is_some() && find("skill", "deploy").is_some());
    assert!(find("knowledge", ".docs/knowledge/billing.md").is_some());
    assert!(find("knowledge", "MAP knowledge search").is_some(), "{items:?}");
    assert!(find("subagent", "Explore").is_some());
    assert_eq!(find("tool", "Edit").unwrap().count, 1);
    assert_eq!(find("tool", "Read").unwrap().count, 1, "a plain file read is a tool use");
    assert!(items.iter().all(|i| !i.detail.contains("abc.def")), "masked");

    let codex = [
        json!({ "timestamp": "2026-10-15T10:00:00Z", "type": "session_meta", "payload": { "cwd": "/w/app", "timestamp": "2026-10-15T10:00:00Z" } }).to_string(),
        json!({ "timestamp": "2026-10-15T10:01:00Z", "type": "response_item", "payload": { "type": "function_call", "name": "exec_command", "arguments": "{\"cmd\":\"cargo build\"}" } }).to_string(),
        json!({ "timestamp": "2026-10-15T10:02:00Z", "type": "response_item", "payload": { "type": "function_call", "name": "shell", "arguments": "{\"command\":[\"bash\",\"-lc\",\"git commit -m wip\"]}" } }).to_string(),
        json!({ "timestamp": "2026-10-15T10:03:00Z", "type": "response_item", "payload": { "type": "custom_tool_call", "name": "apply_patch", "input": "*** Begin Patch" } }).to_string(),
        json!({ "timestamp": "2026-10-15T10:04:00Z", "type": "response_item", "payload": { "type": "function_call", "name": "exec_command", "arguments": "{\"cmd\":\"cat .agents/skills/release/SKILL.md\"}" } }).to_string(),
    ]
    .join("\n");
    let mut items = vec![];
    aggregate(&mut items, parse_codex(&codex));
    let names: Vec<(String, String)> = items.iter().map(|i| (i.kind.clone(), i.name.clone())).collect();
    for want in [("command", "cargo build"), ("command", "git commit"), ("tool", "apply_patch"), ("skill", "release"), ("command", "cat")] {
        assert!(names.contains(&(want.0.to_string(), want.1.to_string())), "{want:?} in {names:?}");
    }
    assert_eq!(items[0].first_at.unwrap().to_rfc3339(), "2026-10-15T10:01:00+00:00");
}

#[test]
fn command_outputs_follow_their_samples() {
    let call = |id: &str, cmd: &str| {
        json!({ "type": "assistant", "timestamp": "2026-10-15T10:00:00Z", "message": { "role": "assistant", "content": [
            { "type": "tool_use", "id": id, "name": "Bash", "input": { "command": cmd } }
        ] } })
        .to_string()
    };
    let result = |id: &str, content: Value| {
        json!({ "type": "user", "message": { "role": "user", "content": [ { "type": "tool_result", "tool_use_id": id, "content": content } ] } }).to_string()
    };
    let claude = [
        call("a", "cargo test"),
        result("a", json!("Compiling map\n\nrunning 3 tests\ntest a ... ok\ntest result: ok. 3 passed; 0 failed")),
        call("b", "cargo test -- lanes"),
        call("c", "curl -H \"Authorization: Bearer abc.def\" $U/x"),
        result("c", json!([{ "type": "text", "text": "token=ghp_abcdefghijklmnop1234" }])),
    ]
    .join("\n");
    let mut items = vec![];
    aggregate(&mut items, parse_claude(&claude));
    let t = items.iter().find(|i| i.name == "cargo test").unwrap();
    assert_eq!(t.samples, vec!["cargo test", "cargo test -- lanes"]);
    assert_eq!(t.outputs, vec!["running 3 tests\ntest a ... ok\ntest result: ok. 3 passed; 0 failed".to_string(), String::new()], "last 3 lines; none for a call without a result");
    let c = items.iter().find(|i| i.kind == "command" && i.name.starts_with("curl")).unwrap();
    assert!(!c.outputs[0].contains("ghp_abc") && c.outputs[0].contains("***"), "{:?}", c.outputs);

    let codex = [
        json!({ "type": "response_item", "payload": { "type": "function_call", "name": "exec_command", "call_id": "k", "arguments": "{\"cmd\":\"cargo build\"}" } }).to_string(),
        json!({ "type": "response_item", "payload": { "type": "function_call_output", "call_id": "k", "output": "{\"output\":\"Finished dev\\n\",\"metadata\":{\"exit_code\":0}}" } }).to_string(),
    ]
    .join("\n");
    let mut items = vec![];
    aggregate(&mut items, parse_codex(&codex));
    assert_eq!(items[0].outputs, vec!["Finished dev"]);

    // Items stored before outputs existed line up again on the next sample.
    let mut old = vec![TraceItem { kind: "command".into(), name: "ls".into(), detail: "ls".into(), count: 1, first_at: None, last_at: None, samples: vec!["ls".into()], outputs: vec![] }];
    let mut r = raw("command", "ls", "ls -la", None);
    r.output = Some("x".into());
    aggregate(&mut old, vec![r]);
    assert_eq!(old[0].outputs, vec![String::new(), "x".into()]);
}

#[test]
fn aggregation_caps_items_and_samples() {
    let mut items = vec![];
    let many: Vec<Raw> = (0..300).map(|i| raw("command", format!("tool{i}"), "x", None)).collect();
    aggregate(&mut items, many);
    assert_eq!(items.len(), MAX_ITEMS);
    let mut one = vec![];
    aggregate(&mut one, (0..20).map(|i| raw("command", "cargo test", format!("cargo test {i}"), None)).collect());
    assert_eq!((one[0].count, one[0].samples.len(), one[0].samples.last().unwrap().as_str()), (20, MAX_SAMPLES, "cargo test 19"));
}

#[test]
fn reads_only_new_complete_lines() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("t.jsonl");
    std::fs::write(&f, "a\nb\npartial").unwrap();
    let (text, off) = read_new(&f, 0).unwrap();
    assert_eq!((text.as_str(), off), ("a\nb\n", 4));
    std::fs::write(&f, "a\nb\npartial line\nc\n").unwrap();
    let (text, off2) = read_new(&f, off).unwrap();
    assert_eq!(text, "partial line\nc\n");
    assert_eq!(read_new(&f, off2).unwrap().0, "");
    // A huge file: only its tail, starting at a whole line.
    let big = d.path().join("big.jsonl");
    std::fs::write(&big, "x".repeat(3 * 1024 * 1024) + "\nlast\n").unwrap();
    assert_eq!(read_new(&big, 0).unwrap().0, "last\n");
}

async fn session(hub: &Hub, name: &str, provider: &str, model: &str, task: Option<i64>, project: &str, state: SessionState) -> Session {
    let now = Utc::now();
    let s = Session {
        name: name.into(),
        provider: provider.into(),
        model: model.into(),
        role: if task.is_some() { Role::Worker } else { Role::Supervisor },
        task_id: task,
        project: Some(project.into()),
        state,
        detail: None,
        rate_limit_reset_at: None,
        wakeups: 0,
        last_line: None,
        attached: false,
        created_at: now,
        last_activity_at: now,
        spawned_by: if task.is_some() { "orchestrator".into() } else { "map".into() },
    };
    hub.store.save_session(&s).await.unwrap();
    s
}

/// The full chain, through the API's own builder, with transcripts in an isolated (temporary) agent home.
#[tokio::test]
async fn full_chain_from_the_manager_to_a_dependent_codex_agent() {
    let (hub, _term, dir) = hub("[agents_home]\nmode = \"isolated\"").await;
    let folder = dir.path().join("app");
    let slug = project(&hub, &folder, "App").await;
    let mut p = hub.store.project(&slug).await.unwrap().unwrap();
    p.settings.manager = ManagerSettings { enabled: true, provider: Some("claude".into()), model: None };
    hub.store.save_project(&p).await.unwrap();

    let new = |title: &str, deps: Vec<i64>| NewTask { title: title.into(), prompt: "Goal".into(), depends_on: deps, provider: None, model: None, project: Some(slug.clone()), workflow: None };
    let mut a = hub.store.create_task(new("Add the endpoint", vec![])).await.unwrap();
    hub.store.set_task_created_by(a.id, "manager").await.unwrap();
    let mut b = hub.store.create_task(new("Add the screen", vec![a.id])).await.unwrap();
    hub.store.set_task_created_by(b.id, "manager").await.unwrap();

    session(&hub, "map-mgr-app", "claude", "sonnet", None, &slug, SessionState::Idle).await;
    session(&hub, "map-claude-1", "claude", "sonnet", Some(a.id), &slug, SessionState::Exited).await;
    session(&hub, "map-codex-2", "codex", "gpt-6-sol", Some(b.id), &slug, SessionState::Working).await;
    let ev = |kind: &'static str, s: Option<&'static str>, t: i64, msg: &'static str| {
        let hub = Arc::clone(&hub);
        async move {
            hub.event(Level::Info, kind, s, Some(t), msg).await;
        }
    };
    ev("decision.intake.ready", None, a.id, "Reflex: ready (0.93)").await;
    ev("decision.ready.go", None, a.id, "no dependencies · slot 1/4").await;
    ev("decision.pick.tier_standard", None, a.id, "standard").await;
    ev("decision.pick.chosen", None, a.id, "order claude → codex").await;
    ev("session.started", Some("map-claude-1"), a.id, "Started claude (sonnet)").await;
    ev("decision.launch.started", Some("map-claude-1"), a.id, "map-claude-1").await;
    ev("decision.watch.done", Some("map-claude-1"), a.id, "reported done").await;
    ev("decision.verify.cmd_start", Some("map-claude-1"), a.id, "bash scripts/verify.sh").await;
    ev("decision.verify.pass", Some("map-claude-1"), a.id, "1 command passed").await;
    ev("decision.audit.ask", Some("map-claude-1"), a.id, "asking codex").await;
    ev("decision.audit.pass", Some("map-claude-1"), a.id, "codex: VERDICT: PASS").await;
    ev("decision.land.merged", Some("map-claude-1"), a.id, "fast-forwarded main").await;
    ev("decision.post.pass", Some("map-claude-1"), a.id, "no hooks").await;
    ev("decision.end.closed", Some("map-claude-1"), a.id, "session closed").await;
    a.status = TaskStatus::Done;
    hub.store.save_task(&a).await.unwrap();
    ev("decision.intake.ready", None, b.id, "Reflex: ready").await;
    ev("decision.pick.chosen", None, b.id, "order claude → codex · claude cooling down → codex").await;
    ev("session.started", Some("map-codex-2"), b.id, "Started codex (gpt-6-sol)").await;
    ev("decision.launch.started", Some("map-codex-2"), b.id, "map-codex-2").await;
    b.status = TaskStatus::Running;
    hub.store.save_task(&b).await.unwrap();

    // The Claude worker's transcript (found by its --name), and the Codex one (by folder and time).
    let claude_home = hub.provider_home("claude").unwrap();
    assert!(claude_home.starts_with(dir.path()), "never the real home");
    let tdir = crate::lanes::claude_project_dir(&claude_home, &folder);
    std::fs::create_dir_all(&tdir).unwrap();
    let lines = [json!({ "type": "custom-title", "customTitle": "map-claude-1" }).to_string(), tool("Bash", json!({ "command": "cargo test" })), tool("Skill", json!({ "skill": "tdd" }))];
    std::fs::write(tdir.join("w.jsonl"), lines.join("\n") + "\n").unwrap();
    std::fs::write(tdir.join("other.jsonl"), json!({ "type": "custom-title", "customTitle": "map-someone-else" }).to_string() + "\n" + &tool("Bash", json!({"command": "rm -rf /"})) + "\n").unwrap();
    let codex_home = hub.provider_home("codex").unwrap();
    assert!(codex_home.starts_with(dir.path()));
    let cdir = codex_home.join("sessions/2026/10/15");
    std::fs::create_dir_all(&cdir).unwrap();
    let meta = json!({ "timestamp": Utc::now().to_rfc3339(), "type": "session_meta", "payload": { "cwd": folder.display().to_string(), "timestamp": Utc::now().to_rfc3339() } });
    let call = json!({ "type": "response_item", "payload": { "type": "function_call", "name": "exec_command", "arguments": "{\"cmd\":\"bun run lint\"}" } });
    std::fs::write(cdir.join("rollout-1.jsonl"), format!("{meta}\n{call}\n")).unwrap();

    let g = activity(&hub, &slug, None, None).await.unwrap();
    let chain: Vec<(String, String, String)> = g.edges.iter().filter(|e| !e.from.starts_with("agent:") || !e.to.starts_with("u:")).filter(|e| !e.to.starts_with("u:")).map(|e| (e.from.clone(), e.to.clone(), e.kind.clone())).collect();
    let (ta, tb) = (format!("task:{}", a.id), format!("task:{}", b.id));
    let want: Vec<(String, String, String)> = [
        ("manager", ta.as_str(), "created"),
        (ta.as_str(), &format!("pick:{}:1", a.id), "picked"),
        (&format!("pick:{}:1", a.id), "agent:map-claude-1", "assigned"),
        ("agent:map-claude-1", &format!("step:{}:1:verify", a.id), "handed"),
        (&format!("step:{}:1:verify", a.id), &format!("step:{}:1:audit", a.id), "handed"),
        (&format!("step:{}:1:audit", a.id), &format!("step:{}:1:land", a.id), "handed"),
        (&format!("step:{}:1:land", a.id), &format!("step:{}:1:post", a.id), "handed"),
        (&format!("step:{}:1:post", a.id), &format!("step:{}:done", a.id), "handed"),
        ("manager", tb.as_str(), "created"),
        (tb.as_str(), &format!("pick:{}:1", b.id), "picked"),
        (&format!("pick:{}:1", b.id), "agent:map-codex-2", "assigned"),
        (&format!("step:{}:done", a.id), tb.as_str(), "unblocks"),
    ]
    .iter()
    .map(|(f, t, k)| (f.to_string(), t.to_string(), k.to_string()))
    .collect();
    assert_eq!(chain, want);

    let n = |id: &str| g.nodes.iter().find(|n| n.id == id).cloned().unwrap_or_else(|| panic!("no node {id}"));
    let pick = n(&format!("pick:{}:1", a.id));
    assert_eq!(pick.detail.as_deref(), Some("intake: ready · tier: standard · order claude → codex"));
    assert_eq!((n("agent:map-claude-1").provider.as_deref(), n("agent:map-codex-2").provider.as_deref()), (Some("claude"), Some("codex")));
    assert_eq!(n(&format!("step:{}:1:verify", a.id)).samples, vec!["bash scripts/verify.sh".to_string()]);
    assert_eq!(n(&format!("step:{}:done", a.id)).state.as_deref(), Some("ok"));
    assert_eq!(n("manager").state.as_deref(), Some("restarting"), "the manager's own state (on, not running in tmux here)");
    // Columns go left to right along the chain; each task has its own row.
    let layers: Vec<u32> = ["agent:map-claude-1", &format!("step:{}:1:verify", a.id), &format!("step:{}:done", a.id)].iter().map(|i| n(i).layer).collect();
    assert!(layers.windows(2).all(|w| w[0] < w[1]), "{layers:?}");
    assert_ne!(n(&ta).row, n(&tb).row);
    // Live: B's hand-off to its agent is in progress; A's chain is not.
    let live: Vec<&ActivityEdge> = g.edges.iter().filter(|e| e.live).collect();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].to, "agent:map-codex-2");
    // What each agent used hangs off it; another session's transcript is not mixed in.
    let used = |owner: &str| g.nodes.iter().filter(|x| x.parent.as_deref() == Some(owner)).map(|x| format!("{}:{}", x.kind, x.label)).collect::<Vec<_>>();
    assert_eq!(used("agent:map-claude-1"), vec!["command:cargo test", "skill:tdd"]);
    assert_eq!(used("agent:map-codex-2"), vec!["command:bun run lint"]);

    // Incremental: new lines are added to the counts; old ones are not read again.
    let more = tool("Bash", json!({ "command": "cargo test -- x" })) + "\n";
    let mut f = std::fs::OpenOptions::new().append(true).open(tdir.join("w.jsonl")).unwrap();
    std::io::Write::write_all(&mut f, more.as_bytes()).unwrap();
    let s = hub.store.session("map-claude-1").await.unwrap().unwrap();
    let t = refresh(&hub, &s).await;
    assert_eq!(t.items.iter().find(|i| i.name == "cargo test").unwrap().count, 2);
    assert_eq!(refresh(&hub, &s).await.items.iter().find(|i| i.name == "cargo test").unwrap().count, 2, "no double count");

    // One task and its neighbours.
    let only = activity(&hub, &slug, None, Some(b.id)).await.unwrap();
    assert!(only.nodes.iter().any(|n| n.id == ta) && only.nodes.iter().any(|n| n.id == tb));
    // Done tasks older than `since` drop out; open ones stay.
    let later = activity(&hub, &slug, Some(Utc::now() + chrono::Duration::hours(1)), None).await.unwrap();
    assert!(!later.nodes.iter().any(|n| n.id == ta) && later.nodes.iter().any(|n| n.id == tb));
}

/// A retry goes round again through a new pick and agent.
#[test]
fn a_retry_goes_round_again() {
    let now = Utc::now();
    let task = Task {
        id: 1,
        title: "T".into(),
        prompt: "p".into(),
        depends_on: vec![],
        provider: None,
        model: None,
        project: Some("app".into()),
        status: TaskStatus::Failed,
        session: None,
        attempts: 2,
        result: None,
        git: None,
        scope: None,
        intake: None,
        workflow: None,
        workflow_state: None,
        created_by: "owner".into(),
        created_at: now,
        updated_at: now,
    };
    let e = |id: i64, kind: &str, s: Option<&str>| Event { id, at: now, level: Level::Info, kind: kind.into(), session: s.map(String::from), task_id: Some(1), message: "m".into() };
    let events = vec![
        e(1, "session.started", Some("map-a")),
        e(2, "decision.verify.fail", Some("map-a")),
        e(3, "decision.retry.again", Some("map-a")),
        e(4, "session.started", Some("map-b")),
        e(5, "decision.watch.failed", Some("map-b")),
        e(6, "decision.retry.failed", Some("map-b")),
    ];
    let traces = HashMap::new();
    let g = build(&Input { slug: "app", prefix: "map-", tasks: &[task], sessions: &[], events: &events, traces: &traces, queries: &[], manager_state: None, since: None });
    let kinds: Vec<(String, String)> = g.edges.iter().map(|x| (x.to.clone(), x.kind.clone())).collect();
    assert_eq!(
        kinds,
        [("task:1", "created"), ("pick:1:1", "picked"), ("agent:map-a", "assigned"), ("step:1:1:verify", "handed"), ("pick:1:2", "retry"), ("agent:map-b", "assigned"), ("step:1:failed", "handed")]
            .map(|(a, b)| (a.to_string(), b.to_string()))
    );
    assert_eq!(g.nodes.iter().find(|n| n.id == "step:1:1:verify").unwrap().state.as_deref(), Some("fail"));
    assert!(g.nodes.iter().any(|n| n.id == "owner"));
}

#[tokio::test]
async fn api_shape_and_manager_token() {
    // Isolated: a trace lookup never reads the real home's transcripts.
    let (hub, _term, dir) = hub("[agents_home]\nmode = \"isolated\"").await;
    let slug = project(&hub, &dir.path().join("app"), "App").await;
    let t = hub.store.create_task(NewTask { title: "T".into(), prompt: "p".into(), depends_on: vec![], provider: None, model: None, project: Some(slug.clone()), workflow: None }).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}/api", listener.local_addr().unwrap());
    let app = crate::api::router(hub.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::new();
    let r = http.get(format!("{base}/projects/{slug}/activity")).bearer_auth(hub.manager_token(&slug).unwrap()).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(v["project"], slug.as_str());
    assert!(v["since"].is_string(), "default: the last 24 hours");
    let task = v["nodes"].as_array().unwrap().iter().find(|n| n["id"] == format!("task:{}", t.id)).unwrap().clone();
    for key in ["kind", "label", "count", "state", "layer", "row", "parent", "samples"] {
        assert!(task.get(key).is_some(), "{key}");
    }
    assert_eq!(v["edges"][0]["kind"], "created");
    let all: Value = http.get(format!("{base}/projects/{slug}/activity?all=true")).bearer_auth("secret").send().await.unwrap().json().await.unwrap();
    assert!(all["since"].is_null());
    assert_eq!(http.get(format!("{base}/projects/nope/activity")).bearer_auth("secret").send().await.unwrap().status(), 404);
    session(&hub, "map-x", "claude", "sonnet", None, &slug, SessionState::Idle).await;
    let tr: Value = http.get(format!("{base}/sessions/map-x/trace")).bearer_auth("secret").send().await.unwrap().json().await.unwrap();
    assert_eq!((tr["session"].as_str(), tr["items"].as_array().map(Vec::len)), (Some("map-x"), Some(0)));
    assert_eq!(http.get(format!("{base}/sessions/map-x/trace")).bearer_auth(hub.manager_token(&slug).unwrap()).send().await.unwrap().status(), 403, "not the manager's");
}
