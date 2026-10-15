use serde_json::{Value, json};

use crate::hooks::tests::{hub, project};

/// Serves the real router (token and Host checks included) on a free loopback port.
async fn serve(hub: std::sync::Arc<crate::hub::Hub>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, super::super::router(hub)).await.unwrap() });
    format!("http://{addr}/api")
}

async fn call(method: &str, url: &str, body: Option<Value>) -> (u16, Value) {
    let mut req = reqwest::Client::new().request(method.parse().unwrap(), url).bearer_auth("secret");
    if let Some(b) = body {
        req = req.json(&b);
    }
    let res = req.send().await.unwrap();
    let status = res.status().as_u16();
    (status, res.json().await.unwrap_or(Value::Null))
}

fn paths(v: &Value) -> Vec<String> {
    v["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap().to_string()).collect()
}

fn find<'a>(v: &'a Value, path: &str) -> &'a Value {
    v["files"].as_array().unwrap().iter().find(|f| f["path"] == path).unwrap_or_else(|| panic!("{path} not in {:?}", paths(v)))
}

#[tokio::test]
async fn rules_endpoints_list_write_toggle_readers_and_delete() {
    let (hub, _, dir) = hub("", "").await;
    let folder = dir.path().join("app");
    let slug = project(&hub, &folder, "App").await;
    std::fs::write(folder.join("AGENTS.md"), "Shared rules.\n").unwrap();
    std::fs::write(folder.join("CLAUDE.md"), "Claude only.\n").unwrap();
    let base = serve(hub.clone()).await;

    let (status, o) = call("GET", &format!("{base}/rules?project={slug}"), None).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(find(&o, "AGENTS.md")["readers"], json!(["codex"]));
    assert_eq!(find(&o, "AGENTS.md")["source"], "project");
    assert_eq!(o["files"].as_array().unwrap().iter().filter(|f| f["source"] == "machine").count(), 3);

    let (status, o) = call("PUT", &format!("{base}/rules/readers"), Some(json!({"project": slug, "path": "AGENTS.md", "readers": ["claude", "codex"]}))).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(find(&o, "AGENTS.md")["readers"], json!(["claude", "codex"]));
    assert_eq!(std::fs::read_to_string(folder.join("CLAUDE.md")).unwrap(), "Claude only.\n@AGENTS.md\n");

    let (status, o) = call("PUT", &format!("{base}/rules"), Some(json!({"project": slug, "path": "docs/rules/db.md", "body": "Migrations only.\n"}))).await;
    assert_eq!(status, 200, "{o}");
    assert_eq!(find(&o, "docs/rules/db.md")["readers"], json!([]));
    assert_eq!(find(&o, "docs/rules/db.md")["lines"], 1);

    let (status, err) = call("PUT", &format!("{base}/rules"), Some(json!({"project": slug, "path": "../escape.md", "body": "x"}))).await;
    assert_eq!(status, 400);
    assert!(err["error"].as_str().unwrap().contains(".."), "{err}");
    assert!(!dir.path().join("escape.md").exists());
    let (status, _) = call("PUT", &format!("{base}/rules"), Some(json!({"path": "CLAUDE.md", "body": "x"}))).await;
    assert_eq!(status, 400, "machine scope takes only the machine files");
    let (status, _) = call("GET", &format!("{base}/rules?project=nope"), None).await;
    assert_eq!(status, 400);

    let (status, o) = call("DELETE", &format!("{base}/rules"), Some(json!({"project": slug, "path": "docs/rules/db.md"}))).await;
    assert_eq!(status, 200, "{o}");
    assert!(!paths(&o).contains(&"docs/rules/db.md".to_string()));

    // Machine scope: only the CLIs' own files, in the temp agent homes.
    let (status, o) = call("GET", &format!("{base}/rules"), None).await;
    assert_eq!(status, 200);
    assert_eq!(o["files"].as_array().unwrap().len(), 3);
    assert!(o["files"].as_array().unwrap().iter().all(|f| f["abs_path"].as_str().unwrap().starts_with(dir.path().to_str().unwrap())));

    let res = reqwest::Client::new().get(format!("{base}/rules")).send().await.unwrap();
    assert_eq!(res.status().as_u16(), 401, "the API token is required");
}

#[tokio::test]
async fn hooks_endpoints_save_merge_and_test() {
    let (hub, _, dir) = hub("post_done = [\"git commit -am {title}\"]", "").await;
    let folder = dir.path().join("app");
    let slug = project(&hub, &folder, "App").await;
    let base = serve(hub.clone()).await;

    let machine = json!({"hooks": [
        {"id": "", "event": "pre_launch", "command": "bun run lint", "blocking": true, "timeout_secs": 60, "enabled": true},
        {"id": "notify", "event": "on_fail", "command": "notify-send failed", "blocking": false, "timeout_secs": 10, "enabled": true},
        // The toml hooks come back from GET; PUT skips them.
        {"id": "toml:post_done:1", "event": "post_done", "command": "git commit -am {title}", "blocking": true, "timeout_secs": 600, "enabled": true}
    ]});
    let (status, o) = call("PUT", &format!("{base}/hooks"), Some(machine)).await;
    assert_eq!(status, 200, "{o}");
    let hooks = o["hooks"].as_array().unwrap();
    assert_eq!(hooks.len(), 3);
    assert_eq!((hooks[0]["id"].as_str().unwrap(), hooks[0]["read_only"].as_bool().unwrap()), ("toml:post_done:1", true));
    assert!(hooks[1]["id"].as_str().unwrap().starts_with("pre_launch-"));
    assert_eq!(o["events"].as_array().unwrap().len(), 5);

    let project_hooks = json!({"project": slug, "hooks": [
        {"id": "notify", "event": "on_fail", "command": "echo project-notify", "blocking": false, "timeout_secs": 10, "enabled": true},
        {"event": "post_verify", "command": "cargo audit", "blocking": true}
    ]});
    let (status, o) = call("PUT", &format!("{base}/hooks"), Some(project_hooks)).await;
    assert_eq!(status, 200, "{o}");
    let rows: Vec<(String, String)> = o["hooks"].as_array().unwrap().iter().map(|h| (h["command"].as_str().unwrap().into(), h["source"].as_str().unwrap().into())).collect();
    assert_eq!(
        rows,
        [
            ("git commit -am {title}".into(), "machine".into()),
            ("bun run lint".into(), "machine".into()),
            ("echo project-notify".into(), "override".into()),
            ("cargo audit".into(), "project".into())
        ]
    );
    let (_, again) = call("GET", &format!("{base}/hooks?project={slug}"), None).await;
    assert_eq!(again["hooks"], o["hooks"]);
    let (_, m) = call("GET", &format!("{base}/hooks"), None).await;
    assert_eq!(m["hooks"].as_array().unwrap().len(), 3, "machine scope: machine hooks only");

    // A project PATCH with settings does not drop the hooks.
    let (_, info) = call("GET", &format!("{base}/projects/{slug}"), None).await;
    let mut settings = info["project"]["settings"].clone();
    settings["hooks"] = json!([]);
    settings["paused"] = json!(true);
    let (status, body) = call("PATCH", &format!("{base}/projects/{slug}"), Some(json!({"settings": settings}))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(hub.store.project(&slug).await.unwrap().unwrap().settings.hooks.len(), 2);

    for bad in [
        json!({"hooks": [{"event": "pre_launch", "command": " "}]}),
        json!({"hooks": [{"event": "pre_launch", "command": "x", "timeout_secs": 0}]}),
        json!({"hooks": [{"event": "pre_launch", "command": "x", "timeout_secs": 4000}]}),
        json!({"hooks": [{"id": "a", "event": "pre_launch", "command": "x"}, {"id": "a", "event": "on_fail", "command": "y"}]}),
    ] {
        let (status, err) = call("PUT", &format!("{base}/hooks"), Some(bad.clone())).await;
        assert_eq!(status, 400, "{bad} → {err}");
    }
    let (status, _) = call("PUT", &format!("{base}/hooks"), Some(json!({"hooks": [{"event": "on_launch", "command": "x"}]}))).await;
    assert_eq!(status, 422, "unknown event");

    let (status, run) = call("POST", &format!("{base}/hooks/test"), Some(json!({"project": slug, "command": "echo hi; exit 2", "timeout_secs": 5}))).await;
    assert_eq!(status, 200, "{run}");
    assert_eq!((run["exit_code"].clone(), run["timed_out"].clone(), run["output_tail"].clone()), (json!(2), json!(false), json!("hi")));
    let (status, run) = call("POST", &format!("{base}/hooks/test"), Some(json!({"command": "sleep 20", "timeout_secs": 1}))).await;
    assert_eq!(status, 200, "{run}");
    assert_eq!((run["exit_code"].clone(), run["timed_out"].clone()), (Value::Null, json!(true)));
    assert!(run["duration_ms"].as_u64().unwrap() < 5000);
}
