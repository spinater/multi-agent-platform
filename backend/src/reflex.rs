//! One Reflex process per project. Reflex reads its corpus once at boot, so every corpus change is an export of the
//! corpus repo plus a restart. Status changes reach the dashboard through `Hub::restart_reflex`.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::model::{ReflexState, ReflexStatus};
use crate::repo;

/// Reflex refuses to boot on more than this many domains.
pub const MAX_DOMAINS: usize = 8;

fn status(state: ReflexState, message: impl Into<String>, domains: Vec<String>) -> ReflexStatus {
    ReflexStatus { state, message: message.into(), domains, since: Utc::now() }
}

fn url(port: u16, path: &str) -> String {
    format!("http://127.0.0.1:{port}{path}")
}

pub async fn tail(path: &Path, lines: usize) -> String {
    let text = tokio::fs::read_to_string(path).await.unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

#[derive(Default)]
pub struct Manager {
    children: Mutex<HashMap<String, Child>>,
    status: std::sync::Mutex<HashMap<String, ReflexStatus>>,
    /// Restarts of one project run one at a time, so two quick pushes cannot start two engines on one port.
    locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    http: reqwest::Client,
    /// `reflex --version`, re-read at most every few seconds so an install shows up without a server restart.
    version: std::sync::Mutex<Option<(std::time::Instant, Option<String>)>>,
}

pub struct Paths<'a> {
    pub corpus_repo: &'a Path,
    pub corpus_live: &'a Path,
    pub log: &'a Path,
}

/// The first line of `<binary> --version` (`reflex 0.2.4`), or `None` when the binary cannot run.
pub fn read_version(binary: &str) -> Option<String> {
    let out = std::process::Command::new(binary).arg("--version").stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    out.status.success().then(|| line.to_string())
}

impl Manager {
    /// The installed Reflex's version, cached for 10 seconds.
    pub fn version(&self, binary: &str) -> Option<String> {
        let mut cache = self.version.lock().unwrap();
        if let Some((at, v)) = cache.as_ref()
            && at.elapsed() < Duration::from_secs(10)
        {
            return v.clone();
        }
        let v = read_version(binary);
        *cache = Some((std::time::Instant::now(), v.clone()));
        v
    }

    pub fn status(&self, slug: &str) -> ReflexStatus {
        self.status.lock().unwrap().get(slug).cloned().unwrap_or_else(|| status(ReflexState::Stopped, "", vec![]))
    }

    /// Tests: report `state` for `slug` without a process (a fake Reflex answers on the project's port).
    #[cfg(test)]
    pub fn pretend(&self, slug: &str, state: ReflexState) {
        self.set(slug, status(state, "test", vec![]));
    }

    fn set(&self, slug: &str, s: ReflexStatus) -> ReflexStatus {
        self.status.lock().unwrap().insert(slug.to_string(), s.clone());
        s
    }

    async fn kill(&self, slug: &str) {
        if let Some(mut child) = self.children.lock().await.remove(slug) {
            let _ = child.kill().await;
        }
    }

    pub async fn stop_all(&self) {
        for (_, mut child) in self.children.lock().await.drain() {
            let _ = child.kill().await;
        }
    }

    pub async fn forget(&self, slug: &str) {
        self.kill(slug).await;
        self.status.lock().unwrap().remove(slug);
        self.locks.lock().await.remove(slug);
    }

    pub async fn restart(&self, binary: &str, slug: &str, port: u16, paths: Paths<'_>) -> ReflexStatus {
        let lock = self.locks.lock().await.entry(slug.to_string()).or_default().clone();
        let _guard = lock.lock().await;
        self.kill(slug).await;
        self.set(slug, status(ReflexState::Starting, "exporting the corpus", vec![]));
        let s = match self.boot(binary, slug, port, paths).await {
            Ok(s) => s,
            Err(err) => status(ReflexState::Error, format!("{err:#}"), vec![]),
        };
        self.set(slug, s)
    }

    async fn boot(&self, binary: &str, slug: &str, port: u16, paths: Paths<'_>) -> Result<ReflexStatus> {
        let domains = repo::export_corpus(paths.corpus_repo, paths.corpus_live).await?;
        if domains.is_empty() {
            return Ok(status(ReflexState::NoCorpus, "The corpus repo has no domains yet: add <domain>/<file>.md", domains));
        }
        if domains.len() > MAX_DOMAINS {
            return Ok(status(ReflexState::Error, format!("Reflex takes 1–{MAX_DOMAINS} domains; the corpus has {}", domains.len()), domains));
        }
        // Something else on the port (often a Reflex left behind by a server that was killed) would answer /healthz
        // for us with another corpus, while ours fails to bind and exits.
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_err() {
            return Ok(status(ReflexState::Error, format!("port {port} is already in use by another process (a Reflex left over from an earlier run?); stop it, then restart"), domains));
        }
        let log = std::fs::File::create(paths.log)?;
        let mut child = Command::new(binary)
            .env("RIIR_REFLEX_CORPUS", paths.corpus_live)
            .env("RIIR_REFLEX_BIND", format!("127.0.0.1:{port}"))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("starting `{binary}` (set reflex.binary in map.toml)"))?;
        for _ in 0..50 {
            if let Some(code) = child.try_wait()? {
                return Ok(status(ReflexState::Error, format!("reflex exited ({code}):\n{}", tail(paths.log, 20).await), domains));
            }
            if self.http.get(url(port, "/healthz")).timeout(Duration::from_secs(1)).send().await.is_ok_and(|r| r.status().is_success()) && child.try_wait()?.is_none() {
                self.children.lock().await.insert(slug.to_string(), child);
                return Ok(status(ReflexState::Running, format!("listening on 127.0.0.1:{port}"), domains));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        let _ = child.kill().await;
        Ok(status(ReflexState::Error, format!("reflex did not answer /healthz:\n{}", tail(paths.log, 20).await), domains))
    }

    /// POSTs JSON to the project's Reflex and returns its reply, turning Reflex's `{"error"}` bodies into errors.
    pub async fn post(&self, port: u16, path: &str, body: &Value) -> Result<Value> {
        let resp = self.http.post(url(port, path)).json(body).timeout(Duration::from_secs(30)).send().await.context("Reflex is not reachable")?;
        let code = resp.status();
        let value: Value = resp.json().await.context("Reflex returned non-JSON")?;
        if !code.is_success() {
            anyhow::bail!("Reflex {code}: {}", value.get("error").and_then(Value::as_str).unwrap_or("?"));
        }
        Ok(value)
    }
}
