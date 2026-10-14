//! `/git/<slug>/corpus.git`: git smart HTTP through `git http-backend`, so a project's Reflex corpus can be cloned
//! and pushed with plain git. The project's code is never served: it stays in the project's folder. With
//! `server.api_token` set, git sends it (or the project's own `$MAP_PROJECT_TOKEN`) as the Basic-auth password
//! (any user name).
//! A successful push to the corpus repo restarts the project's Reflex.

use std::process::Stdio;
use std::sync::Arc;

use axum::body::{Body, Bytes};
use axum::extract::{Path as UrlPath, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::same;
use crate::hub::Hub;

/// The API token, or the project's own token (`$MAP_PROJECT_TOKEN`, so its agents can push the corpus).
fn authorized(hub: &Hub, slug: &str, headers: &HeaderMap) -> bool {
    let Some(expected) = hub.config.server.api_token.as_deref() else { return true };
    let Some(value) = headers.get("authorization").and_then(|v| v.to_str().ok()) else { return false };
    let accepts = |token: &str| same(token, expected) || hub.project_token(slug).is_some_and(|t| same(token, &t));
    if let Some(token) = value.strip_prefix("Bearer ") {
        return accepts(token);
    }
    let Some(encoded) = value.strip_prefix("Basic ") else { return false };
    let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else { return false };
    let decoded = String::from_utf8_lossy(&decoded);
    decoded.split_once(':').is_some_and(|(_, password)| accepts(password))
}

fn plain(status: StatusCode, text: &str) -> Response {
    (status, text.to_string()).into_response()
}

pub async fn handler(
    State(hub): State<Arc<Hub>>,
    UrlPath((slug, rest)): UrlPath<(String, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !authorized(&hub, &slug, &headers) {
        let mut resp = plain(StatusCode::UNAUTHORIZED, "the API token is required (as the password)");
        resp.headers_mut().insert("www-authenticate", HeaderValue::from_static("Basic realm=\"multi-agent-platform\""));
        return resp;
    }
    let repo = rest.split('/').next().unwrap_or("");
    if repo != "corpus.git" || rest.split('/').any(|s| s == ".." || s.is_empty()) {
        return plain(StatusCode::NOT_FOUND, "the only repo is corpus.git");
    }
    match hub.store.project(&slug).await {
        Ok(Some(_)) => {}
        Ok(None) => return plain(StatusCode::NOT_FOUND, "unknown project"),
        Err(err) => return plain(StatusCode::INTERNAL_SERVER_ERROR, &format!("{err:#}")),
    }

    let header = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
    let mut cmd = Command::new("git");
    cmd.arg("http-backend")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("GIT_PROJECT_ROOT", hub.config.server.data_dir.join("projects"))
        .env("GIT_HTTP_EXPORT_ALL", "1")
        .env("PATH_INFO", format!("/{slug}/{rest}"))
        .env("REQUEST_METHOD", method.as_str())
        .env("QUERY_STRING", uri.query().unwrap_or(""))
        .env("CONTENT_TYPE", header("content-type"))
        .env("CONTENT_LENGTH", body.len().to_string())
        .env("HTTP_CONTENT_ENCODING", header("content-encoding"))
        .env("GIT_PROTOCOL", header("git-protocol"))
        .env("REMOTE_ADDR", "127.0.0.1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(err) => return plain(StatusCode::INTERNAL_SERVER_ERROR, &format!("git http-backend: {err}")),
    };
    let mut stdin = child.stdin.take();
    let feed = async move {
        if let Some(pipe) = stdin.as_mut() {
            let _ = pipe.write_all(&body).await;
        }
        drop(stdin);
    };
    let (_, out) = tokio::join!(feed, child.wait_with_output());
    let out = match out {
        Ok(o) => o,
        Err(err) => return plain(StatusCode::INTERNAL_SERVER_ERROR, &format!("git http-backend: {err}")),
    };
    let resp = cgi_response(&out.stdout).unwrap_or_else(|| plain(StatusCode::INTERNAL_SERVER_ERROR, &String::from_utf8_lossy(&out.stderr)));

    if repo == "corpus.git" && rest.ends_with("git-receive-pack") && resp.status().is_success() {
        let hub = hub.clone();
        tokio::spawn(async move {
            if let Err(err) = hub.restart_reflex(&slug).await {
                tracing::warn!("reflex restart after push to {slug}: {err:#}");
            }
        });
    }
    resp
}

/// Turns CGI output (headers, a blank line, the body) into a response.
fn cgi_response(raw: &[u8]) -> Option<Response> {
    let (split, gap) = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)))?;
    let head = String::from_utf8_lossy(&raw[..split]);
    let mut status = StatusCode::OK;
    let mut headers = HeaderMap::new();
    for line in head.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        if k.eq_ignore_ascii_case("status") {
            status = v.split_whitespace().next().and_then(|c| c.parse().ok()).and_then(|c| StatusCode::from_u16(c).ok()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        } else if let (Ok(name), Ok(value)) = (k.parse::<HeaderName>(), HeaderValue::from_str(v)) {
            headers.append(name, value);
        }
    }
    let mut resp = (status, Body::from(raw[split + gap..].to_vec())).into_response();
    resp.headers_mut().extend(headers);
    Some(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_cgi_output() {
        let resp = cgi_response(b"Status: 404 Not Found\r\nContent-Type: text/plain\r\n\r\nnope").unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(resp.headers()["content-type"], "text/plain");
        assert_eq!(cgi_response(b"Content-Type: x\n\nbody").unwrap().status(), StatusCode::OK);
        assert!(cgi_response(b"no blank line").is_none());
    }
}
