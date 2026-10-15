use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::model::{McpValueInput, McpView};
use crate::permissions::tests::{json_of, setup, write};

fn input(name: &str, command: &str, args: &[&str], env: &[(&str, Option<&str>)]) -> McpServerInput {
    McpServerInput {
        name: name.into(),
        transport: McpTransport::Stdio,
        command: Some(command.into()),
        args: args.iter().map(|a| a.to_string()).collect(),
        url: None,
        env: env.iter().map(|(k, v)| McpValueInput { key: k.to_string(), value: v.map(String::from) }).collect(),
        headers: vec![],
    }
}

fn server<'a>(v: &'a McpView, name: &str) -> &'a McpServer {
    v.servers.iter().find(|s| s.name == name).unwrap_or_else(|| panic!("no server {name}"))
}

fn not_tested(_: &str) -> McpStatus {
    status_of(None)
}

#[tokio::test]
async fn reads_every_format_and_keeps_unknown_keys_on_write() {
    let s = setup().await;
    let c = &s.hub.config;
    let claude_json = s.ah.path().join("claude/.claude.json");
    write(&claude_json, r#"{"numStartups": 7, "projects": {"/x": {"hasTrustDialogAccepted": true, "mcpServers": {"local": {"command": "l"}}}}, "mcpServers": {"github": {"type": "stdio", "command": "gh-mcp", "args": ["serve"], "env": {"GITHUB_TOKEN": "ghp_secret_value"}}}}"#);
    let codex = s.ah.path().join("codex/config.toml");
    write(&codex, "# my codex\nmodel = \"gpt-6-sol\" # keep\n\n[mcp_servers.github]\ncommand = \"gh-mcp\"\nargs = [\"serve\"]\nstartup_timeout_sec = 20\n\n[mcp_servers.github.env]\nGITHUB_TOKEN = \"ghp_secret_value\"\n\n[mcp_servers.docs]\nurl = \"https://docs.example/mcp\"\nbearer_token_env_var = \"DOCS_TOKEN\"\n");
    let gemini = s.ah.path().join("gemini-home/.gemini/settings.json");
    write(&gemini, r#"{"theme": "dark", "mcpServers": {"web": {"httpUrl": "https://web.example/mcp", "headers": {"Authorization": "Bearer xyz"}, "trust": true}}}"#);

    let v = view(&collect(c, ConfigScope::Machine, &s.project), ConfigScope::Machine, not_tested);
    let gh = server(&v, "github");
    assert_eq!((gh.transport, gh.command.as_deref(), gh.args.clone()), (McpTransport::Stdio, Some("gh-mcp"), vec!["serve".to_string()]));
    assert_eq!(gh.providers, ["codex", "claude"]);
    assert_eq!(gh.env_keys, [McpKey { key: "GITHUB_TOKEN".into(), set: true }]);
    assert_eq!(gh.source, ConfigSource::Machine);
    assert_eq!(gh.status.label, "not tested");
    let docs = server(&v, "docs");
    assert_eq!((docs.transport, docs.url.as_deref()), (McpTransport::Http, Some("https://docs.example/mcp")));
    assert!(docs.notes.iter().any(|n| n.contains("DOCS_TOKEN")));
    let web = server(&v, "web");
    assert_eq!((web.providers.clone(), web.header_keys.clone()), (vec!["gemini".to_string()], vec![McpKey { key: "Authorization".into(), set: true }]));
    assert!(v.servers.iter().all(|s| s.name != "local"), "Claude's per-folder servers are not machine servers");
    // Values never leave: not in the serialized view.
    let text = serde_json::to_string(&v).unwrap();
    assert!(!text.contains("ghp_secret_value") && !text.contains("Bearer xyz"), "{text}");

    // Update github everywhere with a new arg, keeping its token (value: None).
    let new = input("github", "gh-mcp", &["serve", "--read-only"], &[("GITHUB_TOKEN", None), ("GH_HOST", Some("github.com"))]);
    let all = ["codex".to_string(), "claude".to_string(), "gemini".to_string()];
    let written = save(c, ConfigScope::Machine, &s.project, &new, &all).unwrap();
    assert_eq!(written.len(), 3, "{written:?}");
    let j = json_of(&claude_json);
    assert_eq!(j["numStartups"], 7);
    assert_eq!(j["projects"]["/x"]["mcpServers"]["local"]["command"], "l");
    assert_eq!(j["mcpServers"]["github"], json!({"type": "stdio", "command": "gh-mcp", "args": ["serve", "--read-only"], "env": {"GITHUB_TOKEN": "ghp_secret_value", "GH_HOST": "github.com"}}));
    let t = std::fs::read_to_string(&codex).unwrap();
    assert!(t.starts_with("# my codex\nmodel = \"gpt-6-sol\" # keep\n"), "{t}");
    let doc: toml::Table = toml::from_str(&t).unwrap();
    let gh = &doc["mcp_servers"]["github"];
    assert_eq!(gh["args"].as_array().unwrap().len(), 2);
    assert_eq!(gh["startup_timeout_sec"].as_integer(), Some(20));
    assert_eq!(gh["env"]["GITHUB_TOKEN"].as_str(), Some("ghp_secret_value"));
    assert_eq!(doc["mcp_servers"]["docs"]["bearer_token_env_var"].as_str(), Some("DOCS_TOKEN"));
    let g = json_of(&gemini);
    assert_eq!(g["theme"], "dark");
    assert_eq!(g["mcpServers"]["web"]["trust"], true);
    assert_eq!(g["mcpServers"]["github"]["command"], "gh-mcp");

    // Only Claude now: removed from Codex's and Gemini's files.
    save(c, ConfigScope::Machine, &s.project, &new, &["claude".to_string()]).unwrap();
    let v = view(&collect(c, ConfigScope::Machine, &s.project), ConfigScope::Machine, not_tested);
    assert_eq!(server(&v, "github").providers, ["claude"]);
    assert!(!std::fs::read_to_string(&codex).unwrap().contains("[mcp_servers.github]"));

    // A missing env value with nothing to keep is refused.
    let err = save(c, ConfigScope::Machine, &s.project, &input("fresh", "x", &[], &[("TOKEN", None)]), &all).unwrap_err();
    assert!(format!("{err:#}").contains("needs a value"));
    assert!(save(c, ConfigScope::Machine, &s.project, &input("bad name", "x", &[], &[]), &all).is_err());
    assert!(save(c, ConfigScope::Machine, &s.project, &input("ok", "x", &[], &[]), &["copilot".to_string()]).is_err());
}

#[tokio::test]
async fn project_servers_replace_the_machine_ones_and_delete_per_scope() {
    let s = setup().await;
    let c = &s.hub.config;
    let claude_json = s.ah.path().join("claude/.claude.json");
    write(&claude_json, r#"{"mcpServers": {"github": {"command": "gh-mcp"}, "pg": {"command": "pg-mcp"}}}"#);
    let codex_project = s.project.join(".codex/config.toml");

    let all = ["codex".to_string(), "claude".to_string()];
    save(c, ConfigScope::Project, &s.project, &input("github", "gh-mcp", &["--project"], &[]), &all).unwrap();
    save(c, ConfigScope::Project, &s.project, &input("lint", "lint-mcp", &[], &[]), &["codex".to_string()]).unwrap();
    assert_eq!(json_of(&s.project.join(".mcp.json"))["mcpServers"]["github"]["args"], json!(["--project"]));
    let text = std::fs::read_to_string(&codex_project).unwrap();
    assert!(text.starts_with("[mcp_servers.github]\ncommand = \"gh-mcp\"\nargs = [\"--project\"]\n"), "{text}");
    assert!(text.contains("[mcp_servers.lint]"));
    assert_eq!(json_of(&claude_json)["mcpServers"]["github"], json!({"command": "gh-mcp"}), "the machine file is untouched");

    let v = view(&collect(c, ConfigScope::Project, &s.project), ConfigScope::Project, not_tested);
    let gh = server(&v, "github");
    assert_eq!((gh.source, gh.args.clone()), (ConfigSource::Override, vec!["--project".to_string()]));
    assert_eq!(server(&v, "pg").source, ConfigSource::Machine);
    assert_eq!(server(&v, "lint").source, ConfigSource::Project);
    assert_eq!(server(&v, "lint").providers, ["codex"]);
    assert!(v.files.iter().any(|f| f.path == codex_project.display().to_string() && f.scope == ConfigScope::Project));

    // Deleting in project scope removes the project definitions; the machine one shows again.
    let removed = delete(c, ConfigScope::Project, &s.project, "github").unwrap();
    assert_eq!(removed.len(), 2, "{removed:?}");
    let v = view(&collect(c, ConfigScope::Project, &s.project), ConfigScope::Project, not_tested);
    let gh = server(&v, "github");
    assert_eq!((gh.source, gh.args.is_empty()), (ConfigSource::Machine, true));
    assert!(delete(c, ConfigScope::Project, &s.project, "nope").unwrap().is_empty());
    assert_eq!(delete(c, ConfigScope::Machine, &s.project, "pg").unwrap(), vec![claude_json.clone()]);
    assert!(json_of(&claude_json)["mcpServers"].get("pg").is_none());
}

#[tokio::test]
async fn http_servers_are_written_in_each_dialect() {
    let s = setup().await;
    let c = &s.hub.config;
    let mut i = input("remote", "", &[], &[]);
    i.transport = McpTransport::Http;
    i.command = None;
    i.url = Some("https://mcp.example/v1".into());
    i.headers = vec![McpValueInput { key: "X-Api-Key".into(), value: Some("k-123456".into()) }];
    save(c, ConfigScope::Machine, &s.project, &i, &["codex".to_string(), "claude".to_string(), "gemini".to_string()]).unwrap();
    assert_eq!(json_of(&s.ah.path().join("claude/.claude.json"))["mcpServers"]["remote"], json!({"type": "http", "url": "https://mcp.example/v1", "headers": {"X-Api-Key": "k-123456"}}));
    assert_eq!(json_of(&s.ah.path().join("gemini-home/.gemini/settings.json"))["mcpServers"]["remote"], json!({"httpUrl": "https://mcp.example/v1", "headers": {"X-Api-Key": "k-123456"}}));
    let doc: toml::Table = toml::from_str(&std::fs::read_to_string(s.ah.path().join("codex/config.toml")).unwrap()).unwrap();
    assert_eq!(doc["mcp_servers"]["remote"]["url"].as_str(), Some("https://mcp.example/v1"));
    assert_eq!(doc["mcp_servers"]["remote"]["http_headers"]["X-Api-Key"].as_str(), Some("k-123456"));
    let v = view(&collect(c, ConfigScope::Machine, &s.project), ConfigScope::Machine, not_tested);
    let r = server(&v, "remote");
    assert!(r.notes.is_empty(), "{:?}", r.notes);
    assert!(!serde_json::to_string(&v).unwrap().contains("k-123456"));
}

// ---- the connection test

/// A tiny MCP server over stdio: answers `initialize` and two pages of `tools/list`.
const FAKE_SERVER: &str = r#"#!/bin/bash
echo "fake server starting" >&2
while IFS= read -r line; do
  id=""
  [[ $line =~ \"id\":([0-9]+) ]] && id=${BASH_REMATCH[1]}
  case "$line" in
    *'"method":"initialize"'*)
      echo '{"jsonrpc":"2.0","method":"notifications/message","params":{"level":"info","data":"hi"}}'
      echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{\"tools\":{}},\"serverInfo\":{\"name\":\"fake\",\"version\":\"1\"}}}" ;;
    *'"method":"tools/list"'*)
      if [[ $line == *cursor* ]]; then
        echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"third\"}]}}"
      else
        echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"echo_${MCP_MODE}\"},{\"name\":\"add\"}],\"nextCursor\":\"p2\"}}"
      fi ;;
  esac
done
"#;

fn script(dir: &Path, name: &str, text: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn stdio_def(command: &Path, env: &[(&str, &str)]) -> Def {
    Def {
        transport: McpTransport::Stdio,
        command: Some(command.display().to_string()),
        args: vec![],
        url: None,
        env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
        headers: BTreeMap::new(),
        sse: false,
        bearer_env: None,
        expands: true,
    }
}

fn no_env(_: &str) -> Option<String> {
    None
}

#[tokio::test]
async fn a_stdio_server_lists_its_tools() {
    let dir = tempfile::tempdir().unwrap();
    let path = script(dir.path(), "fake-mcp", FAKE_SERVER);
    let def = stdio_def(&path, &[("MCP_MODE", "${MODE_FROM_MAP:-plain}")]);
    let http = reqwest::Client::new();
    let r = test(&def, dir.path(), Duration::from_secs(10), &http, &|v| (v == "MODE_FROM_MAP").then(|| "loud".to_string())).await;
    assert!(r.ok, "{r:?}");
    assert_eq!((r.tools, r.tool_names.clone()), (3, vec!["echo_loud".to_string(), "add".into(), "third".into()]));
    assert!(r.error.is_none());
}

#[tokio::test]
async fn a_failing_server_reports_why_without_its_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let path = script(dir.path(), "broken-mcp", "#!/bin/bash\necho \"bad token: $API_TOKEN\" >&2\nexit 3\n");
    let def = stdio_def(&path, &[("API_TOKEN", "tok-very-secret")]);
    let r = test(&def, dir.path(), Duration::from_secs(10), &reqwest::Client::new(), &no_env).await;
    assert!(!r.ok);
    let err = r.error.unwrap();
    assert!(err.contains("bad token: ***") && !err.contains("tok-very-secret"), "{err}");

    let missing = stdio_def(&dir.path().join("does-not-exist"), &[]);
    let r = test(&missing, dir.path(), Duration::from_secs(10), &reqwest::Client::new(), &no_env).await;
    assert!(!r.ok && r.error.unwrap().contains("could not start"));
}

#[tokio::test]
async fn a_silent_server_times_out_and_its_whole_group_is_ended() {
    let dir = tempfile::tempdir().unwrap();
    let pids = dir.path().join("pids");
    let path = script(dir.path(), "silent-mcp", &format!("#!/bin/bash\nsleep 300 &\necho $! > {0}\necho $$ >> {0}\nwhile IFS= read -r line; do :; done\nwait\n", pids.display()));
    let started = std::time::Instant::now();
    let r = test(&stdio_def(&path, &[]), dir.path(), Duration::from_millis(800), &reqwest::Client::new(), &no_env).await;
    assert!(!r.ok && r.error.as_deref().unwrap().contains("no answer within"), "{r:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let pids: Vec<i32> = std::fs::read_to_string(&pids).unwrap().lines().map(|l| l.trim().parse().unwrap()).collect();
    assert_eq!(pids.len(), 2);
    // Reaped child and its background sleep: both gone (allow a moment for the kernel).
    for _ in 0..50 {
        // SAFETY: signal 0 only checks that the pid exists; these are the processes this test started.
        if pids.iter().all(|p| unsafe { libc::kill(*p, 0) } != 0) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("a process of the test server is still running: {pids:?}");
}

/// A streamable-HTTP MCP server on a loopback port: JSON for `initialize` (with a session id), 202 for the
/// notification, an SSE stream for `tools/list` that stays open after the answer. Requires the session id.
async fn http_server() -> String {
    use axum::http::{HeaderMap, StatusCode};
    use axum::response::IntoResponse;
    async fn handle(headers: HeaderMap, body: String) -> axum::response::Response {
        let msg: Value = serde_json::from_str(&body).unwrap();
        let method = msg["method"].as_str().unwrap_or_default();
        let session = headers.get("mcp-session-id").and_then(|v| v.to_str().ok());
        if headers.get("x-api-key").and_then(|v| v.to_str().ok()) != Some("key-from-env") {
            return (StatusCode::UNAUTHORIZED, "no key").into_response();
        }
        match method {
            "initialize" => ([("mcp-session-id", "s-1"), ("content-type", "application/json")], json!({"jsonrpc": "2.0", "id": msg["id"], "result": {"protocolVersion": "2025-06-18", "capabilities": {}}}).to_string()).into_response(),
            _ if session != Some("s-1") => (StatusCode::BAD_REQUEST, "no session").into_response(),
            "notifications/initialized" => StatusCode::ACCEPTED.into_response(),
            "tools/list" => {
                let first = format!("event: message\ndata: {}\n\n", json!({"jsonrpc": "2.0", "method": "notifications/progress", "params": {}}));
                let answer = format!("event: message\ndata: {}\n\n", json!({"jsonrpc": "2.0", "id": msg["id"], "result": {"tools": [{"name": "search"}, {"name": "fetch"}]}}));
                let stream = futures::stream::iter(vec![Ok::<_, std::io::Error>(first), Ok(answer)]).chain(futures::stream::pending());
                ([("content-type", "text/event-stream")], axum::body::Body::from_stream(stream)).into_response()
            }
            _ => StatusCode::NOT_FOUND.into_response(),
        }
    }
    use futures::StreamExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().route("/mcp", axum::routing::post(handle)).route("/slow", axum::routing::post(|| async { std::future::pending::<String>().await }));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

#[tokio::test]
async fn an_http_server_lists_its_tools_over_json_and_sse() {
    let base = http_server().await;
    let mut def = stdio_def(Path::new("/unused"), &[]);
    def.transport = McpTransport::Http;
    def.command = None;
    def.url = Some(format!("{base}/mcp"));
    def.headers.insert("X-Api-Key".into(), "${MCP_TEST_KEY}".into());
    let lookup = |v: &str| (v == "MCP_TEST_KEY").then(|| "key-from-env".to_string());
    let r = test(&def, Path::new("/"), Duration::from_secs(10), &reqwest::Client::new(), &lookup).await;
    assert!(r.ok, "{r:?}");
    assert_eq!(r.tool_names, ["search", "fetch"]);

    // Without the key: the HTTP status, nothing else.
    let r = test(&def, Path::new("/"), Duration::from_secs(10), &reqwest::Client::new(), &no_env).await;
    assert!(!r.ok && r.error.as_deref().unwrap().contains("401"), "{r:?}");

    def.url = Some(format!("{base}/slow"));
    let r = test(&def, Path::new("/"), Duration::from_millis(500), &reqwest::Client::new(), &lookup).await;
    assert!(!r.ok && r.error.as_deref().unwrap().contains("no answer within"), "{r:?}");
}
