//! `/api/git/*`: MAP's SSH key, git identity and host tests (`git_access.rs`), and `/api/projects/{slug}/git*`: a
//! project folder's status, fetch, pull, push and remote (`project_git.rs`). Only public keys are ever returned.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::Value;

use super::{ApiError, ApiResult};
use crate::git_access::{self, on_path};
use crate::hub::Hub;
use crate::model::{GitAccess, GitIdentity, GitKey, GitOpResult, GitSettings, GitStatus, GitTestResult, KnownHost, Level, SettingsInfo, SshMethod};
use crate::project_git::{self, Op, OpError};

/// `ssh -T` gets this long; GitHub answers within a second or two.
const TEST_TIMEOUT: Duration = Duration::from_secs(20);

pub async fn access(State(hub): State<Arc<Hub>>) -> ApiResult<GitAccess> {
    let files = hub.git_files();
    let gitconfig = hub.gitconfig();
    Ok(Json(GitAccess {
        key: git_access::key(&files).await,
        identity: git_access::read_identity(&files),
        known_hosts: git_access::known_hosts(&files).await,
        gitconfig_path: gitconfig.display().to_string(),
        includes: git_access::user_includes().iter().map(|p| p.display().to_string()).collect(),
        projects_root: hub.config.projects.default_root().path.display().to_string(),
        ssh_available: on_path("ssh") && on_path("ssh-keygen"),
        settings: git_access::read_settings(&files),
        system_ssh: git_access::system_ssh(&git_access::home_ssh_dir(), crate::settings::in_container()).await,
    }))
}

/// `{"ssh": "map_key" | "system"}`: which SSH setup git uses; the gitconfig is regenerated at once.
pub async fn set_settings(State(hub): State<Arc<Hub>>, Json(b): Json<GitSettings>) -> ApiResult<GitSettings> {
    let saved = git_access::write_settings(&hub.git_files(), &b)?;
    let word = match saved.ssh {
        SshMethod::MapKey => "MAP's own key",
        SshMethod::System => "this machine's SSH setup (~/.ssh, ssh-agent)",
    };
    hub.event(Level::Info, "git.settings", None, None, format!("Git now uses {word}")).await;
    Ok(Json(saved))
}

/// Forgets a host recorded on first use (409 for a pinned host, 404 when MAP's known_hosts does not name it);
/// returns the known hosts left.
pub async fn remove_known_host(State(hub): State<Arc<Hub>>, UrlPath(host): UrlPath<String>) -> ApiResult<Vec<KnownHost>> {
    let files = hub.git_files();
    if host.split(',').any(|h| git_access::is_pinned(h.trim())) {
        return Err(ApiError(StatusCode::CONFLICT, format!("{host} is pinned: MAP ships its published host keys, which cannot be removed")));
    }
    let removed = git_access::remove_known_host(&files, &host)?;
    if removed == 0 {
        return Err(super::not_found("host"));
    }
    hub.event(Level::Info, "git.known_host", None, None, format!("Forgot {host}'s host key{} (recorded on first use)", if removed == 1 { "" } else { "s" })).await;
    Ok(Json(git_access::known_hosts(&files).await))
}

/// The Settings screen's read-only facts; never a secret's value.
pub async fn settings(State(hub): State<Arc<Hub>>) -> Json<SettingsInfo> {
    Json(crate::settings::info(&hub))
}

#[derive(Deserialize)]
pub struct KeyBody {
    #[serde(default)]
    replace: bool,
}

/// Creates MAP's key; 409 when one exists and `replace` is not set.
pub async fn create_key(State(hub): State<Arc<Hub>>, Json(b): Json<KeyBody>) -> ApiResult<GitKey> {
    let files = hub.git_files();
    if !b.replace && (files.key().exists() || files.public_key().exists()) {
        return Err(ApiError(StatusCode::CONFLICT, "MAP already has a key; send {\"replace\": true} to replace it (the old one is kept aside)".into()));
    }
    let key = git_access::create_key(&files, b.replace).await?;
    let word = if b.replace { "replaced" } else { "created" };
    hub.event(Level::Info, "git.key", None, None, format!("MAP's SSH key {word} · {} · {}", key.fingerprint, key.comment)).await;
    Ok(Json(key))
}

#[derive(Deserialize)]
pub struct TestBody {
    host: String,
}

pub async fn test(State(hub): State<Arc<Hub>>, Json(b): Json<TestBody>) -> ApiResult<GitTestResult> {
    let host = git_access::parse_host(&b.host)?;
    let result = git_access::test(&hub.git_files(), &host, TEST_TIMEOUT).await?;
    let level = if result.ok { Level::Info } else { Level::Warning };
    let first = result.output.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(160).collect::<String>();
    hub.event(level, "git.test", None, None, format!("{}: {}", result.command, first)).await;
    Ok(Json(result))
}

pub async fn set_identity(State(hub): State<Arc<Hub>>, Json(b): Json<GitIdentity>) -> ApiResult<GitIdentity> {
    let id = git_access::write_identity(&hub.git_files(), &b)?;
    let shown = if id.name.is_empty() { "cleared".to_string() } else { format!("{} <{}>", id.name, id.email) };
    hub.event(Level::Info, "git.identity", None, None, format!("Git identity {shown}")).await;
    Ok(Json(id))
}

// ---- a project's folder

async fn known(hub: &Hub, slug: &str) -> Result<(), ApiError> {
    hub.store.project(slug).await?.ok_or_else(|| super::not_found("project"))?;
    Ok(())
}

pub async fn status(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<GitStatus> {
    known(&hub, &slug).await?;
    Ok(Json(project_git::status(&hub, &slug).await?))
}

async fn run(hub: &Hub, slug: &str, op: Op, url: Option<&str>) -> ApiResult<GitOpResult> {
    known(hub, slug).await?;
    match project_git::run_op(hub, slug, op, url).await {
        Ok(r) => Ok(Json(r)),
        Err(OpError::Refused(why)) => Err(ApiError(StatusCode::CONFLICT, why)),
        Err(OpError::Invalid(err)) => Err(err.into()),
    }
}

/// Takes `{}`, like the other state-changing routes, so a cross-site form cannot trigger it.
pub async fn fetch(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(_): Json<Value>) -> ApiResult<GitOpResult> {
    run(&hub, &slug, Op::Fetch, None).await
}

pub async fn pull(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(_): Json<Value>) -> ApiResult<GitOpResult> {
    run(&hub, &slug, Op::Pull, None).await
}

pub async fn push(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(_): Json<Value>) -> ApiResult<GitOpResult> {
    run(&hub, &slug, Op::Push, None).await
}

#[derive(Deserialize)]
pub struct RemoteBody {
    url: String,
}

pub async fn remote(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<RemoteBody>) -> ApiResult<GitOpResult> {
    run(&hub, &slug, Op::Remote, Some(&b.url)).await
}
