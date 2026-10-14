//! Folder trust. Claude Code, Codex and agy each ask once per folder whether to trust it, and an unattended worker
//! waits at that question forever. Two layers keep tasks from sitting there:
//!
//! 1. Before `Hub::launch` starts a provider in a folder, MAP marks the folder trusted in the CLI's own config
//!    (`[providers.<name>.trust]`; off with `workspace.trust_folders = false`), never touching other keys.
//! 2. A question that still shows is the session state `needs_trust` (`detect::trust_prompt`), and
//!    `POST /api/sessions/{name}/trust` ([`answer`]) records the folder and picks the CLI's "trust" option.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::config::{Config, TrustFormat, TrustSpec};
use crate::detect::{EndDecision, end_decision, trust_prompt};
use crate::hub::Hub;
use crate::model::{CheckLine, FolderTrust, Level, ProviderTrust, SessionState, TrustAnswer};

/// Attempts at a write that a concurrent rewrite of the file (Claude Code rewrites `.claude.json` often) undid.
const ATTEMPTS: usize = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marked {
    /// The file already said the folder is trusted; nothing was written.
    Already,
    /// MAP wrote it now.
    Now,
    /// The file marks the folder untrusted on purpose (Codex `trust_level = "untrusted"`); left as it is.
    Refused(String),
}

/// The file `spec` points at: `<$dir_env>/<dir_file>` when the variable is set in `[workspace.env]` (which wins) or
/// in MAP's environment, else `file`. `~/` is the home directory.
pub fn trust_file(config: &Config, spec: &TrustSpec) -> PathBuf {
    let dir = spec.dir_env.as_deref().and_then(|var| {
        config.workspace.env.get(var).cloned().filter(|v| !v.is_empty()).or_else(|| std::env::var(var).ok().filter(|v| !v.is_empty()))
    });
    match (dir, spec.dir_file.as_deref()) {
        (Some(dir), Some(file)) => crate::login::expand_home(&dir).join(file),
        _ => crate::login::expand_home(&spec.file),
    }
}

/// The key the CLIs look a folder up by: its canonical absolute path.
pub fn folder_key(folder: &Path) -> Result<String> {
    let path = std::fs::canonicalize(folder).with_context(|| format!("resolving {}", folder.display()))?;
    path.to_str().map(String::from).with_context(|| format!("{} is not UTF-8", path.display()))
}

/// Marks `folder` (a canonical path) trusted in `file`, in `format`.
pub fn mark(format: TrustFormat, file: &Path, folder: &str) -> Result<Marked> {
    #[cfg(test)]
    if !file.starts_with(std::env::temp_dir()) {
        bail!("tests only write trust files under the temp dir, not {}", file.display());
    }
    // A symlinked config is written where it points, so the link survives.
    let file = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
    for _ in 0..ATTEMPTS {
        let original = match std::fs::read(&file) {
            Ok(bytes) => Some(bytes),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err).with_context(|| format!("reading {}", file.display())),
        };
        let text = std::str::from_utf8(original.as_deref().unwrap_or_default()).with_context(|| format!("{} is not UTF-8; left alone", file.display()))?;
        let (state, updated) = match format {
            TrustFormat::ClaudeJson => claude_json(text, folder),
            TrustFormat::CodexToml => codex_toml(text, folder),
        }
        .with_context(|| format!("{}; left alone", file.display()))?;
        let Some(updated) = updated else { return Ok(state) };
        if !replace_if_unchanged(&file, original.as_deref(), updated.as_bytes())? {
            continue;
        }
        // The CLI may have renamed its own copy over ours in the meantime.
        let now = std::fs::read_to_string(&file).unwrap_or_default();
        let kept = match format {
            TrustFormat::ClaudeJson => claude_json(&now, folder),
            TrustFormat::CodexToml => codex_toml(&now, folder),
        };
        if matches!(kept, Ok((Marked::Already, None))) {
            return Ok(Marked::Now);
        }
    }
    bail!("{} kept changing while MAP wrote it; the CLI may still ask about {folder}", file.display())
}

/// Claude Code's `.claude.json` with `projects[folder].hasTrustDialogAccepted = true`; `None` when it already is.
/// Every other key, project and value is kept (and in its order).
fn claude_json(text: &str, folder: &str) -> Result<(Marked, Option<String>)> {
    let mut doc: Value = if text.trim().is_empty() { Value::Object(Map::new()) } else { serde_json::from_str(text).context("not valid JSON")? };
    let root = doc.as_object_mut().context("not a JSON object")?;
    let projects = root.entry("projects").or_insert_with(|| Value::Object(Map::new())).as_object_mut().context("`projects` is not an object")?;
    let project = projects.entry(folder).or_insert_with(|| Value::Object(Map::new())).as_object_mut().with_context(|| format!("`projects[{folder:?}]` is not an object"))?;
    if project.get("hasTrustDialogAccepted") == Some(&Value::Bool(true)) {
        return Ok((Marked::Already, None));
    }
    project.insert("hasTrustDialogAccepted".into(), Value::Bool(true));
    // Claude Code writes it with JSON.stringify(config, null, 2).
    Ok((Marked::Now, Some(serde_json::to_string_pretty(&doc)?)))
}

/// Codex's `config.toml` with `[projects."<folder>"] trust_level = "trusted"`, comments and layout kept; `None` when
/// it already is, or when the folder is marked untrusted on purpose.
fn codex_toml(text: &str, folder: &str) -> Result<(Marked, Option<String>)> {
    let mut doc: toml_edit::DocumentMut = text.parse().context("not valid TOML")?;
    let projects = doc.as_table_mut().entry("projects").or_insert_with(|| {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        toml_edit::Item::Table(t)
    });
    let projects = projects.as_table_like_mut().context("`projects` is not a table")?;
    let project = projects.entry(folder).or_insert(toml_edit::Item::Table(toml_edit::Table::new()));
    let project = project.as_table_like_mut().with_context(|| format!("`projects.{folder:?}` is not a table"))?;
    match project.get("trust_level").map(|v| v.as_str()) {
        Some(Some("trusted")) => return Ok((Marked::Already, None)),
        Some(Some(level)) => return Ok((Marked::Refused(format!("trust_level = {level:?}")), None)),
        Some(None) => bail!("`projects.{folder:?}.trust_level` is not a string"),
        None => {}
    }
    project.insert("trust_level", toml_edit::value("trusted"));
    Ok((Marked::Now, Some(doc.to_string())))
}

/// Writes `bytes` to a temporary file next to `path` and renames it over `path`, keeping its mode, unless `path`
/// no longer holds `expected` (`None`: it did not exist). False when it changed, so the caller reads it again.
fn replace_if_unchanged(path: &Path, expected: Option<&[u8]>, bytes: &[u8]) -> Result<bool> {
    let dir = path.parent().context("no parent folder")?;
    if !dir.is_dir() {
        // The CLI is installed but has not run yet (no ~/.codex); it creates the same folder itself.
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
        }
    }
    let name = path.file_name().context("no file name")?.to_string_lossy();
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.subsec_nanos());
    let tmp = dir.join(format!(".{name}.map-{}-{nanos}.tmp", std::process::id()));
    let write = || -> Result<bool> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            let mode = std::fs::metadata(path).map_or(0o600, |m| m.permissions().mode() & 0o7777);
            options.mode(mode);
            let mut f = options.open(&tmp)?;
            // The umask may have narrowed `mode`.
            f.set_permissions(std::fs::Permissions::from_mode(mode))?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        #[cfg(not(unix))]
        {
            let mut f = options.open(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
        }
        let current = match std::fs::read(path) {
            Ok(b) => Some(b),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(err) => return Err(err.into()),
        };
        if current.as_deref() != expected {
            return Ok(false);
        }
        std::fs::rename(&tmp, path)?;
        Ok(true)
    };
    let result = write().with_context(|| format!("writing {}", path.display()));
    if !matches!(result, Ok(true)) {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Whether `file` says `folder` (a canonical path) is trusted; a missing file says no. Reads only.
pub fn is_trusted(format: TrustFormat, file: &Path, folder: &str) -> Result<bool> {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err).with_context(|| format!("reading {}", file.display())),
    };
    let (state, _) = match format {
        TrustFormat::ClaudeJson => claude_json(&text, folder),
        TrustFormat::CodexToml => codex_toml(&text, folder),
    }
    .with_context(|| file.display().to_string())?;
    Ok(state == Marked::Already)
}

/// What layer 1 did for one provider and folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// `workspace.trust_folders = false`.
    Off,
    /// The provider has no `trust` file MAP knows how to write.
    Unsupported,
    Done { file: PathBuf, marked: Marked },
    Failed(String),
}

impl Outcome {
    pub fn text(&self) -> String {
        match self {
            Outcome::Off => "not written (workspace.trust_folders = false)".into(),
            Outcome::Unsupported => "not written (MAP cannot mark folders for this CLI)".into(),
            Outcome::Done { file, marked: Marked::Already } => format!("already trusted in {}", file.display()),
            Outcome::Done { file, marked: Marked::Now } => format!("marked in {}", file.display()),
            Outcome::Done { file, marked: Marked::Refused(why) } => format!("not written: {} has {why}", file.display()),
            Outcome::Failed(why) => format!("not written: {why}"),
        }
    }
}

/// Layer 1 for one provider: mark `folder` trusted in the CLI's config. Never fails its caller (a launch, a project
/// creation): an unreadable or unparsable file is left alone with a warning event, and the watcher catches the
/// question if the CLI asks. Idempotent; an event is recorded only when MAP writes.
pub async fn ensure(hub: &Hub, provider: &str, folder: &Path, task_id: Option<i64>) -> Outcome {
    if !hub.config.workspace.trust_folders {
        return Outcome::Off;
    }
    let Some(spec) = hub.config.providers.get(provider).and_then(|s| s.trust.as_ref()) else { return Outcome::Unsupported };
    let file = trust_file(&hub.config, spec);
    // Tests never touch a real home directory's CLI config.
    #[cfg(test)]
    if !file.starts_with(std::env::temp_dir()) {
        return Outcome::Failed(format!("tests do not write {}", file.display()));
    }
    let key = match folder_key(folder) {
        Ok(k) => k,
        Err(err) => return Outcome::Failed(format!("{err:#}")),
    };
    let (format, path, k) = (spec.format, file.clone(), key.clone());
    let marked = tokio::task::spawn_blocking(move || mark(format, &path, &k)).await.unwrap_or_else(|err| Err(anyhow::anyhow!("{err}")));
    match marked {
        Ok(marked) => {
            match &marked {
                Marked::Now => hub.event(Level::Info, "folder.trusted", None, task_id, format!("Marked {key} as trusted for {provider} in {}", file.display())).await,
                Marked::Refused(why) => hub.event(Level::Warning, "folder.trust_refused", None, task_id, format!("{} has {why} for {key}; MAP leaves it as it is", file.display())).await,
                Marked::Already => return Outcome::Done { file, marked },
            };
            Outcome::Done { file, marked }
        }
        Err(err) => {
            let why = format!("{err:#}");
            hub.event(Level::Warning, "folder.trust_failed", None, task_id, format!("Could not mark {key} as trusted for {provider}: {why}. The CLI may ask; answer it from the session.")).await;
            Outcome::Failed(why)
        }
    }
}

/// Providers MAP marks folders trusted for: those with a `trust` file whose CLI is installed.
pub fn pretrust_providers(hub: &Hub) -> Vec<String> {
    hub.config.providers.iter().filter(|(name, spec)| spec.trust.is_some() && hub.provider_available(name)).map(|(name, _)| name.clone()).collect()
}

/// Layer 1 for every provider in [`pretrust_providers`], as check lines: when a project is created, at server start
/// for `workspace.dir`, and from the project page. A failed write is a warning line, never an error.
pub async fn trust_folder(hub: &Hub, folder: &Path) -> Vec<CheckLine> {
    let shown = folder_key(folder).unwrap_or_else(|_| folder.display().to_string());
    if !hub.config.workspace.trust_folders {
        return vec![CheckLine { level: Level::Info, text: format!("folder trust: off (workspace.trust_folders = false) · the CLIs ask about {shown} once") }];
    }
    let providers = pretrust_providers(hub);
    if providers.is_empty() {
        return vec![CheckLine { level: Level::Info, text: "folder trust: no installed CLI MAP can mark folders for".into() }];
    }
    let (mut trusted, mut lines) = (vec![], vec![]);
    for provider in providers {
        match ensure(hub, &provider, folder, None).await {
            Outcome::Done { marked: Marked::Now | Marked::Already, .. } => trusted.push(provider),
            other => lines.push(CheckLine { level: Level::Warning, text: format!("could not trust {shown} for {provider}: {}", other.text()) }),
        }
    }
    if !trusted.is_empty() {
        lines.insert(0, CheckLine { level: Level::Success, text: format!("trusted {shown} for {}", trusted.join(", ")) });
    }
    lines
}

/// At server start: the default workspace, so tasks without a project do not stop at the question either.
pub async fn trust_workspace(hub: &Hub) -> Vec<CheckLine> {
    let lines = trust_folder(hub, &hub.config.workspace.dir).await;
    for line in &lines {
        match line.level {
            Level::Warning | Level::Error => tracing::warn!("{}", line.text),
            _ => tracing::info!("{}", line.text),
        }
    }
    lines
}

/// Read-only: whether `folder` is trusted for each installed provider, for the project page.
pub fn status(hub: &Hub, folder: &Path) -> FolderTrust {
    let key = folder_key(folder);
    let providers = hub
        .config
        .providers
        .iter()
        .filter(|(name, _)| hub.provider_available(name))
        .map(|(name, spec)| {
            let Some(trust) = &spec.trust else {
                return ProviderTrust { provider: name.clone(), file: None, trusted: None, note: "MAP cannot mark folders for this CLI; it asks once, answer it from the session".into() };
            };
            let file = trust_file(&hub.config, trust);
            let (trusted, note) = match key.as_ref().map_err(|e| anyhow::anyhow!("{e:#}")).and_then(|k| is_trusted(trust.format, &file, k)) {
                Ok(true) => (Some(true), "trusted".to_string()),
                Ok(false) => (Some(false), "not trusted: the CLI will ask".to_string()),
                Err(err) => (None, format!("{err:#}")),
            };
            ProviderTrust { provider: name.clone(), file: Some(file.display().to_string()), trusted, note }
        })
        .collect();
    FolderTrust { folder: key.unwrap_or_else(|_| folder.display().to_string()), enabled: hub.config.workspace.trust_folders, providers }
}

#[derive(Debug)]
pub enum AnswerError {
    NotFound(String),
    /// The question is not on screen, a person is in the session, or the selection did not land on the option.
    Conflict(String),
    Failed(anyhow::Error),
}

impl From<anyhow::Error> for AnswerError {
    fn from(err: anyhow::Error) -> Self {
        AnswerError::Failed(err)
    }
}

/// How long [`answer`] waits for the pane to show the effect of a key.
const SETTLE: Duration = Duration::from_millis(150);

/// Layer 2, the owner's "Trust this folder" click: only while the session's pane shows the CLI's trust question and
/// nobody is attached or typing, record the folder (as [`ensure`] does) and move the selection to the option that
/// trusts it, check the pane, press Enter.
pub async fn answer(hub: &Arc<Hub>, name: &str) -> Result<TrustAnswer, AnswerError> {
    let mut session = hub.store.session(name).await?.ok_or_else(|| AnswerError::NotFound(format!("session `{name}` not found")))?;
    if !session.state.is_live() || !hub.term.exists(name).await? {
        return Err(AnswerError::Conflict(format!("session `{name}` is not running")));
    }
    let spec = hub.spec(&session.provider)?.clone();
    let pane = hub.term.capture(name, 200, true).await?;
    let Some(prompt) = trust_prompt(&spec, &pane) else {
        return Err(AnswerError::Conflict("No folder-trust question is on the session's screen".into()));
    };
    let clients = hub.term.list().await?.into_iter().find(|s| s.name == name).map_or(0, |s| s.attached_clients);
    match end_decision(clients, &prompt.above) {
        EndDecision::End => {}
        EndDecision::KeepAttached => return Err(AnswerError::Conflict("A terminal is attached to this session; answer the question there".into())),
        EndDecision::KeepInput => return Err(AnswerError::Conflict("Someone has typed in this session; answer the question there".into())),
    }
    let Some(moves) = prompt.moves else {
        return Err(AnswerError::Conflict(format!("Could not find the selected option or \"{}\" on screen; answer it in the console", spec.trust_accept.join("\" / \""))));
    };

    let dir = hub.work_dir(session.project.as_deref()).await;
    let folder = folder_key(&dir).unwrap_or_else(|_| dir.display().to_string());
    let recorded = ensure(hub, &session.provider, &dir, session.task_id).await.text();

    let mut keys = vec![];
    let step = if moves > 0 { "Down" } else { "Up" };
    for _ in 0..moves.unsigned_abs() {
        hub.term.send_key(name, step).await?;
        keys.push(step.to_string());
    }
    if moves != 0 {
        tokio::time::sleep(SETTLE).await;
        let now = hub.term.capture(name, 200, true).await?;
        if trust_prompt(&spec, &now).and_then(|p| p.moves) != Some(0) {
            return Err(AnswerError::Conflict(format!("Sent {} but the selection is not on \"{}\"; answer it in the console", keys.join(" "), spec.trust_accept.join("\" / \""))));
        }
    }
    hub.term.send_key(name, "Enter").await?;
    keys.push("Enter".into());
    let mut answered = false;
    for _ in 0..10 {
        tokio::time::sleep(SETTLE).await;
        if trust_prompt(&spec, &hub.term.capture(name, 200, true).await?).is_none() {
            answered = true;
            break;
        }
    }
    let how = if answered { "answered" } else { "sent the answer, but the question is still on screen" };
    hub.event(Level::Info, "session.trusted", Some(name), session.task_id, format!("Trust {folder} for {}: {how} ({}); {recorded}", session.provider, keys.join(" "))).await;
    if answered && session.state == SessionState::NeedsTrust {
        session.state = SessionState::Working;
        session.detail = None;
        hub.put_session(&session).await?;
    }
    Ok(TrustAnswer { session: name.into(), folder, recorded, keys, answered })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::hub::LaunchRequest;
    use crate::model::{ProjectSettings, ProjectSource, Role, SourceKind};
    use crate::projects::{NewProject, create_with};
    use crate::store::Store;
    use crate::terminal::fake::FakeTerminal;

    /// A hub whose Claude and Codex configs live in the temp dir (`CLAUDE_CONFIG_DIR`, `CODEX_HOME` in
    /// `[workspace.env]`), and whose `claude` and `codex` are `/bin/sh` so they count as installed.
    async fn setup(workspace: &str) -> (Arc<Hub>, Arc<FakeTerminal>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().display();
        for sub in ["ws", "claude", "codex"] {
            std::fs::create_dir_all(dir.path().join(sub)).unwrap();
        }
        let text = format!(
            "[workspace]\ndir = \"{d}/ws\"\n{workspace}\n[workspace.env]\nCLAUDE_CONFIG_DIR = \"{d}/claude\"\nCODEX_HOME = \"{d}/codex\"\n[server]\ndata_dir = \"{d}/data\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"/bin/sh\"\n[providers.codex]\nbinary = \"/bin/sh\"\n"
        );
        let term = Arc::new(FakeTerminal::default());
        let hub = Hub::new(Config::parse(&text, Path::new("/")).unwrap(), Store::memory().await.unwrap(), term.clone());
        (hub, term, dir)
    }

    fn claude_file(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("claude/.claude.json")
    }

    fn codex_file(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("codex/config.toml")
    }

    fn claude_trusts(dir: &tempfile::TempDir, folder: &Path) -> bool {
        is_trusted(TrustFormat::ClaudeJson, &claude_file(dir), &folder_key(folder).unwrap()).unwrap()
    }

    fn codex_trusts(dir: &tempfile::TempDir, folder: &Path) -> bool {
        is_trusted(TrustFormat::CodexToml, &codex_file(dir), &folder_key(folder).unwrap()).unwrap()
    }

    #[test]
    fn claude_json_keeps_other_keys_and_projects_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".claude.json");
        let before = r#"{"numStartups": 7, "projects": {"/other": {"allowedTools": ["Bash"], "hasTrustDialogAccepted": false}, "/w": {"history": [1]}}, "theme": "dark"}"#;
        std::fs::write(&file, before).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert_eq!(mark(TrustFormat::ClaudeJson, &file, "/w").unwrap(), Marked::Now);
        let after: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(after["numStartups"], 7);
        assert_eq!(after["theme"], "dark");
        assert_eq!(after["projects"]["/other"], serde_json::json!({"allowedTools": ["Bash"], "hasTrustDialogAccepted": false}));
        assert_eq!(after["projects"]["/w"], serde_json::json!({"history": [1], "hasTrustDialogAccepted": true}));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Idempotent: the second call writes nothing.
        let bytes = std::fs::read(&file).unwrap();
        assert_eq!(mark(TrustFormat::ClaudeJson, &file, "/w").unwrap(), Marked::Already);
        assert_eq!(std::fs::read(&file).unwrap(), bytes);
        // A new project object; no stray temporary files.
        assert_eq!(mark(TrustFormat::ClaudeJson, &file, "/new").unwrap(), Marked::Now);
        assert!(is_trusted(TrustFormat::ClaudeJson, &file, "/new").unwrap());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn unparsable_json_is_never_written() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(".claude.json");
        std::fs::write(&file, "{\"projects\": {").unwrap();
        let err = mark(TrustFormat::ClaudeJson, &file, "/w").unwrap_err();
        assert!(format!("{err:#}").contains("not valid JSON"), "{err:#}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"projects\": {");
        std::fs::write(&file, "[1, 2]").unwrap();
        assert!(mark(TrustFormat::ClaudeJson, &file, "/w").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[1, 2]");
        // A missing file (and folder) is created.
        let fresh = dir.path().join("fresh/.claude.json");
        assert_eq!(mark(TrustFormat::ClaudeJson, &fresh, "/w").unwrap(), Marked::Now);
        assert!(is_trusted(TrustFormat::ClaudeJson, &fresh, "/w").unwrap());
    }

    #[test]
    fn codex_toml_keeps_comments_and_other_tables_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        let before = "# my codex settings\nmodel = \"gpt-6-sol\" # the default\n\n[projects.\"/other\"]\ntrust_level = \"trusted\"\n\n[mcp_servers.docs]\ncommand = \"docs-mcp\" # keep\n";
        std::fs::write(&file, before).unwrap();
        assert_eq!(mark(TrustFormat::CodexToml, &file, "/w").unwrap(), Marked::Now);
        let after = std::fs::read_to_string(&file).unwrap();
        // The new table goes next to the other projects; everything else is kept byte for byte.
        let added = "[projects.\"/w\"]\ntrust_level = \"trusted\"\n\n";
        assert!(after.contains(added), "{after}");
        assert_eq!(after.replacen(added, "", 1), before);
        let parsed: toml::Table = toml::from_str(&after).unwrap();
        assert_eq!(parsed["mcp_servers"]["docs"]["command"].as_str(), Some("docs-mcp"));
        assert_eq!(parsed["projects"]["/other"]["trust_level"].as_str(), Some("trusted"));
        assert_eq!(mark(TrustFormat::CodexToml, &file, "/w").unwrap(), Marked::Already);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), after);
        // An explicit "untrusted" is the owner's choice; broken TOML is left alone.
        std::fs::write(&file, "[projects.\"/w\"]\ntrust_level = \"untrusted\"\n").unwrap();
        assert!(matches!(mark(TrustFormat::CodexToml, &file, "/w").unwrap(), Marked::Refused(_)));
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[projects.\"/w\"]\ntrust_level = \"untrusted\"\n");
        std::fs::write(&file, "model = ").unwrap();
        assert!(mark(TrustFormat::CodexToml, &file, "/w").is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "model = ");
        // A new file holds only the project.
        let fresh = dir.path().join("new.toml");
        mark(TrustFormat::CodexToml, &fresh, "/w").unwrap();
        assert_eq!(std::fs::read_to_string(&fresh).unwrap(), "[projects.\"/w\"]\ntrust_level = \"trusted\"\n");
    }

    #[tokio::test]
    async fn files_follow_claude_config_dir_and_codex_home() {
        let (hub, _term, dir) = setup("").await;
        let spec = |p: &str| hub.config.providers[p].trust.clone().unwrap();
        assert_eq!(trust_file(&hub.config, &spec("claude")), claude_file(&dir));
        assert_eq!(trust_file(&hub.config, &spec("codex")), codex_file(&dir));
        let plain = Config::parse("[workspace]\ndir = \"/r\"\n", Path::new("/")).unwrap();
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        if std::env::var_os("CLAUDE_CONFIG_DIR").is_none() {
            assert_eq!(trust_file(&plain, &plain.providers["claude"].trust.clone().unwrap()), home.join(".claude.json"));
        }
        if std::env::var_os("CODEX_HOME").is_none() {
            assert_eq!(trust_file(&plain, &plain.providers["codex"].trust.clone().unwrap()), home.join(".codex/config.toml"));
        }
        assert!(plain.providers["gemini"].trust.is_none());
        // Overrides: another file, or off.
        let c = Config::parse("[workspace]\ndir = \"/r\"\n[providers.claude.trust]\nfile = \"/etc/c.json\"\ndir_env = \"\"\ndir_file = \"\"\n[providers.codex.trust]\nfile = \"\"\n", Path::new("/")).unwrap();
        assert_eq!(trust_file(&c, &c.providers["claude"].trust.clone().unwrap()), PathBuf::from("/etc/c.json"));
        assert!(c.providers["codex"].trust.is_none());
        assert!(Config::parse("[workspace]\ndir = \"/r\"\n[providers.aider]\nbinary = \"aider\"\ninteractive = [\"aider\"]\ntrust = { file = \"/x\" }\n", Path::new("/")).is_err());
    }

    #[tokio::test]
    async fn launch_trusts_the_folder_once_and_helpers_do_not() {
        let (hub, _term, dir) = setup("").await;
        let ws = dir.path().join("ws");
        let req = |name: &str| LaunchRequest { name: Some(name.into()), provider: "claude".into(), model: None, role: Role::Manual, task_id: None, project: None, prompt: None };
        hub.launch_command(req("login-claude"), Some(&["true".to_string()])).await.unwrap();
        assert!(!claude_file(&dir).exists(), "sign-in and update helpers do not trust folders");
        hub.launch(req("a")).await.unwrap();
        assert!(claude_trusts(&dir, &ws));
        assert!(!codex_file(&dir).exists(), "only the launched provider");
        hub.launch(req("b")).await.unwrap();
        let events = hub.store.events(100, None).await.unwrap();
        assert_eq!(events.iter().filter(|e| e.kind == "folder.trusted").count(), 1, "an event only when written");
    }

    #[tokio::test]
    async fn trust_folders_false_writes_nothing() {
        let (hub, _term, dir) = setup("trust_folders = false").await;
        let ws = dir.path().join("ws");
        assert_eq!(ensure(&hub, "claude", &ws, None).await, Outcome::Off);
        let lines = trust_workspace(&hub).await;
        assert!(lines[0].text.contains("off (workspace.trust_folders = false)"), "{lines:?}");
        hub.launch(LaunchRequest { name: None, provider: "codex".into(), model: None, role: Role::Manual, task_id: None, project: None, prompt: None }).await.unwrap();
        assert!(!claude_file(&dir).exists() && !codex_file(&dir).exists());
        assert!(!crate::project_settings::defaults(&hub).trust_folders);
    }

    #[tokio::test]
    async fn startup_trusts_the_default_workspace() {
        let (hub, _term, dir) = setup("").await;
        let ws = dir.path().join("ws");
        let lines = trust_workspace(&hub).await;
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert_eq!(lines[0].text, format!("trusted {} for claude, codex", folder_key(&ws).unwrap()));
        assert!(claude_trusts(&dir, &ws) && codex_trusts(&dir, &ws));
        let st = status(&hub, &ws);
        assert!(st.enabled);
        for p in ["claude", "codex"] {
            assert_eq!(st.providers.iter().find(|x| x.provider == p).unwrap().trusted, Some(true), "{p}");
        }
        assert_eq!(crate::project_settings::defaults(&hub).trust_providers, ["claude", "codex"]);
    }

    fn git(cwd: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap()
            .success();
        assert!(ok, "git {args:?}");
    }

    #[tokio::test]
    async fn creating_a_project_trusts_its_folder_for_every_source() {
        let (hub, _term, dir) = setup("").await;
        let new = |name: &str| NewProject { name: name.into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let settings = |kind: SourceKind, folder: &Path, remote: Option<String>| ProjectSettings {
            source: ProjectSource { kind, location: Some(folder.display().to_string()), remote, branch: None },
            ..Default::default()
        };
        let existing = dir.path().join("existing");
        std::fs::create_dir_all(&existing).unwrap();
        let fresh = dir.path().join("fresh");
        let seed = dir.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join("README.md"), "hi").unwrap();
        git(&seed, &["init", "-q", "-b", "main"]);
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-q", "-m", "one"]);
        git(dir.path(), &["clone", "-q", "--bare", "seed", "remote.git"]);
        let url = format!("file://{}", dir.path().join("remote.git").display());
        let cloned = dir.path().join("cloned");
        for (name, s, folder) in [
            ("Existing", settings(SourceKind::Local, &existing, None), &existing),
            ("Fresh", settings(SourceKind::Empty, &fresh, None), &fresh),
            ("Cloned", settings(SourceKind::Clone, &cloned, Some(url.clone())), &cloned),
        ] {
            let (_, checks) = create_with(&hub, new(name), s).await.unwrap();
            let line = format!("trusted {} for claude, codex", folder_key(folder).unwrap());
            assert!(checks.iter().any(|c| c.level == Level::Success && c.text == line), "{name}: {checks:?}");
            assert!(claude_trusts(&dir, folder) && codex_trusts(&dir, folder), "{name}");
        }
    }

    #[tokio::test]
    async fn a_failed_trust_write_does_not_block_creating_a_project() {
        let (hub, _term, dir) = setup("").await;
        std::fs::write(claude_file(&dir), "{broken").unwrap();
        let folder = dir.path().join("app");
        std::fs::create_dir_all(&folder).unwrap();
        let settings = ProjectSettings { source: ProjectSource { kind: SourceKind::Local, location: Some(folder.display().to_string()), ..Default::default() }, ..Default::default() };
        let new = NewProject { name: "App".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let (project, checks) = create_with(&hub, new, settings).await.unwrap();
        assert!(hub.store.project(&project.slug).await.unwrap().is_some());
        let key = folder_key(&folder).unwrap();
        assert!(checks.iter().any(|c| c.level == Level::Warning && c.text.starts_with(&format!("could not trust {key} for claude:")) && c.text.contains("not valid JSON")), "{checks:?}");
        assert!(checks.iter().any(|c| c.text == format!("trusted {key} for codex")), "{checks:?}");
        assert_eq!(std::fs::read_to_string(claude_file(&dir)).unwrap(), "{broken");
        let st = status(&hub, &folder);
        assert_eq!(st.providers.iter().find(|p| p.provider == "claude").unwrap().trusted, None);
        // The project page's button, once the file is fixed.
        std::fs::write(claude_file(&dir), "{}").unwrap();
        let again = trust_folder(&hub, &folder).await;
        assert_eq!(again[0].text, format!("trusted {key} for claude, codex"));
    }

    async fn worker(hub: &Arc<Hub>) -> String {
        hub.launch(LaunchRequest { name: Some("w".into()), provider: "claude".into(), model: None, role: Role::Worker, task_id: None, project: None, prompt: None }).await.unwrap().name
    }

    #[tokio::test]
    async fn answer_refuses_without_the_question_on_screen() {
        let (hub, term, _dir) = setup("trust_folders = false").await;
        let name = worker(&hub).await;
        term.set_pane(&name, "● Working on it\n✻ Thinking… (esc to interrupt)\n");
        assert!(matches!(answer(&hub, &name).await, Err(AnswerError::Conflict(_))));
        assert!(matches!(answer(&hub, "map-nope").await, Err(AnswerError::NotFound(_))));
        // A terminal attached: a person answers it there.
        term.set_pane(&name, crate::detect::CLAUDE_TRUST_PANE);
        term.sessions.lock().unwrap().get_mut(&name).unwrap().attached = 1;
        assert!(matches!(answer(&hub, &name).await, Err(AnswerError::Conflict(why)) if why.contains("attached")));
        assert!(term.sent(&name).is_empty(), "no keys sent");
    }

    #[tokio::test]
    async fn answer_records_the_folder_and_selects_yes() {
        let (hub, term, dir) = setup("trust_folders = false").await;
        let name = worker(&hub).await;
        let on_yes = crate::detect::CLAUDE_TRUST_PANE.replace(" ❯ No, exit\n   Yes,", "   No, exit\n ❯ Yes,");
        term.set_pane(&name, crate::detect::CLAUDE_TRUST_PANE);
        term.on_keys(&name, &[&on_yes, "╭────╮\n│ >  │\n╰────╯\n  ? for shortcuts\n"]);
        let a = answer(&hub, &name).await.unwrap();
        assert_eq!(a.keys, ["Down", "Enter"]);
        assert_eq!(term.sent(&name), ["<Down>", "<Enter>"]);
        assert!(a.answered);
        assert_eq!(a.folder, folder_key(&dir.path().join("ws")).unwrap());
        assert!(a.recorded.contains("trust_folders = false"), "{}", a.recorded);
        assert!(!claude_file(&dir).exists(), "the setting is off: the CLI records the answer itself");

        // With the setting on the folder is recorded too; a selection that does not move stops before Enter.
        let (hub, term, dir) = setup("").await;
        let name = worker(&hub).await;
        std::fs::remove_file(claude_file(&dir)).unwrap();
        term.set_pane(&name, crate::detect::CLAUDE_TRUST_PANE);
        let err = answer(&hub, &name).await.unwrap_err();
        assert!(matches!(&err, AnswerError::Conflict(why) if why.contains("selection")), "{err:?}");
        assert_eq!(term.sent(&name), ["<Down>"], "no Enter on the wrong option");
        assert!(claude_trusts(&dir, &dir.path().join("ws")));
    }
}
