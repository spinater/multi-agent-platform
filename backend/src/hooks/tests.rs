use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::config::Config;
use crate::model::{ProjectSettings, ProjectSource, SessionState, SourceKind, TaskStatus};
use crate::orchestrator::Orchestrator;
use crate::store::{NewTask, Store};
use crate::terminal::fake::FakeTerminal;

/// A hub on temporary folders only: isolated agent homes under the temp dir, `claude` is `/bin/sh` (installed),
/// no audit, `HOOK_TEST_VAR` in `[workspace.env]`. `orch` goes in `[orchestrator]`, `extra` at the end.
pub(crate) async fn hub(orch: &str, extra: &str) -> (Arc<Hub>, Arc<FakeTerminal>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let text = format!(
        "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[workspace.env]\nHOOK_TEST_VAR = \"from-workspace\"\n[server]\ndata_dir = \"{0}/data\"\napi_token = \"secret\"\n[agents_home]\nmode = \"isolated\"\ndir = \"{0}/agent-home\"\n[orchestrator]\nprovider_order = [\"claude\"]\n{orch}\n[orchestrator.audit]\nenabled = false\n[reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"/bin/sh\"\ncredentials = []\n[providers.codex]\nbinary = \"/nonexistent/codex\"\n[providers.gemini]\nbinary = \"/nonexistent/agy\"\n{extra}\n",
        dir.path().display()
    );
    let config = Config::parse(&text, Path::new("/")).unwrap();
    assert!(config.agents_home.dir().starts_with(dir.path()), "agent homes must stay in the temp dir");
    let term = Arc::new(FakeTerminal::default());
    (Hub::new(config, Store::memory().await.unwrap(), term.clone()), term, dir)
}

/// A project on its own folder `<tmp>/<name>`; returns the slug.
pub(crate) async fn project(hub: &Hub, folder: &Path, name: &str) -> String {
    std::fs::create_dir_all(folder).unwrap();
    let settings = ProjectSettings { source: ProjectSource { kind: SourceKind::Local, location: Some(folder.display().to_string()), ..Default::default() }, ..Default::default() };
    let new = crate::projects::NewProject { name: name.into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
    crate::projects::create_with(hub, new, settings).await.unwrap().0.slug
}

fn hook(id: &str, event: HookEvent, command: &str, blocking: bool) -> Hook {
    Hook { id: id.into(), event, command: command.into(), blocking, timeout_secs: 10, enabled: true }
}

fn new_task(title: &str, project: Option<&str>) -> NewTask {
    NewTask { title: title.into(), prompt: format!("do {title}"), depends_on: vec![], provider: None, model: None, project: project.map(String::from), workflow: None }
}

async fn kinds(hub: &Hub, id: i64) -> Vec<String> {
    let mut kinds: Vec<String> = hub.store.decision_events(id, 500).await.unwrap().into_iter().map(|e| e.kind.trim_start_matches("decision.").to_string()).collect();
    kinds.reverse();
    kinds
}

/// Waits (up to 5s) until `f` holds.
async fn until(mut f: impl AsyncFnMut() -> bool) {
    for _ in 0..250 {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("condition not met within 5s");
}

async fn wait_status(hub: &Hub, id: i64, status: TaskStatus) -> Task {
    until(async || hub.store.task(id).await.unwrap().unwrap().status == status).await;
    hub.store.task(id).await.unwrap().unwrap()
}

#[test]
fn project_hooks_follow_the_machine_ones_and_replace_them_by_id() {
    let machine = vec![hook("a", HookEvent::PreLaunch, "one", false), hook("b", HookEvent::PostDone, "two", false)];
    let project = vec![hook("c", HookEvent::OnFail, "three", false), hook("b", HookEvent::PostDone, "two, the project's way", true)];
    let merged: Vec<(String, String, ConfigSource)> = merge(&machine, &project).into_iter().map(|(h, s)| (h.id, h.command, s)).collect();
    assert_eq!(
        merged,
        [
            ("a".into(), "one".into(), ConfigSource::Machine),
            ("b".into(), "two, the project's way".into(), ConfigSource::Override),
            ("c".into(), "three".into(), ConfigSource::Project),
        ]
    );
}

#[test]
fn validation_needs_a_command_a_sane_timeout_and_unique_ids() {
    let ok = hook("x", HookEvent::PreLaunch, "true", false);
    assert!(validate(std::slice::from_ref(&ok)).is_ok());
    let err = |h: Vec<Hook>| format!("{:#}", validate(&h).unwrap_err());
    assert!(err(vec![Hook { command: "  ".into(), ..ok.clone() }]).contains("needs a command"));
    assert!(err(vec![Hook { timeout_secs: 0, ..ok.clone() }]).contains("1 to 3600"));
    assert!(err(vec![Hook { timeout_secs: 3601, ..ok.clone() }]).contains("1 to 3600"));
    assert!(err(vec![ok.clone(), ok.clone()]).contains("used twice"));
    assert!(err(vec![Hook { id: "toml:post_done:1".into(), ..ok.clone() }]).contains("read-only"));
    assert!(err(vec![Hook { id: "has space".into(), ..ok.clone() }]).contains("letters"));

    // New hooks get ids; toml hooks sent back are dropped; commands are trimmed.
    let out = normalize(vec![
        hook("", HookEvent::PreLaunch, " bun run lint ", true),
        hook("", HookEvent::PreLaunch, "b", false),
        hook("toml:post_done:1", HookEvent::PostDone, "git commit", true),
        hook("keep", HookEvent::OnFail, "c", false),
    ])
    .unwrap();
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].command, "bun run lint");
    assert!(out[0].id.starts_with("pre_launch-") && out[1].id.starts_with("pre_launch-") && out[0].id != out[1].id, "{out:?}");
    assert_eq!(out[2].id, "keep");
}

#[tokio::test]
async fn overview_lists_toml_then_machine_then_project_hooks_with_their_source() {
    let (hub, _, dir) = hub("post_done = [\"git commit -am {title}\"]", "").await;
    let slug = project(&hub, &dir.path().join("app"), "App").await;
    save(&hub, None, vec![hook("lint", HookEvent::PreLaunch, "bun run lint", true), hook("ping", HookEvent::OnFail, "notify", false)]).await.unwrap();
    save(&hub, Some(&slug), vec![hook("lint", HookEvent::PreLaunch, "cargo clippy", true), hook("", HookEvent::OnStall, "say stalled", false)]).await.unwrap();

    let machine = overview(&hub, None).await.unwrap();
    let rows: Vec<(&str, ConfigSource, bool)> = machine.hooks.iter().map(|h| (h.command.as_str(), h.source, h.read_only)).collect();
    assert_eq!(rows, [("git commit -am {title}", ConfigSource::Machine, true), ("bun run lint", ConfigSource::Machine, false), ("notify", ConfigSource::Machine, false)]);
    assert_eq!(machine.hooks[0].id, "toml:post_done:1");
    assert_eq!(machine.events.len(), 5);

    let p = overview(&hub, Some(&slug)).await.unwrap();
    let rows: Vec<(&str, ConfigSource)> = p.hooks.iter().map(|h| (h.command.as_str(), h.source)).collect();
    assert_eq!(rows, [("git commit -am {title}", ConfigSource::Machine), ("cargo clippy", ConfigSource::Override), ("notify", ConfigSource::Machine), ("say stalled", ConfigSource::Project)]);
    // Saved in the project's settings.
    assert_eq!(hub.store.project(&slug).await.unwrap().unwrap().settings.hooks.len(), 2);
}

#[tokio::test]
async fn a_command_reports_exit_output_and_masks_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let cmd = "echo first; for i in $(seq 1 30); do echo line$i; done; echo API_TOKEN=abc123secret >&2; exit 3";
    let run = run_command(dir.path(), cmd, Duration::from_secs(10), &[]).await.unwrap();
    assert_eq!((run.exit_code, run.timed_out, run.ok()), (Some(3), false, false));
    let lines: Vec<&str> = run.output_tail.lines().collect();
    assert_eq!(lines.len(), 20, "{}", run.output_tail);
    assert_eq!(*lines.last().unwrap(), "API_TOKEN=***");
    assert!(!run.output_tail.contains("abc123secret") && !run.output_tail.contains("first"));

    let ok = run_command(dir.path(), "pwd; echo $X", Duration::from_secs(10), &[("X".into(), "y".into())]).await.unwrap();
    assert!(ok.ok());
    assert!(ok.output_tail.ends_with("\ny"), "{}", ok.output_tail);
}

#[tokio::test]
async fn a_command_past_its_timeout_is_killed_with_what_it_started() {
    let dir = tempfile::tempdir().unwrap();
    let started = std::time::Instant::now();
    let run = run_command(dir.path(), "echo begun; sleep 30 & sleep 30", Duration::from_secs(1), &[]).await.unwrap();
    assert!(run.timed_out && run.exit_code.is_none(), "{run:?}");
    assert!(started.elapsed() < Duration::from_secs(6), "took {:?}", started.elapsed());
    assert!(run.output_tail.starts_with("begun") && run.output_tail.ends_with("timed out after 1s"), "{}", run.output_tail);
}

#[tokio::test]
async fn a_blocking_pre_launch_failure_fails_the_task_without_a_session() {
    let (hub, term, dir) = hub("max_attempts = 3", "").await;
    let marker = dir.path().join("on_fail.txt");
    save(
        &hub,
        None,
        vec![
            hook("", HookEvent::PreLaunch, "echo checking; exit 4", true),
            hook("", HookEvent::OnFail, &format!("echo \"$MAP_EVENT $MAP_TASK_ID\" > {}", marker.display()), false),
        ],
    )
    .await
    .unwrap();
    let t = hub.store.create_task(new_task("a", None)).await.unwrap();
    Orchestrator::new(hub.clone()).tick().await.unwrap();
    let after = hub.store.task(t.id).await.unwrap().unwrap();
    assert_eq!(after.status, TaskStatus::Failed, "no retry: a person decides");
    let result = after.result.unwrap();
    assert!(result.starts_with("pre_launch hook failed: echo checking; exit 4: exit 4"), "{result}");
    assert!(result.contains("checking"), "{result}");
    assert!(term.sessions.lock().unwrap().is_empty(), "no agent session was started");
    let k = kinds(&hub, t.id).await;
    assert!(k.contains(&"hook.pre_launch.fail".to_string()) && k.contains(&"hook.pre_launch.blocked".to_string()), "{k:?}");
    assert!(!k.contains(&"launch.started".to_string()), "{k:?}");
    // on_fail runs in the background, with the task in its env.
    until(async || std::fs::read_to_string(&marker).is_ok_and(|text| text.trim() == format!("on_fail {}", t.id))).await;
    until(async || kinds(&hub, t.id).await.contains(&"hook.on_fail.pass".to_string())).await;
}

#[tokio::test]
async fn a_non_blocking_pre_launch_failure_only_logs() {
    let (hub, term, _dir) = hub("", "").await;
    save(&hub, None, vec![hook("", HookEvent::PreLaunch, "exit 1", false)]).await.unwrap();
    let t = hub.store.create_task(new_task("a", None)).await.unwrap();
    Orchestrator::new(hub.clone()).tick().await.unwrap();
    let after = hub.store.task(t.id).await.unwrap().unwrap();
    assert_eq!(after.status, TaskStatus::Running);
    assert_eq!(term.sessions.lock().unwrap().len(), 1);
    let k = kinds(&hub, t.id).await;
    assert!(k.contains(&"hook.pre_launch.fail".to_string()) && !k.contains(&"hook.pre_launch.blocked".to_string()), "{k:?}");
}

#[tokio::test]
async fn hooks_get_the_task_env_and_the_workspace_env_in_the_task_folder() {
    let (hub, term, dir) = hub("", "").await;
    let folder = dir.path().join("app");
    let slug = project(&hub, &folder, "App").await;
    save(&hub, Some(&slug), vec![hook("", HookEvent::PreLaunch, "env | grep -E '^(MAP_|HOOK_TEST_VAR)' | sort > env.txt; pwd > pwd.txt", true)]).await.unwrap();
    let t = hub.store.create_task(new_task("Fix the login", Some(&slug))).await.unwrap();
    Orchestrator::new(hub.clone()).tick().await.unwrap();
    assert_eq!(hub.store.task(t.id).await.unwrap().unwrap().status, TaskStatus::Running);
    let env = std::fs::read_to_string(folder.join("env.txt")).unwrap();
    let session = term.sessions.lock().unwrap().keys().next().cloned().unwrap();
    for line in ["MAP_EVENT=pre_launch".to_string(), format!("MAP_TASK_ID={}", t.id), "MAP_TASK_TITLE=Fix the login".into(), format!("MAP_PROJECT={slug}"), format!("MAP_SESSION={session}"), "HOOK_TEST_VAR=from-workspace".into()] {
        assert!(env.lines().any(|l| l == line), "{line} missing in\n{env}");
    }
    assert!(!env.contains("MAP_BRANCH="), "no branch in a shared folder: {env}");
    assert_eq!(std::fs::read_to_string(folder.join("pwd.txt")).unwrap().trim(), folder.canonicalize().unwrap().display().to_string());
}

#[tokio::test]
async fn a_blocking_post_verify_failure_fails_the_task_without_a_retry() {
    let (hub, _term, _dir) = hub("max_attempts = 3\nverify = [\"true\"]", "").await;
    save(&hub, None, vec![hook("", HookEvent::PostVerify, "exit 7", true)]).await.unwrap();
    let t = hub.store.create_task(new_task("a", None)).await.unwrap();
    let mut orch = Orchestrator::new(hub.clone());
    orch.tick().await.unwrap();
    let s = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
    std::fs::write(hub.result_file(&s), r#"{"status":"done","summary":"x"}"#).unwrap();
    orch.tick().await.unwrap();
    let after = wait_status(&hub, t.id, TaskStatus::Failed).await;
    assert_eq!(after.attempts, 1);
    assert!(after.result.unwrap().starts_with("post_verify hook failed: exit 7: exit 7"));
    let k = kinds(&hub, t.id).await;
    let i = k.iter().position(|k| k == "verify.pass").unwrap();
    assert_eq!(&k[i + 1..i + 3], ["hook.post_verify.fail", "hook.post_verify.blocked"], "{k:?}");
    assert!(!k.iter().any(|k| k.starts_with("audit.") || k.starts_with("retry.")), "{k:?}");
}

#[tokio::test]
async fn post_done_runs_the_toml_list_then_machine_then_project_hooks() {
    // Every post_done command runs in the task's folder, so a relative file collects them all.
    let (hub, _term, dir) = hub("post_done = [\"echo toml >> order.txt\"]", "").await;
    let log = |w: &str| format!("echo {w} >> order.txt");
    let order = dir.path().join("app/order.txt");
    let folder = dir.path().join("app");
    let slug = project(&hub, &folder, "App").await;
    save(&hub, None, vec![hook("", HookEvent::PostDone, &log("machine"), true), hook("off", HookEvent::PostDone, &log("disabled"), true)]).await.unwrap();
    let mut machine = machine_hooks(&hub).await.unwrap();
    machine[1].enabled = false;
    save(&hub, None, machine).await.unwrap();
    save(&hub, Some(&slug), vec![hook("", HookEvent::PostDone, &log("project"), true)]).await.unwrap();
    let t = hub.store.create_task(new_task("a", Some(&slug))).await.unwrap();
    let mut orch = Orchestrator::new(hub.clone());
    orch.tick().await.unwrap();
    let s = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
    std::fs::write(hub.result_file(&s), r#"{"status":"done","summary":"x"}"#).unwrap();
    orch.tick().await.unwrap();
    wait_status(&hub, t.id, TaskStatus::Done).await;
    assert_eq!(std::fs::read_to_string(&order).unwrap(), "toml\nmachine\nproject\n");
    let k = kinds(&hub, t.id).await;
    assert_eq!(k.iter().filter(|k| *k == "hook.post_done.pass").count(), 2, "{k:?}");
}

#[tokio::test]
async fn a_blocking_post_done_hook_failure_fails_the_task() {
    let (hub, _term, dir) = hub("", "").await;
    let marker = dir.path().join("on_fail.txt");
    save(&hub, None, vec![hook("", HookEvent::PostDone, "exit 2", true), hook("", HookEvent::OnFail, &format!("touch {}", marker.display()), false)]).await.unwrap();
    let t = hub.store.create_task(new_task("a", None)).await.unwrap();
    let mut orch = Orchestrator::new(hub.clone());
    orch.tick().await.unwrap();
    let s = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
    std::fs::write(hub.result_file(&s), r#"{"status":"done","summary":"x"}"#).unwrap();
    orch.tick().await.unwrap();
    let after = wait_status(&hub, t.id, TaskStatus::Failed).await;
    assert!(after.result.unwrap().starts_with("post_done hook failed: exit 2: exit 2"));
    assert!(kinds(&hub, t.id).await.contains(&"post.fail".to_string()));
    until(async || marker.exists()).await;
}

#[tokio::test]
async fn on_fail_runs_when_the_last_attempt_fails() {
    let (hub, _term, dir) = hub("max_attempts = 1", "").await;
    let marker = dir.path().join("failed.txt");
    save(&hub, None, vec![hook("", HookEvent::OnFail, &format!("echo \"$MAP_EVENT:$MAP_TASK_TITLE\" > {}", marker.display()), false)]).await.unwrap();
    let t = hub.store.create_task(new_task("Broken", None)).await.unwrap();
    let mut orch = Orchestrator::new(hub.clone());
    orch.tick().await.unwrap();
    let s = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
    std::fs::write(hub.result_file(&s), r#"{"status":"failed","summary":"cannot"}"#).unwrap();
    orch.tick().await.unwrap();
    assert_eq!(hub.store.task(t.id).await.unwrap().unwrap().status, TaskStatus::Failed);
    until(async || std::fs::read_to_string(&marker).is_ok_and(|t| t.trim() == "on_fail:Broken")).await;
}

#[tokio::test]
async fn on_stall_runs_once_when_the_watcher_marks_a_task_session_stalled() {
    let (hub, term, dir) = hub("", "[watcher]\nstall_minutes = 1\n").await;
    let marker = dir.path().join("stalls.txt");
    save(&hub, None, vec![hook("", HookEvent::OnStall, &format!("echo \"$MAP_SESSION\" >> {}", marker.display()), false)]).await.unwrap();
    let t = hub.store.create_task(new_task("a", None)).await.unwrap();
    Orchestrator::new(hub.clone()).tick().await.unwrap();
    let s = hub.store.task(t.id).await.unwrap().unwrap().session.unwrap();
    term.set_pane(&s, "some output\n");
    let mut watcher = crate::watcher::Watcher::new(hub.clone());
    let now = chrono::Utc::now();
    watcher.tick(now).await.unwrap();
    for minutes in [5, 6, 7] {
        watcher.tick(now + chrono::Duration::minutes(minutes)).await.unwrap();
    }
    assert_eq!(hub.store.session(&s).await.unwrap().unwrap().state, SessionState::Stalled);
    until(async || std::fs::read_to_string(&marker).is_ok_and(|t| t.trim() == s)).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(std::fs::read_to_string(&marker).unwrap().lines().count(), 1, "once per stall");
    assert_eq!(hub.store.task(t.id).await.unwrap().unwrap().status, TaskStatus::Running, "on_stall never changes the task");
}

#[tokio::test]
async fn the_test_run_uses_the_project_folder_and_a_timeout() {
    let (hub, _term, dir) = hub("", "").await;
    let folder = dir.path().join("app");
    let slug = project(&hub, &folder, "App").await;
    let run = test(&hub, Some(&slug), "pwd; echo $MAP_EVENT $MAP_PROJECT", 5).await.unwrap();
    assert!(run.ok());
    assert_eq!(run.output_tail, format!("{}\ntest {slug}", folder.canonicalize().unwrap().display()));
    let slow = test(&hub, None, "sleep 20", 1).await.unwrap();
    assert!(slow.timed_out && !slow.ok());
    assert!(test(&hub, None, "true", 0).await.is_err());
    assert!(test(&hub, None, " ", 5).await.is_err());
}
