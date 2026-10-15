//! `/api/workflows*` and `/api/configure/ask`: Configure › Workflows and the "Ask an agent to do it" panels.
//! `project` is a slug; empty or missing is the machine (every project).

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::{ApiError, ApiResult, known_project, not_found};
use crate::hub::Hub;
use crate::model::{ConfigAskRequest, ConfigAskResult, Level, Workflow, WorkflowList, WorkflowStats};
use crate::workflows;

#[derive(Deserialize)]
pub struct ProjectQuery {
    project: Option<String>,
}

pub async fn list(State(hub): State<Arc<Hub>>, Query(q): Query<ProjectQuery>) -> ApiResult<WorkflowList> {
    let project = known_project(&hub, q.project).await?;
    let workflows = workflows::list(&hub, project.as_deref()).await?;
    Ok(Json(WorkflowList { project, workflows }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveBody {
    #[serde(default)]
    project: Option<String>,
    workflow: Workflow,
}

/// Creates or replaces the workflow with this id in the project (or the machine); 400 with every problem when it is
/// not valid. Returns it as listed (with its source).
pub async fn save(State(hub): State<Arc<Hub>>, Json(b): Json<SaveBody>) -> ApiResult<Workflow> {
    let project = known_project(&hub, b.project).await?;
    let saved = workflows::save(&hub, project.as_deref(), b.workflow).await?;
    let place = project.as_deref().unwrap_or("the machine");
    hub.event(Level::Info, "workflow.saved", None, None, format!("Workflow {} saved for {place}", saved.id)).await;
    Ok(Json(saved))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteBody {
    #[serde(default)]
    project: Option<String>,
    id: String,
}

/// Removes the project's (or the machine's) workflow; a removed project override brings the machine one back.
pub async fn delete(State(hub): State<Arc<Hub>>, Json(b): Json<DeleteBody>) -> ApiResult<WorkflowList> {
    let project = known_project(&hub, b.project).await?;
    if !workflows::delete(&hub, project.as_deref(), &b.id).await? {
        return Err(not_found("workflow"));
    }
    let place = project.as_deref().unwrap_or("the machine");
    hub.event(Level::Info, "workflow.deleted", None, None, format!("Workflow {} deleted for {place}", b.id)).await;
    Ok(Json(WorkflowList { workflows: workflows::list(&hub, project.as_deref()).await?, project }))
}

pub async fn stats(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<String>, Query(q): Query<ProjectQuery>) -> ApiResult<WorkflowStats> {
    let project = known_project(&hub, q.project).await?;
    Ok(Json(workflows::stats(&hub, project.as_deref(), &id).await?))
}

/// One headless run of the chosen provider; 400 for a bad request or a reply without ops, 504 when it timed out.
pub async fn ask(State(hub): State<Arc<Hub>>, Json(mut req): Json<ConfigAskRequest>) -> ApiResult<ConfigAskResult> {
    req.project = known_project(&hub, req.project).await?;
    match crate::configure_ask::ask(&hub, req).await {
        Ok(out) => Ok(Json(out)),
        Err(err) if format!("{err:#}").contains("no answer within") => Err(ApiError(StatusCode::GATEWAY_TIMEOUT, format!("{err:#}"))),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use crate::plans::tests::{hub, project};

    async fn serve(hub: std::sync::Arc<crate::hub::Hub>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, super::super::router(hub)).await.unwrap() });
        format!("http://{addr}/api")
    }

    async fn call(method: &str, url: &str, token: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = reqwest::Client::new().request(method.parse().unwrap(), url).bearer_auth(token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        (status, res.json().await.unwrap_or(Value::Null))
    }

    fn flow(name: &str) -> Value {
        json!({
            "id": "bugfix", "name": name, "trigger": {"keywords": ["Bug"]},
            "nodes": [
                {"id": "t", "kind": "trigger"},
                {"id": "code", "kind": "agent", "agent": "claude"},
                {"id": "ver", "kind": "verify", "command": "true", "max_attempts": 2},
                {"id": "done", "kind": "end", "outcome": "done"}
            ],
            "edges": [{"from": "t", "to": "code"}, {"from": "code", "to": "ver"}, {"from": "ver", "to": "done", "kind": "pass"}, {"from": "ver", "to": "code", "kind": "fail"}]
        })
    }

    #[tokio::test]
    async fn workflows_are_saved_merged_overridden_used_by_tasks_and_deleted() {
        let (hub, _, dir) = hub("[agents_home]\nmode = \"isolated\"\ndir = \"/nonexistent/agent-home\"").await;
        let app = project(&hub, &dir.path().join("app"), "App").await;
        let base = serve(hub.clone()).await;
        let (status, saved) = call("PUT", &format!("{base}/workflows"), "secret", Some(json!({"workflow": flow("machine")}))).await;
        assert_eq!(status, 200, "{saved}");
        assert_eq!((saved["source"].as_str(), saved["trigger"]["keywords"][0].as_str()), (Some("machine"), Some("bug")));
        let (_, saved) = call("PUT", &format!("{base}/workflows"), "secret", Some(json!({"project": app, "workflow": flow("ours")}))).await;
        assert_eq!(saved["source"], "override");
        let (_, list) = call("GET", &format!("{base}/workflows?project={app}"), "secret", None).await;
        assert_eq!((list["workflows"].as_array().unwrap().len(), list["workflows"][0]["name"].as_str()), (1, Some("ours")));
        let (_, list) = call("GET", &format!("{base}/workflows"), "secret", None).await;
        assert_eq!(list["workflows"][0]["name"], "machine");

        // A broken one is a 400 with every problem.
        let mut bad = flow("x");
        bad["nodes"][2]["max_attempts"] = json!(0);
        bad["edges"] = json!([{"from": "t", "to": "code"}]);
        let (status, err) = call("PUT", &format!("{base}/workflows"), "secret", Some(json!({"workflow": bad}))).await;
        assert_eq!(status, 400);
        let err = err["error"].as_str().unwrap();
        assert!(err.contains("max attempts 0") && err.contains("cannot be reached"), "{err}");

        // Tasks may pin a workflow of their scope only.
        let (status, task) = call("POST", &format!("{base}/tasks"), "secret", Some(json!({"title": "T", "prompt": "p", "project": app, "workflow": "bugfix"}))).await;
        assert_eq!((status, task["workflow"].as_str()), (200, Some("bugfix")), "{task}");
        let (status, _) = call("POST", &format!("{base}/tasks"), "secret", Some(json!({"title": "T", "prompt": "p", "workflow": "nope"}))).await;
        assert_eq!(status, 400);
        let (status, task) = call("POST", &format!("{base}/projects/{app}/tasks"), "secret", Some(json!({"title": "T", "prompt": "p", "workflow": "bugfix"}))).await;
        assert_eq!((status, task["workflow"].as_str()), (200, Some("bugfix")), "{task}");

        let (status, stats) = call("GET", &format!("{base}/workflows/bugfix/stats?project={app}"), "secret", None).await;
        assert_eq!((status, stats["runs"].as_u64()), (200, Some(0)), "{stats}");
        assert_eq!(stats["nodes"].as_array().unwrap().len(), 4);

        // A project token may not touch them.
        let token = hub.project_token(&app).unwrap();
        let (status, _) = call("GET", &format!("{base}/workflows?project={app}"), &token, None).await;
        assert_eq!(status, 403);

        // Deleting the override brings the machine workflow back; deleting it again is a 404.
        let (status, list) = call("DELETE", &format!("{base}/workflows"), "secret", Some(json!({"project": app, "id": "bugfix"}))).await;
        assert_eq!((status, list["workflows"][0]["source"].as_str()), (200, Some("machine")));
        let (status, _) = call("DELETE", &format!("{base}/workflows"), "secret", Some(json!({"project": app, "id": "bugfix"}))).await;
        assert_eq!(status, 404);
        // A settings PATCH leaves the project's workflows alone.
        call("PUT", &format!("{base}/workflows"), "secret", Some(json!({"project": app, "workflow": flow("ours")}))).await;
        let p = hub.store.project(&app).await.unwrap().unwrap();
        let mut settings = serde_json::to_value(&p.settings).unwrap();
        settings.as_object_mut().unwrap().remove("workflows");
        let (status, _) = call("PATCH", &format!("{base}/projects/{app}"), "secret", Some(json!({"settings": settings}))).await;
        assert_eq!(status, 200);
        assert_eq!(hub.store.project(&app).await.unwrap().unwrap().settings.workflows.len(), 1);
    }
}
