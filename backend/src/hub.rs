//! Shared state: config, store, terminal and the broadcast channel that feeds dashboard clients.
//! Every change to a session or task goes through here so the database and the live view never disagree.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use regex::Regex;
use tokio::sync::broadcast;

use crate::config::{Config, PromptMode, ProviderSpec};
use crate::detect::{EndDecision, end_decision};
use crate::model::{Event, Level, OrchestratorStatus, Project, ProjectInfo, ProviderInfo, ReflexStatus, Role, ServerInfo, ServerMessage, Session, SessionState, SourceKind, Task, TaskStatus};
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
    /// Tasks at once across all projects: `[orchestrator] max_parallel`, or the dashboard's value.
    max_parallel: AtomicUsize,
    /// Runs one orchestrator tick on request (`POST /api/orchestrator/step`).
    pub step: crate::decisions::Stepper,
    cooldowns: Mutex<HashMap<String, DateTime<Utc>>>,
    pub reflex: reflex::Manager,
    pub http: reqwest::Client,
    /// The latest plan-usage report (`usage.rs`); also feeds the skip line in [`Hub::cooldown_until`].
    pub usage: crate::usage::UsageState,
    /// The latest update check of the agent CLIs (`updates.rs`).
    pub updates: crate::updates::UpdatesState,
    /// Tasks whose verify/audit/land step is running in the background (the orchestrator loop and the owner's
    /// Merge button share it, so a task is never finished twice at once).
    pub finishing: Arc<Mutex<std::collections::HashSet<i64>>>,
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
        let usage = crate::usage::UsageState::new(&config);
        let updates = crate::updates::UpdatesState::new(&config);
        let max_parallel = config.orchestrator.max_parallel;
        Arc::new(Self {
            config,
            store,
            term,
            tx,
            orchestrator_running: AtomicBool::new(running),
            max_parallel: AtomicUsize::new(max_parallel),
            step: Default::default(),
            cooldowns: Mutex::new(HashMap::new()),
            reflex: reflex::Manager::default(),
            http: reqwest::Client::new(),
            usage,
            updates,
            finishing: Default::default(),
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

    pub fn corpus_repo(&self, slug: &str) -> PathBuf {
        self.project_dir(slug).join("corpus.git")
    }

    /// The folder the project's agents work in: its folder on this machine, or for a legacy `git` project the
    /// checkout the platform made of it.
    pub fn workspace_of(&self, project: &Project) -> PathBuf {
        match (project.settings.source.kind, project.settings.source.location.as_deref()) {
            (SourceKind::Empty | SourceKind::Local | SourceKind::Clone, Some(path)) => crate::login::expand_home(path),
            _ => self.project_dir(&project.slug).join("workspace"),
        }
    }

    /// MAP's SSH key, known_hosts, identity and gitconfig under `<data_dir>/git/` (`git_access.rs`).
    pub fn git_files(&self) -> crate::git_access::GitFiles {
        crate::git_access::GitFiles::new(&self.config.server.data_dir)
    }

    /// MAP's gitconfig, brought up to date; exported as `GIT_CONFIG_GLOBAL` to agents and used by MAP's own git
    /// commands in project folders.
    pub fn gitconfig(&self) -> PathBuf {
        let files = self.git_files();
        crate::git_access::ensure(&files).unwrap_or_else(|err| {
            tracing::warn!("MAP gitconfig: {err:#}");
            files.gitconfig()
        })
    }

    /// The workspace env plus `GIT_CONFIG_GLOBAL`, for commands run in a project's folder (verify, post_done, audit).
    /// A `GIT_CONFIG_GLOBAL` set in `workspace.env` wins.
    pub fn command_env(&self) -> Vec<(String, String)> {
        let mut env = vec![("GIT_CONFIG_GLOBAL".to_string(), self.gitconfig().display().to_string())];
        env.extend(self.config.workspace.env.iter().map(|(k, v)| (k.clone(), v.clone())));
        env
    }

    pub fn reflex_log(&self, slug: &str) -> PathBuf {
        self.project_dir(slug).join("reflex.log")
    }

    /// Where a task or session of `project` runs: the project's folder, or `workspace.dir`.
    pub async fn work_dir(&self, project: Option<&str>) -> PathBuf {
        let Some(slug) = project else { return self.config.workspace.dir.clone() };
        match self.store.project(slug).await {
            Ok(Some(p)) => self.workspace_of(&p),
            _ => self.project_dir(slug).join("workspace"),
        }
    }

    /// Where `task` runs: its worktree while it has one (`branches.mode = worktree`), else [`Hub::work_dir`].
    pub async fn task_dir(&self, task: &crate::model::Task) -> PathBuf {
        match task.git.as_ref().and_then(|g| g.worktree.as_deref()).map(PathBuf::from) {
            Some(wt) if wt.is_dir() => wt,
            _ => self.work_dir(task.project.as_deref()).await,
        }
    }

    /// The token an agent of `slug` gets as `$MAP_PROJECT_TOKEN`: HMAC-SHA256 of the slug under `api_token`. It opens
    /// only that project's decision endpoints and its corpus (see `api::project_scope`), never the rest of the API.
    /// `None` without an `api_token`, when the API is open anyway.
    pub fn project_token(&self, slug: &str) -> Option<String> {
        self.derived_token(b"map-project:", slug)
    }

    /// The default workspace's planner token (`$MAP_PROJECT_TOKEN` of `<prefix>plan`): it opens only
    /// `/api/workspace/plans` (see `api::workspace_scope`). `None` without an `api_token`.
    pub fn workspace_token(&self) -> Option<String> {
        self.derived_token(b"map-workspace:", "plans")
    }

    fn derived_token(&self, label: &[u8], value: &str) -> Option<String> {
        use hmac::{KeyInit, Mac};
        let key = self.config.server.api_token.as_deref()?;
        let mut mac = hmac::Hmac::<sha2::Sha256>::new_from_slice(key.as_bytes()).expect("HMAC takes any key length");
        mac.update(label);
        mac.update(value.as_bytes());
        Some(mac.finalize().into_bytes().iter().map(|b| format!("{b:02x}")).collect())
    }

    pub fn announce_plan(&self, plan: &crate::model::Plan) {
        self.send(ServerMessage::Plan { plan: Box::new(plan.clone()) });
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
        let folder = self.workspace_of(&project);
        // Only folder projects are judged: a legacy `git` project's workspace is MAP's own.
        let (root, outside_roots) = match project.settings.source.kind {
            SourceKind::Git => (None, false),
            _ => {
                let root = crate::roots::root_name(self.config.projects.roots(), &folder);
                let outside = root.is_none();
                (root, outside)
            }
        };
        Ok(ProjectInfo { reflex: self.reflex.status(&project.slug), stats, project, workspace: folder.display().to_string(), root, outside_roots })
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
                Ok(project) => self.send(ServerMessage::Project { project: Box::new(project) }),
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
            .map(|ts| ts.iter().filter(|t| matches!(t.status, TaskStatus::Running | TaskStatus::Verifying | TaskStatus::Auditing | TaskStatus::Landing)).count())
            .unwrap_or(0);
        OrchestratorStatus { running: self.orchestrator_running(), max_parallel: self.max_parallel(), config_max_parallel: self.config.orchestrator.max_parallel, active }
    }

    pub fn max_parallel(&self) -> usize {
        self.max_parallel.load(Ordering::Relaxed)
    }

    /// Sets the machine-wide task limit from the dashboard (kept in the database across restarts); `None` goes back
    /// to map.toml's value.
    pub async fn set_max_parallel(&self, n: Option<usize>) -> Result<()> {
        if n == Some(0) {
            bail!("max_parallel must be at least 1");
        }
        self.store.set_runtime_setting("orchestrator.max_parallel", n.map(|n| n.to_string()).as_deref()).await?;
        self.max_parallel.store(n.unwrap_or(self.config.orchestrator.max_parallel), Ordering::Relaxed);
        Ok(())
    }

    /// Settings the dashboard changed earlier, read once at startup.
    pub async fn load_runtime_settings(&self) {
        if let Ok(Some(v)) = self.store.runtime_setting("orchestrator.max_parallel").await
            && let Ok(n) = v.parse::<usize>()
            && n > 0
        {
            self.max_parallel.store(n, Ordering::Relaxed);
        }
    }

    /// Avoid `provider` until `until` (a quota reset or an auth failure). A later deadline wins.
    pub fn cool_down(&self, provider: &str, until: DateTime<Utc>) {
        let mut map = self.cooldowns.lock().unwrap();
        let entry = map.entry(provider.to_string()).or_insert(until);
        if until > *entry {
            *entry = until;
        }
    }

    /// Also set while a plan-usage window is at or above `usage.skip_pct` (until it resets).
    pub fn cooldown_until(&self, provider: &str) -> Option<DateTime<Utc>> {
        let mut map = self.cooldowns.lock().unwrap();
        let manual = match map.get(provider) {
            Some(until) if *until > Utc::now() => Some(*until),
            Some(_) => {
                map.remove(provider);
                None
            }
            None => None,
        };
        manual.max(self.usage.skip_until(provider, &self.config))
    }

    pub fn announce_usage(&self, report: crate::model::UsageReport) {
        self.send(ServerMessage::Usage { report });
    }

    pub fn announce_updates(&self, report: crate::model::UpdatesReport) {
        self.send(ServerMessage::Updates { report });
    }

    pub fn spec(&self, provider: &str) -> Result<&ProviderSpec> {
        self.config.providers.get(provider).with_context(|| format!("unknown provider `{provider}`"))
    }

    /// The provider CLI's state folder (sign-in, settings, history) as the CLI sees it: see [`Config::provider_home`].
    pub fn provider_home(&self, provider: &str) -> Option<PathBuf> {
        self.config.provider_home(provider)
    }

    /// The variables every run of `provider` gets before `[workspace.env]` (isolated mode only): see
    /// [`Config::provider_env`].
    pub fn provider_env(&self, provider: &str) -> Vec<(String, String)> {
        self.config.provider_env(provider)
    }

    /// Every variable a command MAP runs itself for `provider` gets, in order: [`Hub::provider_env`], `extra`
    /// (`[workspace.env]`, or [`Hub::command_env`]), then the provider's own `env` with its references resolved in
    /// memory ([`Config::provider_fixed_env`]).
    pub fn run_env(&self, provider: &str, extra: Vec<(String, String)>) -> Vec<(String, String)> {
        let mut env = self.provider_env(provider);
        env.extend(extra);
        env.extend(self.config.provider_fixed_env(provider));
        env
    }

    /// The provider's binary is on the workspace PATH (or the absolute path exists).
    pub fn provider_available(&self, provider: &str) -> bool {
        self.spec(provider).is_ok_and(|spec| self.binary_path(&spec.binary).is_some())
    }

    /// Where `binary` is: the path itself when it has a `/`, else the first match on the workspace PATH.
    pub fn binary_path(&self, binary: &str) -> Option<PathBuf> {
        if binary.contains('/') {
            return Path::new(binary).exists().then(|| PathBuf::from(binary));
        }
        let path = self.config.workspace.env.get("PATH").cloned().or_else(|| std::env::var("PATH").ok()).unwrap_or_default();
        std::env::split_paths(&path).map(|dir| dir.join(binary)).find(|p| p.is_file())
    }

    pub fn server_info(&self) -> ServerInfo {
        ServerInfo {
            version: env!("CARGO_PKG_VERSION").into(),
            workspace_dir: self.config.workspace.dir.display().to_string(),
            session_prefix: self.config.workspace.session_prefix.clone(),
            database: self.config.server.data_dir.join("map.db").display().to_string(),
            watcher_interval_secs: self.config.watcher.interval_secs,
            reflex_version: self.reflex.version(&self.config.reflex.binary),
            tmux_socket: self.config.workspace.tmux_socket.clone(),
            api_fallback: self.config.reflex.api_name(),
        }
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
                signed_in: crate::login::signed_in(self, spec),
                login: crate::login::info(self, name, spec),
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
        self.launch_command(req, None).await
    }

    /// [`Hub::launch`] with `command` (a template like `interactive`) in place of the provider's interactive
    /// command, without its `extra_args` or prompt argument; used for the provider's sign-in.
    pub async fn launch_command(self: &Arc<Self>, req: LaunchRequest, command: Option<&[String]>) -> Result<Session> {
        self.launch_with(req, command, &[]).await
    }

    /// [`Hub::launch`] with `env` exported after MAP's own variables (the planner's plans URL and token).
    pub async fn launch_env(self: &Arc<Self>, req: LaunchRequest, env: &[(String, String)]) -> Result<Session> {
        self.launch_with(req, None, env).await
    }

    async fn launch_with(self: &Arc<Self>, req: LaunchRequest, command: Option<&[String]>, env: &[(String, String)]) -> Result<Session> {
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
        let mut argv: Vec<String> = command.unwrap_or(&spec.interactive).iter().map(|a| quote(&substitute(a))).collect::<Result<_>>()?;
        for a in spec.extra_args.iter().filter(|_| command.is_none()) {
            argv.push(quote(a)?);
        }
        if req.prompt.is_some() && spec.prompt_mode == PromptMode::Arg && command.is_none() {
            argv.push(format!("\"$(cat {})\"", quote(prompt_file.to_str().context("non-UTF-8 path")?)?));
        }

        let mut script = String::from("#!/bin/sh\n");
        // MAP's git defaults (key, known hosts, identity; the user's own ~/.gitconfig included after them). Before
        // workspace.env, so a GIT_CONFIG_GLOBAL set there wins.
        script.push_str(&format!("export GIT_CONFIG_GLOBAL={}\n", quote(self.gitconfig().to_str().context("non-UTF-8 path")?)?));
        // `[agents_home] mode = "isolated"`: the CLI's own state folder (CLAUDE_CONFIG_DIR, CODEX_HOME, …), for every
        // launch of the provider, sign-in and update helpers included. Nothing in shared mode.
        for (k, v) in self.provider_env(&req.provider) {
            script.push_str(&format!("export {k}={}\n", quote(&v)?));
        }
        for (k, v) in &self.config.workspace.env {
            script.push_str(&format!("export {k}={}\n", quote(v)?));
        }
        // The provider's own variables (openrouter's endpoint and key), after workspace.env so they win over it. A
        // secret is exported by reference (`"$OPENROUTER_API_KEY"`), never by value.
        for line in self.config.provider_script_env(&req.provider) {
            script.push_str(&line);
            script.push('\n');
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
            script.push_str(&format!("export MAP_API_URL={}\n", quote(&self.local_api_url())?));
            script.push_str(&format!("export MAP_KNOWLEDGE_URL={}\n", quote(&format!("{}/projects/{slug}/knowledge", self.local_api_url()))?));
            script.push_str(&format!("export MAP_PROJECT_TOKEN={}\n", quote(&self.project_token(slug).unwrap_or_default())?));
        }
        for (k, v) in env {
            script.push_str(&format!("export {k}={}\n", quote(v)?));
        }
        script.push_str(&format!("exec {}\n", argv.join(" ")));
        let script_path = dir.join("launch.sh");
        std::fs::write(&script_path, &script)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))?;
        }

        // A task's worker runs in the task's worktree when it has one.
        let cwd = match req.task_id {
            Some(id) => match self.store.task(id).await? {
                Some(task) => self.task_dir(&task).await,
                None => self.work_dir(req.project.as_deref()).await,
            },
            None => self.work_dir(req.project.as_deref()).await,
        };
        if !cwd.is_dir() {
            bail!("workspace {} does not exist", cwd.display());
        }
        if command.is_none() {
            // So the CLI does not stop at its "trust this folder?" question (sign-in and update helpers need not).
            crate::trust::ensure(self, &req.provider, &cwd, req.task_id).await;
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
