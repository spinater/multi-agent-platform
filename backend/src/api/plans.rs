//! Plans and the planner session (`plans.rs`): a project's planner submits drafts with its project token; only the
//! owner (the API token) edits, approves or rejects them and starts the planner.
//!
//! - `/projects/{slug}/plans` and `/workspace/plans` (the default workspace): GET list, POST a draft. The project's
//!   (or workspace's) planner token opens these two only.
//! - `/plans?project=` (absent: all, empty: the default workspace), `/plans/{id}` GET/PATCH, `/plans/{id}/approve`,
//!   `/plans/{id}/reject`.
//! - `/projects/{slug}/planner`, `/workspace/planner`: GET its session and default provider, POST start it.
//! - `/planner/instructions`: the planner's instructions, read-only.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use super::{ApiError, ApiResult};
use crate::hub::Hub;
use crate::model::{Plan, PlanApproved, PlanEdit, PlanSubmit, PlannerInfo, PlannerInstructions, Session};
use crate::plans::{self, PlanError};

impl From<PlanError> for ApiError {
    fn from(err: PlanError) -> Self {
        match err {
            PlanError::NotFound(what) => ApiError(StatusCode::NOT_FOUND, format!("{what} not found")),
            PlanError::Conflict(why) => ApiError(StatusCode::CONFLICT, why),
            PlanError::Invalid(err) => ApiError(StatusCode::BAD_REQUEST, format!("{err:#}")),
        }
    }
}

const LIST_LIMIT: i64 = 200;

async fn list_for(hub: &Hub, project: Option<&str>) -> ApiResult<Vec<Plan>> {
    if let Some(slug) = project {
        hub.store.project(slug).await?.ok_or_else(|| super::not_found("project"))?;
    }
    Ok(Json(hub.store.plans(Some(project.unwrap_or("")), LIST_LIMIT).await?))
}

pub async fn project_list(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<Vec<Plan>> {
    list_for(&hub, Some(&slug)).await
}

pub async fn workspace_list(State(hub): State<Arc<Hub>>) -> ApiResult<Vec<Plan>> {
    list_for(&hub, None).await
}

pub async fn project_submit(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(body): Json<PlanSubmit>) -> Result<(StatusCode, Json<Plan>), ApiError> {
    Ok((StatusCode::CREATED, Json(plans::submit(&hub, Some(&slug), body).await?)))
}

pub async fn workspace_submit(State(hub): State<Arc<Hub>>, Json(body): Json<PlanSubmit>) -> Result<(StatusCode, Json<Plan>), ApiError> {
    Ok((StatusCode::CREATED, Json(plans::submit(&hub, None, body).await?)))
}

#[derive(Deserialize)]
pub struct ListQuery {
    project: Option<String>,
}

/// `project` absent: every plan; empty: the default workspace's; else that project's.
pub async fn list(State(hub): State<Arc<Hub>>, Query(q): Query<ListQuery>) -> ApiResult<Vec<Plan>> {
    Ok(Json(hub.store.plans(q.project.as_deref(), LIST_LIMIT).await?))
}

pub async fn get(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<i64>) -> ApiResult<Plan> {
    Ok(Json(hub.store.plan(id).await?.ok_or_else(|| super::not_found("plan"))?))
}

pub async fn edit(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<i64>, Json(body): Json<PlanEdit>) -> ApiResult<Plan> {
    Ok(Json(plans::edit(&hub, id, body).await?))
}

/// Takes `{}`, like the other state-changing routes.
pub async fn approve(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<i64>, Json(_): Json<Value>) -> ApiResult<PlanApproved> {
    Ok(Json(plans::approve(&hub, id).await?))
}

#[derive(Deserialize)]
pub struct RejectBody {
    #[serde(default)]
    reason: Option<String>,
}

pub async fn reject(State(hub): State<Arc<Hub>>, UrlPath(id): UrlPath<i64>, Json(body): Json<RejectBody>) -> ApiResult<Plan> {
    Ok(Json(plans::reject(&hub, id, body.reason).await?))
}

// ---- the planner session

#[derive(Deserialize)]
pub struct StartBody {
    #[serde(default)]
    provider: Option<String>,
    #[serde(default)]
    model: Option<String>,
}

pub async fn project_planner(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<PlannerInfo> {
    Ok(Json(plans::info(&hub, Some(&slug)).await?))
}

pub async fn workspace_planner(State(hub): State<Arc<Hub>>) -> ApiResult<PlannerInfo> {
    Ok(Json(plans::info(&hub, None).await?))
}

/// Starts the planner session, or returns it while it runs (`provider` and `model` then do not apply).
pub async fn project_start(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<StartBody>) -> ApiResult<Session> {
    Ok(Json(plans::start(&hub, Some(&slug), b.provider, b.model).await?))
}

pub async fn workspace_start(State(hub): State<Arc<Hub>>, Json(b): Json<StartBody>) -> ApiResult<Session> {
    Ok(Json(plans::start(&hub, None, b.provider, b.model).await?))
}

pub async fn instructions(State(hub): State<Arc<Hub>>) -> ApiResult<PlannerInstructions> {
    Ok(Json(plans::instructions(&hub)?))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::plans::tests::{hub, project};

    /// Serves the real router (token, Host and scope checks included) on a free loopback port.
    async fn serve(hub: std::sync::Arc<crate::hub::Hub>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, super::super::router(hub)).await.unwrap() });
        format!("http://{addr}/api")
    }

    async fn call(method: &str, url: &str, token: &str, body: Option<serde_json::Value>) -> (u16, serde_json::Value) {
        let client = reqwest::Client::new();
        let mut req = client.request(method.parse().unwrap(), url).bearer_auth(token);
        if let Some(b) = body {
            req = req.json(&b);
        }
        let res = req.send().await.unwrap();
        let status = res.status().as_u16();
        (status, res.json().await.unwrap_or(serde_json::Value::Null))
    }

    fn draft() -> serde_json::Value {
        json!({"title": "Bookings", "tasks": [{"key": "t1", "title": "Schema", "prompt": "Goal: a table."}, {"key": "t2", "title": "API", "prompt": "Goal: an endpoint.", "depends_on": ["t1"]}]})
    }

    #[tokio::test]
    async fn planner_tokens_submit_and_list_only_their_own_plans_and_never_approve() {
        let (hub, _, dir) = hub("").await;
        let app = project(&hub, &dir.path().join("app"), "App").await;
        let other = project(&hub, &dir.path().join("other"), "Other").await;
        let (token, other_token, ws_token) = (hub.project_token(&app).unwrap(), hub.project_token(&other).unwrap(), hub.workspace_token().unwrap());
        let base = serve(hub.clone()).await;

        let (status, plan) = call("POST", &format!("{base}/projects/{app}/plans"), &token, Some(draft())).await;
        assert_eq!(status, 201, "{plan}");
        let id = plan["id"].as_i64().unwrap();
        assert_eq!(plan["status"], "draft");
        let (status, list) = call("GET", &format!("{base}/projects/{app}/plans"), &token, None).await;
        assert_eq!((status, list.as_array().unwrap().len()), (200, 1));
        // Invalid drafts are a 400 with the reason.
        let bad = json!({"title": "x", "tasks": [{"key": "a", "title": "A", "prompt": "p", "depends_on": ["zz"]}]});
        let (status, err) = call("POST", &format!("{base}/projects/{app}/plans"), &token, Some(bad)).await;
        assert_eq!(status, 400);
        assert!(err["error"].as_str().unwrap().contains("not a task of this plan"));

        // Not another project's, not the default workspace's, not the owner's routes.
        for (method, url, body) in [
            ("POST", format!("{base}/projects/{other}/plans"), Some(draft())),
            ("GET", format!("{base}/projects/{other}/plans"), None),
            ("POST", format!("{base}/workspace/plans"), Some(draft())),
            ("GET", format!("{base}/plans?project={app}"), None),
            ("GET", format!("{base}/plans/{id}"), None),
            ("POST", format!("{base}/plans/{id}/approve"), Some(json!({}))),
            ("POST", format!("{base}/plans/{id}/reject"), Some(json!({}))),
            ("PATCH", format!("{base}/plans/{id}"), Some(json!({"title": "mine"}))),
            ("POST", format!("{base}/projects/{app}/planner"), Some(json!({}))),
            ("POST", format!("{base}/tasks"), Some(json!({"title": "t", "prompt": "p"}))),
        ] {
            // A real project token outside its scope is forbidden (403), not unauthenticated.
            assert_eq!(call(method, &url, &token, body).await.0, 403, "{method} {url}");
        }
        assert_eq!(call("GET", &format!("{base}/projects/{app}/plans"), &other_token, None).await.0, 403);
        assert_eq!(call("POST", &format!("{base}/plans/{id}/approve"), &ws_token, Some(json!({}))).await.0, 401);
        assert!(hub.store.tasks().await.unwrap().is_empty());

        // The default workspace's planner token opens only the default workspace's plans.
        let (status, ws_plan) = call("POST", &format!("{base}/workspace/plans"), &ws_token, Some(draft())).await;
        assert_eq!((status, ws_plan["project"].clone()), (201, serde_json::Value::Null));
        assert_eq!(call("GET", &format!("{base}/workspace/plans"), &ws_token, None).await.1.as_array().unwrap().len(), 1);
        assert_eq!(call("GET", &format!("{base}/projects/{app}/plans"), &ws_token, None).await.0, 401);

        // The owner approves once; a second approval is a conflict.
        let (status, approved) = call("POST", &format!("{base}/plans/{id}/approve"), "secret", Some(json!({}))).await;
        assert_eq!(status, 200, "{approved}");
        let tasks = approved["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[1]["depends_on"], json!([tasks[0]["id"]]));
        assert_eq!(tasks[0]["project"], app.as_str());
        assert_eq!(call("POST", &format!("{base}/plans/{id}/approve"), "secret", Some(json!({}))).await.0, 409);
        assert_eq!(call("POST", &format!("{base}/plans/9999/approve"), "secret", Some(json!({}))).await.0, 404);
        let (status, list) = call("GET", &format!("{base}/plans?project="), "secret", None).await;
        assert_eq!((status, list.as_array().unwrap().len()), (200, 1));
        assert_eq!(call("GET", &format!("{base}/plans"), "secret", None).await.1.as_array().unwrap().len(), 2);
        let (status, rejected) = call("POST", &format!("{base}/plans/{}/reject", ws_plan["id"]), "secret", Some(json!({"reason": "later"}))).await;
        assert_eq!((status, rejected["status"].as_str(), rejected["reason"].as_str()), (200, Some("rejected"), Some("later")));
    }
}
