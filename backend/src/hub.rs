//! Shared state: config, store, terminal and the broadcast channel that feeds dashboard clients.
//! Every change to a session or task goes through here so the database and the live view never disagree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use regex::Regex;
use tokio::sync::broadcast;

use crate::config::{Config, PromptMode, ProviderSpec};
use crate::detect::{EndDecision, end_decision};
use crate::model::{Event, Level, OrchestratorStatus, Project, ProjectInfo, ProviderInfo, ReflexStatus, Role, ServerMessage, Session, SessionState, Task, TaskStatus};
use crate::reflex;
use crate::store::Store;
use crate::terminal::Terminal;

static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_-]{1,64}$").unwrap());

pub struct Hub {
    pub config: Config,
    pub store: Store,
    pub term: Arc<dyn Terminal>,
    tx: broadcast::Sender<ServerMessage>,
    orchestrator_running: AtomicBool,
    cooldowns: Mutex<HashMap<String, DateTime<Utc>>>,
    pub reflex: reflex::Manager,
    pub http: reqwest::Client,
}

pub struct LaunchRequest {
    /// Appended to the session prefix; generated when empty.
    pub name: Option<String>,
    pub provider: String,
    pub model: Option<String>,
    pub role: Role,
    pub task_id: Option<i64>,
    /// Run in this project's workspace instead of `workspace.dir`.
    pub project: Option<String>,
    pub prompt: Option<String>,
}

pub fn quote(s: &str) -> Result<String> {
    Ok(shlex::try_quote(s).map_err(|e| anyhow!("cannot quote {s:?}: {e}"))?.into_owned())
}

impl Hub {
    pub fn new(config: Config, store: Store, term: Arc<dyn Terminal>) -> Arc<Self> {
        let (tx, _) = broadcast::channel(512);
        let running = config.orchestrator.autostart;
        Arc::new(Self {
            config,
            store,
            term,
            tx,
            orchestrator_running: AtomicBool::new(running),
            cooldowns: Mutex::new(HashMap::new()),
            reflex: reflex::Manager::default(),
            http: reqwest::Client::new(),
        })
    }

    pub fn subscribe(&self) -> broadcast::Receiver<ServerMessage> {
        self.tx.subscribe()
    }

    fn send(&self, msg: ServerMessage) {
        // No subscribers is normal (nobody has the dashboard open).
        let _ = self.tx.send(msg);
    }

    pub async fn event(&self, level: Level, kind: &str, session: Option<&str>, task_id: Option<i64>, message: impl AsRef<str>) -> Event {
        let message = message.as_ref();
        match level {
            Level::Error => tracing::error!(kind, session, task_id, "{message}"),
            Level::Warning => tracing::warn!(kind, session, task_id, "{message}"),
            _ => tracing::info!(kind, session, task_id, "{message}"),
        }
        let event = match self.store.insert_event(level, kind, session, task_id, message).await {
            Ok(e) => e,
            Err(err) => {
                tracing::error!("could not store event: {err:#}");
                Event { id: 0, at: Utc::now(), level, kind: kind.into(), session: session.map(String::from), task_id, message: message.into() }
            }
        };
        self.send(ServerMessage::Event { event: event.clone() });
        event
    }

    pub async fn put_session(&self, s: &Session) -> Result<()> {
        self.store.save_session(s).await?;
        self.send(ServerMessage::Session { session: s.clone() });
        Ok(())
    }

    pub async fn forget_session(&self, name: &str) -> Result<()> {
        self.store.delete_session(name).await?;
        self.send(ServerMessage::SessionRemoved { name: name.into() });
        Ok(())
    }

    pub async fn put_task(&self, t: &mut Task) -> Result<()> {
        t.updated_at = Utc::now();
        self.store.save_task(t).await?;
        self.send(ServerMessage::Task { task: t.clone() });
        Ok(())
    }

    pub fn announce_task(&self, t: &Task) {
        self.send(ServerMessage::Task { task: t.clone() });
    }

    pub fn announce_task_removed(&self, id: i64) {
        self.send(ServerMessage::TaskRemoved { id });
    }

    // ---- projects

    pub fn project_dir(&self, slug: &str) -> PathBuf {
        self.config.server.data_dir.join("projects").join(slug)
    }

    pub fn code_repo(&self, slug: &str) -> PathBuf {
        self.project_dir(slug).join("code.git")
    }

    pub fn corpus_repo(&self, slug: &str) -> PathBuf {
        self.project_dir(slug).join("corpus.git")
    }

    /// The checkout of `code.git` that the project's agents work in.
    pub fn project_workspace(&self, slug: &str) -> PathBuf {
        self.project_dir(slug).join("workspace")
    }

    pub fn reflex_log(&self, slug: &str) -> PathBuf {
        self.project_dir(slug).join("reflex.log")
    }

    /// Where a task or session of `project` runs: the project's workspace, or `workspace.dir`.
    pub fn work_dir(&self, project: Option<&str>) -> PathBuf {
        project.map_or_else(|| self.config.workspace.dir.clone(), |slug| self.project_workspace(slug))
    }

    /// The API as agents on this machine reach it (a wildcard bind is reached through loopback).
    pub fn local_api_url(&self) -> String {
        let bind = &self.config.server.bind;
        let port = bind.rsplit_once(':').map_or("4400", |(_, p)| p);
        let host = crate::api::host_name(bind);
        let host = if host.is_empty() || host == "0.0.0.0" || host == "::" { "127.0.0.1" } else { host };
        let host = if host.contains(':') { format!("[{host}]") } else { host.to_string() };
        format!("http://{host}:{port}/api")
    }

    pub async fn project_info(&self, project: Project) -> Result<ProjectInfo> {
        let stats = self.store.decision_stats(&project.slug).await?;
        Ok(ProjectInfo { reflex: self.reflex.status(&project.slug), stats, project })
    }

    pub async fn projects(&self) -> Result<Vec<ProjectInfo>> {
        let mut out = vec![];
        for p in self.store.projects().await? {
            out.push(self.project_info(p).await?);
        }
        Ok(out)
    }

    /// Pushes the project's current info (settings, Reflex status, decision counts) to the dashboard.
    pub async fn announce_project(&self, slug: &str) {
        match self.store.project(slug).await {
            Ok(Some(p)) => match self.project_info(p).await {
                Ok(project) => self.send(ServerMessage::Project { project }),
                Err(err) => tracing::warn!("project {slug}: {err:#}"),
            },
            Ok(None) => {}
            Err(err) => tracing::warn!("project {slug}: {err:#}"),
        }
    }

    pub async fn put_project(&self, p: &Project) -> Result<()> {
        self.store.save_project(p).await?;
        self.announce_project(&p.slug).await;
        Ok(())
    }

    pub fn announce_project_removed(&self, slug: &str) {
        self.send(ServerMessage::ProjectRemoved { slug: slug.into() });
    }

    /// Exports the corpus repo and (re)starts the project's Reflex, announcing each state change.
    pub async fn restart_reflex(&self, slug: &str) -> Result<ReflexStatus> {
        let project = self.store.project(slug).await?.with_context(|| format!("unknown project `{slug}`"))?;
        let live = self.project_dir(slug).join("corpus-live");
        let (repo, log) = (self.corpus_repo(slug), self.reflex_log(slug));
        let restart = self.reflex.restart(&self.config.reflex.binary, slug, project.port, reflex::Paths { corpus_repo: &repo, corpus_live: &live, log: &log });
        tokio::pin!(restart);
        // Announce "starting" as soon as it is set, then the outcome.
        let status = tokio::select! {
            s = &mut restart => s,
            _ = tokio::time::sleep(Duration::from_millis(50)) => {
                self.announce_project(slug).await;
                restart.await
            }
        };
        self.announce_project(slug).await;
        let level = if status.state == crate::model::ReflexState::Error { Level::Warning } else { Level::Info };
        self.event(level, "project.reflex", None, None, format!("{slug}: Reflex {:?} {}", status.state, status.message.lines().next().unwrap_or(""))).await;
        Ok(status)
    }

    // ---- orchestrator switch and provider cooldowns

    pub fn orchestrator_running(&self) -> bool {
        self.orchestrator_running.load(Ordering::SeqCst)
    }

    pub async fn set_orchestrator_running(&self, running: bool) {
        self.orchestrator_running.store(running, Ordering::SeqCst);
        let status = self.orchestrator_status().await;
        self.send(ServerMessage::Orchestrator { status });
    }

    pub async fn orchestrator_status(&self) -> OrchestratorStatus {
        let active = self
            .store
            .tasks()
            .await
            .map(|ts| ts.iter().filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Verifying | TaskStatus::Auditing)).count())
            .unwrap_or(0);
        OrchestratorStatus { running: self.orchestrator_running(), max_parallel: self.config.orchestrator.max_parallel, active }
    }

    /// Avoid `provider` until `until` (a quota reset or an auth failure). A later deadline wins.
    pub fn cool_down(&self, provider: &str, until: DateTime<Utc>) {
        let mut map = self.cooldowns.lock().unwrap();
        let entry = map.entry(provider.to_string()).or_insert(until);
        if until > *entry {
            *entry = until;
        }
    }

    pub fn cooldown_until(&self, provider: &str) -> Option<DateTime<Utc>> {
        let mut map = self.cooldowns.lock().unwrap();
        match map.get(provider) {
            Some(until) if *until > Utc::now() => Some(*until),
            Some(_) => {
                map.remove(provider);
                None
            }
            None => None,
        }
    }

    pub fn spec(&self, provider: &str) -> Result<&ProviderSpec> {
        self.config.providers.get(provider).with_context(|| format!("unknown provider `{provider}`"))
    }

    /// The provider's binary is on the workspace PATH (or the absolute path exists).
    pub fn provider_available(&self, provider: &str) -> bool {
        let Ok(spec) = self.spec(provider) else { return false };
        if spec.binary.contains('/') {
            return Path::new(&spec.binary).exists();
        }
        let path = self.config.workspace.env.get("PATH").cloned().or_else(|| std::env::var("PATH").ok()).unwrap_or_default();
        std::env::split_paths(&path).any(|dir| dir.join(&spec.binary).is_file())
    }

    pub fn providers(&self) -> Vec<ProviderInfo> {
        self.config
            .providers
            .iter()
            .map(|(name, spec)| ProviderInfo {
                name: name.clone(),
                models: spec.models.clone(),
                default_model: spec.default_model.clone(),
                available: self.provider_available(name),
                cooldown_until: self.cooldown_until(name),
            })
            .collect()
    }

    // ---- sessions

    pub fn run_dir(&self, session: &str) -> PathBuf {
        self.config.server.data_dir.join("runs").join(session)
    }

    pub fn result_file(&self, session: &str) -> PathBuf {
        self.run_dir(session).join("result.json")
    }

    pub(crate) fn resolve_model(spec: &ProviderSpec, requested: Option<&str>) -> Result<String> {
        let model = requested.filter(|m| !m.is_empty()).unwrap_or(&spec.default_model).to_string();
        if !spec.models.is_empty() && !spec.models.contains(&model) {
            bail!("model `{model}` is not in this provider's list ({})", spec.models.join(", "));
        }
        Ok(model)
    }

    /// Writes the run directory (prompt, launch script) and starts the agent in a new tmux session.
    pub async fn launch(self: &Arc<Self>, req: LaunchRequest) -> Result<Session> {
        let spec = self.spec(&req.provider)?.clone();
        let model = Self::resolve_model(&spec, req.model.as_deref())?;
        let suffix = match req.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(n) => n.to_string(),
            None => format!("{}-{}", req.provider, Utc::now().format("%H%M%S")),
        };
        let prefix = &self.config.workspace.session_prefix;
        let name = if suffix.starts_with(prefix.as_str()) { suffix } else { format!("{prefix}{suffix}") };
        if !NAME.is_match(&name) {
            bail!("session names use 1–64 letters, digits, _ or -");
        }
        if self.term.exists(&name).await? {
            bail!("session `{name}` already exists");
        }

        let dir = self.run_dir(&name);
        // A reused name must not inherit an old result file.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir)?;
        let prompt_file = dir.join("prompt.md");
        if let Some(prompt) = &req.prompt {
            std::fs::write(&prompt_file, prompt)?;
        }

        let substitute = |arg: &str| arg.replace("{binary}", &spec.binary).replace("{model}", &model).replace("{session}", &name);
        let mut argv: Vec<String> = spec.interactive.iter().map(|a| quote(&substitute(a))).collect::<Result<_>>()?;
        for a in &spec.extra_args {
            argv.push(quote(a)?);
        }
        if req.prompt.is_some() && spec.prompt_mode == PromptMode::Arg {
            argv.push(format!("\"$(cat {})\"", quote(prompt_file.to_str().context("non-UTF-8 path")?)?));
        }

        let mut script = String::from("#!/bin/sh\n");
        for (k, v) in &self.config.workspace.env {
            script.push_str(&format!("export {k}={}\n", quote(v)?));
        }
        script.push_str(&format!("export MAP_SESSION={}\n", quote(&name)?));
        script.push_str(&format!("export MAP_RUN_DIR={}\n", quote(dir.to_str().context("non-UTF-8 path")?)?));
        if req.task_id.is_some() {
            script.push_str(&format!("export MAP_RESULT_FILE={}\n", quote(self.result_file(&name).to_str().context("non-UTF-8 path")?)?));
        }
        if let Some(slug) = &req.project {
            // Agents ask the project's decision endpoint before spending an LLM call on a classification.
            script.push_str(&format!("export MAP_PROJECT={}\n", quote(slug)?));
            script.push_str(&format!("export MAP_DECIDE_URL={}\n", quote(&format!("{}/projects/{slug}/decide", self.local_api_url()))?));
        }
        script.push_str(&format!("exec {}\n", argv.join(" ")));
        let script_path = dir.join("launch.sh");
        std::fs::write(&script_path, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))?;
        }

        let cwd = self.work_dir(req.project.as_deref());
        if !cwd.is_dir() {
            bail!("workspace {} does not exist", cwd.display());
        }
        self.term.spawn(&name, &cwd, &script_path).await?;
        let now = Utc::now();
        let session = Session {
            name: name.clone(),
            provider: req.provider.clone(),
            model: model.clone(),
            role: req.role,
            task_id: req.task_id,
            project: req.project.clone(),
            state: SessionState::Starting,
            detail: None,
            rate_limit_reset_at: None,
            wakeups: 0,
            last_line: None,
            attached: false,
            created_at: now,
            last_activity_at: now,
        };
        self.put_session(&session).await?;
        self.event(Level::Info, "session.started", Some(&name), req.task_id, format!("Started {} ({model})", req.provider)).await;

        if req.prompt.is_some() && spec.prompt_mode == PromptMode::Send {
            // Multi-line text typed into a TUI would submit line by line, so point the agent at the file instead.
            let hub = Arc::clone(self);
            let line = format!("Read {} and carry out the instructions in it.", prompt_file.display());
            let wait = Duration::from_secs(spec.startup_secs);
            let target = name.clone();
            tokio::spawn(async move {
                tokio::time::sleep(wait).await;
                if let Err(err) = hub.term.send_text(&target, &line, true).await {
                    hub.event(Level::Error, "session.prompt_failed", Some(&target), None, format!("Could not send the initial prompt: {err:#}")).await;
                }
            });
        }
        Ok(session)
    }

    /// Ends a managed session. Without `force`, refuses while a person is attached or has typed on the input line.
    /// The final pane is saved next to the run's prompt before the kill.
    pub async fn close_session(&self, name: &str, force: bool, reason: &str) -> Result<EndDecision> {
        let Some(mut session) = self.store.session(name).await? else { bail!("unknown session `{name}`") };
        if self.term.exists(name).await? {
            let pane = self.term.capture(name, 3000, true).await.unwrap_or_default();
            if !force {
                let clients = self.term.list().await?.into_iter().find(|s| s.name == name).map_or(0, |s| s.attached_clients);
                let decision = end_decision(clients, &pane);
                if decision != EndDecision::End {
                    return Ok(decision);
                }
            }
            let dir = self.run_dir(name);
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("final-pane.txt"), crate::detect::strip_ansi(&pane));
            self.term.kill(name).await?;
        }
        session.state = SessionState::Exited;
        session.detail = Some(reason.to_string());
        session.attached = false;
        self.put_session(&session).await?;
        self.event(Level::Info, "session.closed", Some(name), session.task_id, format!("Closed: {reason}")).await;
        Ok(EndDecision::End)
    }

    pub async fn send_prompt(&self, name: &str, text: &str) -> Result<()> {
        self.managed(name).await?;
        self.term.send_text(name, text.trim(), true).await
    }

    /// The session exists in the store and is still running in tmux.
    pub async fn managed(&self, name: &str) -> Result<Session> {
        let session = self.store.session(name).await?.with_context(|| format!("unknown session `{name}`"))?;
        if !session.state.is_live() || !self.term.exists(name).await? {
            bail!("session `{name}` is not running");
        }
        Ok(session)
    }
}
