//! `/api/permissions` and `/api/mcp*`: the Configure screen's Permissions and MCP tabs. `project` is a slug; empty
//! or missing is the machine scope (the CLIs' own files in their state folders).

use std::path::PathBuf;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;

use super::{ApiResult, known_project, not_found};
use crate::hub::Hub;
use crate::model::{ConfigScope, Level, McpNameRequest, McpSave, McpTestResult, McpView, PermissionsUpdate, PermissionsView};
use crate::{mcp, permissions};

#[derive(Deserialize)]
pub struct ScopeQuery {
    project: Option<String>,
}

/// The scope, the folder project paths are relative to, and the project slug.
async fn place(hub: &Hub, project: Option<String>) -> Result<(ConfigScope, PathBuf, Option<String>), super::ApiError> {
    let project = known_project(hub, project).await?;
    let scope = if project.is_some() { ConfigScope::Project } else { ConfigScope::Machine };
    Ok((scope, hub.work_dir(project.as_deref()).await, project))
}

fn where_(project: Option<&str>) -> String {
    match project {
        Some(p) => format!("project {p}"),
        None => "the machine".into(),
    }
}

fn paths(p: &[PathBuf]) -> String {
    if p.is_empty() { "nothing changed".into() } else { p.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ") }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> Result<T, super::ApiError> {
    Ok(tokio::task::spawn_blocking(f).await.map_err(anyhow::Error::from)??)
}

async fn permissions_view(hub: &Arc<Hub>, scope: ConfigScope, workspace: PathBuf, project: Option<String>) -> Result<PermissionsView, super::ApiError> {
    let h = hub.clone();
    let mut view = blocking(move || {
        let servers = mcp::names(&mcp::collect(&h.config, scope, &workspace));
        Ok(permissions::view(&h.config, scope, &workspace, &servers))
    })
    .await?;
    view.project = project;
    Ok(view)
}

pub async fn permissions(State(hub): State<Arc<Hub>>, Query(q): Query<ScopeQuery>) -> ApiResult<PermissionsView> {
    let (scope, workspace, project) = place(&hub, q.project).await?;
    Ok(Json(permissions_view(&hub, scope, workspace, project).await?))
}

pub async fn set_permissions(State(hub): State<Arc<Hub>>, Json(b): Json<PermissionsUpdate>) -> ApiResult<PermissionsView> {
    let (scope, workspace, project) = place(&hub, b.project).await?;
    let (h, ws, cells) = (hub.clone(), workspace.clone(), b.cells);
    let n = cells.len();
    let written = blocking(move || permissions::apply(&h.config, scope, &ws, &cells)).await?;
    hub.event(Level::Info, "permissions.saved", None, None, format!("Permissions for {}: {n} cell(s); {}", where_(project.as_deref()), paths(&written))).await;
    Ok(Json(permissions_view(&hub, scope, workspace, project).await?))
}

async fn mcp_view(hub: &Arc<Hub>, scope: ConfigScope, workspace: PathBuf, project: Option<String>) -> Result<McpView, super::ApiError> {
    let h = hub.clone();
    let key = mcp::scope_key(project.as_deref());
    let mut view = blocking(move || {
        let c = mcp::collect(&h.config, scope, &workspace);
        Ok(mcp::view(&c, scope, |name| h.mcp_tests.status(&key, name)))
    })
    .await?;
    view.project = project;
    Ok(view)
}

pub async fn mcp(State(hub): State<Arc<Hub>>, Query(q): Query<ScopeQuery>) -> ApiResult<McpView> {
    let (scope, workspace, project) = place(&hub, q.project).await?;
    Ok(Json(mcp_view(&hub, scope, workspace, project).await?))
}

pub async fn save_mcp(State(hub): State<Arc<Hub>>, Json(b): Json<McpSave>) -> ApiResult<McpView> {
    let (scope, workspace, project) = place(&hub, b.project).await?;
    let name = b.server.name.clone();
    let (h, ws, server, providers) = (hub.clone(), workspace.clone(), b.server, b.providers);
    let written = blocking(move || mcp::save(&h.config, scope, &ws, &server, &providers)).await?;
    hub.mcp_tests.forget(&mcp::scope_key(project.as_deref()), &name);
    hub.event(Level::Info, "mcp.saved", None, None, format!("MCP server {name} saved for {}: {}", where_(project.as_deref()), paths(&written))).await;
    Ok(Json(mcp_view(&hub, scope, workspace, project).await?))
}

pub async fn delete_mcp(State(hub): State<Arc<Hub>>, Json(b): Json<McpNameRequest>) -> ApiResult<McpView> {
    let (scope, workspace, project) = place(&hub, b.project).await?;
    let (h, ws, name) = (hub.clone(), workspace.clone(), b.name.clone());
    let removed = blocking(move || mcp::delete(&h.config, scope, &ws, &name)).await?;
    if removed.is_empty() {
        return Err(not_found("MCP server"));
    }
    hub.mcp_tests.forget(&mcp::scope_key(project.as_deref()), &b.name);
    hub.event(Level::Info, "mcp.deleted", None, None, format!("MCP server {} removed from {}: {}", b.name, where_(project.as_deref()), paths(&removed))).await;
    Ok(Json(mcp_view(&hub, scope, workspace, project).await?))
}

pub async fn test_mcp(State(hub): State<Arc<Hub>>, Json(b): Json<McpNameRequest>) -> ApiResult<McpTestResult> {
    let (scope, workspace, project) = place(&hub, b.project).await?;
    let (h, ws, name) = (hub.clone(), workspace.clone(), b.name.clone());
    let def = blocking(move || Ok(mcp::def_for_test(&mcp::collect(&h.config, scope, &ws), scope, &name))).await?.ok_or_else(|| not_found("MCP server"))?;
    let config = &hub.config;
    let result = mcp::test(&def, &workspace, mcp::TEST_TIMEOUT, &hub.http, &|v| config.env_value(v)).await;
    hub.mcp_tests.record(&mcp::scope_key(project.as_deref()), &b.name, &result);
    Ok(Json(result))
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::permissions::tests::{setup, write};

    async fn serve(hub: std::sync::Arc<crate::hub::Hub>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, super::super::router(hub)).await.unwrap() });
        format!("http://{addr}/api")
    }

    async fn call(method: &str, url: &str, body: Option<Value>) -> (u16, Value, String) {
        let mut req = reqwest::Client::new().request(method.parse().unwrap(), url).bearer_auth("secret");
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        let text = res.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null), text)
    }

    const FAKE: &str = r#"#!/bin/bash
while IFS= read -r line; do
  [[ $line =~ \"id\":([0-9]+) ]] && id=${BASH_REMATCH[1]}
  case "$line" in
    *'"method":"initialize"'*) echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}" ;;
    *'"method":"tools/list"'*) echo "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"one\"}]}}" ;;
  esac
done
"#;

    #[tokio::test]
    async fn the_configure_routes_read_write_and_test() {
        let s = setup().await;
        let base = serve(s.hub.clone()).await;
        let slug = &s.slug;

        // Permissions: a project cell, then cleared.
        let body = json!({"project": slug, "cells": [{"rule": "Bash(*)", "provider": "claude", "value": "deny"}]});
        let (status, v, _) = call("PUT", &format!("{base}/permissions"), Some(body)).await;
        assert_eq!(status, 200, "{v}");
        let row = v["rows"].as_array().unwrap().iter().find(|r| r["rule"] == "Bash(*)").unwrap();
        let claude = row["cells"].as_array().unwrap().iter().find(|c| c["provider"] == "claude").unwrap();
        assert_eq!((claude["value"].as_str(), claude["source"].as_str()), (Some("deny"), Some("project")));
        assert_eq!(v["project"], json!(slug));
        let (status, v, _) = call("PUT", &format!("{base}/permissions"), Some(json!({"project": slug, "cells": [{"rule": "Bash(*)", "provider": "claude", "value": null}]}))).await;
        assert_eq!(status, 200);
        let row = v["rows"].as_array().unwrap().iter().find(|r| r["rule"] == "Bash(*)").unwrap();
        assert!(row["cells"].as_array().unwrap().iter().any(|c| c["provider"] == "claude" && c["source"].is_null()));
        let (status, _, _) = call("PUT", &format!("{base}/permissions"), Some(json!({"cells": [{"rule": "Bash(*)", "provider": "codex", "value": "deny"}]}))).await;
        assert_eq!(status, 400);
        let (status, _, _) = call("GET", &format!("{base}/permissions?project=nope"), None).await;
        assert_eq!(status, 400);

        // MCP: add a stdio server with a secret, list it, test it, delete it.
        let fake = s.project.join("fake-mcp");
        write(&fake, FAKE);
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let server = json!({"name": "fake", "transport": "stdio", "command": fake.display().to_string(), "args": [], "env": [{"key": "FAKE_TOKEN", "value": "sk-never-shown"}]});
        let (status, v, text) = call("PUT", &format!("{base}/mcp"), Some(json!({"project": slug, "server": server, "providers": ["claude", "codex"]}))).await;
        assert_eq!(status, 200, "{v}");
        assert!(!text.contains("sk-never-shown"));
        let fake_row = v["servers"].as_array().unwrap().iter().find(|s| s["name"] == "fake").unwrap().clone();
        assert_eq!(fake_row["env_keys"], json!([{"key": "FAKE_TOKEN", "set": true}]));
        assert_eq!(fake_row["providers"], json!(["codex", "claude"]));
        assert_eq!(fake_row["source"], "project");
        assert_eq!(fake_row["status"]["label"], "not tested");

        let (status, r, _) = call("POST", &format!("{base}/mcp/test"), Some(json!({"project": slug, "name": "fake"}))).await;
        assert_eq!(status, 200, "{r}");
        assert_eq!((r["ok"].as_bool(), r["tools"].as_u64()), (Some(true), Some(1)), "{r}");
        let (_, v, text) = call("GET", &format!("{base}/mcp?project={slug}"), None).await;
        assert!(!text.contains("sk-never-shown"));
        let row = v["servers"].as_array().unwrap().iter().find(|s| s["name"] == "fake").unwrap();
        assert_eq!(row["status"]["label"], "connected · 1 tools");
        // Not in the machine scope.
        let (_, v, _) = call("GET", &format!("{base}/mcp"), None).await;
        assert!(v["servers"].as_array().unwrap().iter().all(|s| s["name"] != "fake"));
        // The permissions matrix gets its row.
        let (_, v, _) = call("GET", &format!("{base}/permissions?project={slug}"), None).await;
        assert!(v["rows"].as_array().unwrap().iter().any(|r| r["rule"] == "mcp__fake__*"));

        let (status, _, _) = call("POST", &format!("{base}/mcp/test"), Some(json!({"project": slug, "name": "missing"}))).await;
        assert_eq!(status, 404);
        let (status, v, _) = call("DELETE", &format!("{base}/mcp"), Some(json!({"project": slug, "name": "fake"}))).await;
        assert_eq!(status, 200, "{v}");
        assert!(v["servers"].as_array().unwrap().is_empty());
        let (status, _, _) = call("DELETE", &format!("{base}/mcp"), Some(json!({"project": slug, "name": "fake"}))).await;
        assert_eq!(status, 404);
    }
}
