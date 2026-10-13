//! A project's own orchestrator settings (providers, verify, audit, paused) and the helpers the orchestrator asks.
//! Tasks without a project, and projects that leave a field unset, get the `[orchestrator]` values from `map.toml`.

use std::collections::{BTreeSet, HashSet};

use anyhow::{Context, Result, bail};

use crate::hub::Hub;
use crate::model::{ProjectDefaults, ProjectSettings, Role, SourceKind, Task};

pub fn defaults(hub: &Hub) -> ProjectDefaults {
    let o = &hub.config.orchestrator;
    ProjectDefaults {
        provider_order: o.provider_order.clone(),
        verify: o.verify.clone(),
        audit_enabled: o.audit.enabled,
        audit_chain: o.audit.chain.clone(),
        max_parallel: o.max_parallel,
        post_done: o.post_done.clone(),
        session_prefix: hub.config.workspace.session_prefix.clone(),
    }
}

/// The task's project settings; `None` for the default workspace (or an unreadable project, which then runs with
/// the global settings rather than not at all).
async fn of(hub: &Hub, task: &Task) -> Option<ProjectSettings> {
    let slug = task.project.as_deref()?;
    match hub.store.project(slug).await {
        Ok(p) => p.map(|p| p.settings),
        Err(err) => {
            tracing::warn!("project {slug} settings: {err:#}");
            None
        }
    }
}

/// Slugs of paused projects, whose pending tasks the orchestrator leaves alone.
pub async fn paused(hub: &Hub) -> HashSet<String> {
    hub.store.projects().await.unwrap_or_default().into_iter().filter(|p| p.settings.paused).map(|p| p.slug).collect()
}

pub fn is_paused(paused: &HashSet<String>, task: &Task) -> bool {
    task.project.as_ref().is_some_and(|p| paused.contains(p))
}

/// Providers to try for `task`, best first: the pinned one, else the project's list, else `provider_order`;
/// without those already at the project's per-provider session limit.
pub async fn candidates(hub: &Hub, task: &Task) -> Vec<String> {
    let settings = of(hub, task).await.unwrap_or_default();
    let list: Vec<String> = match &task.provider {
        Some(p) => vec![p.clone()],
        None if !settings.providers.is_empty() => settings.providers.iter().map(|p| p.name.clone()).collect(),
        None => hub.config.orchestrator.provider_order.clone(),
    };
    let limited = settings.providers.iter().any(|p| p.max_parallel.is_some());
    if !limited {
        return list;
    }
    let sessions = hub.store.sessions().await.unwrap_or_default();
    list.into_iter()
        .filter(|name| {
            let Some(limit) = settings.providers.iter().find(|p| &p.name == name).and_then(|p| p.max_parallel) else { return true };
            let live = sessions.iter().filter(|s| s.role == Role::Worker && s.state.is_live() && s.project == task.project && &s.provider == name).count();
            live < limit as usize
        })
        .collect()
}

/// The project's model for `provider`, used when the task does not name one.
pub async fn model(hub: &Hub, task: &Task, provider: &str) -> Option<String> {
    of(hub, task).await?.providers.into_iter().find(|p| p.name == provider)?.model
}

pub async fn verify(hub: &Hub, task: &Task) -> Vec<String> {
    of(hub, task).await.and_then(|s| s.verify).unwrap_or_else(|| hub.config.orchestrator.verify.clone())
}

pub async fn audit_enabled(hub: &Hub, task: &Task) -> bool {
    of(hub, task).await.and_then(|s| s.audit).map_or(hub.config.orchestrator.audit.enabled, |a| a.enabled)
}

pub async fn audit_chain(hub: &Hub, task: &Task) -> Vec<String> {
    match of(hub, task).await.and_then(|s| s.audit).and_then(|a| a.provider) {
        Some(p) => vec![p],
        None => hub.config.orchestrator.audit.chain.clone(),
    }
}

/// A folder on this machine as an absolute path; for a legacy `git` project, a git URL that is never an option or a
/// transport that runs commands.
pub fn validate_location(kind: SourceKind, location: Option<&str>) -> Result<()> {
    match kind {
        SourceKind::Git => {
            let url = location.map(str::trim).filter(|l| !l.is_empty()).context("a git source needs a URL")?;
            let scp = regex::Regex::new(r"^[A-Za-z0-9._-]+@[A-Za-z0-9.-]+:[^\s]+$").unwrap();
            let ok = ["https://", "http://", "ssh://", "git://", "file://"].iter().any(|p| url.starts_with(p)) || scp.is_match(url);
            if !ok || url.chars().any(char::is_whitespace) {
                bail!("use an https://, ssh://, git:// or file:// URL, or user@host:path");
            }
            Ok(())
        }
        SourceKind::Empty | SourceKind::Local => {
            let path = location.map(str::trim).filter(|l| !l.is_empty()).context("a project needs its folder on this machine")?;
            if !(path.starts_with('/') || path.starts_with("~/")) {
                bail!("give the folder as an absolute path (or ~/…)");
            }
            Ok(())
        }
    }
}

pub fn validate_branch(branch: Option<&str>) -> Result<()> {
    let Some(b) = branch else { return Ok(()) };
    let ok = !b.is_empty()
        && b.len() <= 200
        && !b.starts_with('-')
        && !b.contains("..")
        && b.chars().all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c));
    if !ok {
        bail!("invalid branch `{b}`");
    }
    Ok(())
}

pub fn validate(hub: &Hub, s: &ProjectSettings) -> Result<()> {
    validate_location(s.source.kind, s.source.location.as_deref())?;
    validate_branch(s.source.branch.as_deref())?;
    let mut seen = BTreeSet::new();
    for p in &s.providers {
        let spec = hub.spec(&p.name)?;
        if !seen.insert(p.name.as_str()) {
            bail!("provider `{}` is listed twice", p.name);
        }
        if let Some(m) = &p.model
            && !spec.models.is_empty()
            && !spec.models.contains(m)
        {
            bail!("model `{m}` is not in {}'s list ({})", p.name, spec.models.join(", "));
        }
        if p.max_parallel == Some(0) {
            bail!("max parallel sessions for {} must be at least 1", p.name);
        }
    }
    if let Some(v) = &s.verify
        && v.iter().any(|c| c.trim().is_empty())
    {
        bail!("verify commands cannot be empty");
    }
    if let Some(p) = s.audit.as_ref().and_then(|a| a.provider.as_deref())
        && hub.spec(p)?.headless.is_empty()
    {
        bail!("provider `{p}` has no headless command, so it cannot audit");
    }
    Ok(())
}

/// Empty strings from a form mean "not set".
pub fn normalize(mut s: ProjectSettings) -> ProjectSettings {
    let blank = |v: Option<String>| v.map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    s.source.location = blank(s.source.location);
    s.source.branch = blank(s.source.branch);
    for p in &mut s.providers {
        p.model = blank(p.model.take());
    }
    if let Some(v) = &mut s.verify {
        *v = v.iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
    }
    if let Some(a) = &mut s.audit {
        a.provider = blank(a.provider.take());
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    use crate::config::Config;
    use crate::model::{ProjectAudit, ProjectProvider, ProjectSource, TaskStatus};
    use crate::orchestrator::Orchestrator;
    use crate::projects::{NewProject, create_with};
    use crate::store::{NewTask, Store};
    use crate::terminal::fake::FakeTerminal;

    /// `claude` and `codex` both count as installed (`/bin/sh`); no Reflex binary.
    async fn hub(extra: &str) -> (Arc<Hub>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[orchestrator]\nprovider_order = [\"claude\", \"codex\"]\nmax_parallel = 4\nverify = [\"global-check\"]\n{extra}\n[reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"/bin/sh\"\ncredentials = []\n[providers.codex]\nbinary = \"/bin/sh\"\ncredentials = []\n",
            dir.path().display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        (Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default())), dir)
    }

    fn task(title: &str, project: Option<&str>) -> NewTask {
        NewTask { title: title.into(), prompt: "p".into(), depends_on: vec![], provider: None, model: None, project: project.map(String::from) }
    }

    /// A project in a new folder next to the data dir, unless `settings` names one.
    async fn project(hub: &Hub, name: &str, mut settings: ProjectSettings) -> String {
        if settings.source.location.is_none() {
            settings.source.location = Some(hub.config.server.data_dir.with_file_name("folders").join(name).display().to_string());
        }
        let new = NewProject { name: name.into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        create_with(hub, new, settings).await.unwrap().0.slug
    }

    #[tokio::test]
    async fn orchestrator_honours_project_providers_limits_models_and_pause() {
        let (hub, _dir) = hub("").await;
        let settings = ProjectSettings {
            providers: vec![ProjectProvider { name: "codex".into(), model: Some("gpt-6-luna".into()), max_parallel: Some(1) }],
            verify: Some(vec!["cargo test".into()]),
            audit: Some(ProjectAudit { enabled: false, provider: None }),
            ..Default::default()
        };
        let slug = project(&hub, "Api", settings).await;
        let paused = project(&hub, "Quiet", ProjectSettings { paused: true, ..Default::default() }).await;
        let a = hub.store.create_task(task("a", Some(&slug))).await.unwrap();
        let b = hub.store.create_task(task("b", Some(&slug))).await.unwrap();
        let c = hub.store.create_task(task("c", Some(&paused))).await.unwrap();
        let d = hub.store.create_task(task("d", None)).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();

        let a = hub.store.task(a.id).await.unwrap().unwrap();
        assert_eq!(a.status, TaskStatus::Running);
        let session = hub.store.session(a.session.as_deref().unwrap()).await.unwrap().unwrap();
        assert_eq!((session.provider.as_str(), session.model.as_str()), ("codex", "gpt-6-luna"));
        let prompt = std::fs::read_to_string(hub.run_dir(&session.name).join("prompt.md")).unwrap();
        assert!(prompt.contains("cargo test") && !prompt.contains("global-check"), "{prompt}");

        let b = hub.store.task(b.id).await.unwrap().unwrap();
        assert_eq!(b.status, TaskStatus::Pending, "codex is at the project's limit of 1");
        assert!(b.result.unwrap().starts_with("Waiting"));
        assert_eq!(hub.store.task(c.id).await.unwrap().unwrap().status, TaskStatus::Pending, "paused project");
        let d = hub.store.task(d.id).await.unwrap().unwrap();
        assert_eq!(hub.store.session(d.session.as_deref().unwrap()).await.unwrap().unwrap().provider, "claude", "the default workspace keeps provider_order");
    }

    #[tokio::test]
    async fn uses_a_folder_in_place_and_reports_checks() {
        let (hub, dir) = hub("").await;
        let folder = dir.path().join("my-app");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("Cargo.toml"), "[package]").unwrap();
        let local = |path: &Path| ProjectSettings { source: ProjectSource { kind: SourceKind::Local, location: Some(path.display().to_string()), branch: None }, ..Default::default() };
        let new = |name: &str| NewProject { name: name.into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let (p, checks) = create_with(&hub, new("Mine"), local(&folder)).await.unwrap();
        assert_eq!(hub.workspace_of(&p), folder.canonicalize().unwrap());
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 1, "nothing added to the folder");
        assert!(!hub.project_dir(&p.slug).join("code.git").exists(), "the code is never copied");
        let text: Vec<&str> = checks.iter().map(|c| c.text.as_str()).collect();
        assert!(text[0].starts_with("using ") && text[0].ends_with("not a git repo"), "{text:?}");
        assert!(text.iter().any(|t| t.starts_with("claude · sonnet · found")), "{text:?}");

        // Another project cannot take the same folder or one inside it; missing folders and git URLs are refused.
        assert!(create_with(&hub, new("Again"), local(&folder)).await.is_err());
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        assert!(create_with(&hub, new("Inside"), local(&folder.join("sub"))).await.is_err());
        assert!(create_with(&hub, new("Broken"), local(Path::new("/nonexistent/repo"))).await.is_err());
        assert!(!hub.project_dir("broken").exists(), "a failed create leaves nothing behind");
        let git = ProjectSettings { source: ProjectSource { kind: SourceKind::Git, location: Some("https://github.com/a/b.git".into()), branch: None }, ..Default::default() };
        assert!(create_with(&hub, new("Cloned"), git).await.is_err());

        // A new folder is created, and deleting the project leaves the folder and its files alone.
        let fresh = dir.path().join("fresh");
        let empty = ProjectSettings { source: ProjectSource { kind: SourceKind::Empty, location: Some(fresh.display().to_string()), branch: None }, ..Default::default() };
        let (q, _) = create_with(&hub, new("Fresh"), empty).await.unwrap();
        assert!(fresh.is_dir());
        std::fs::write(fresh.join("work.txt"), "x").unwrap();
        crate::projects::delete(&hub, &q.slug).await.unwrap();
        assert!(fresh.join("work.txt").exists());
        crate::projects::delete(&hub, &p.slug).await.unwrap();
        assert!(folder.join("Cargo.toml").exists());
    }

    #[tokio::test]
    async fn rejects_bad_settings() {
        let (hub, _dir) = hub("").await;
        let twice = ProjectSettings { providers: vec![ProjectProvider { name: "claude".into(), model: None, max_parallel: None }; 2], ..Default::default() };
        assert!(validate(&hub, &twice).is_err());
        let model = ProjectSettings { providers: vec![ProjectProvider { name: "claude".into(), model: Some("gpt".into()), max_parallel: None }], ..Default::default() };
        assert!(validate(&hub, &model).is_err());
        let unknown = ProjectSettings { providers: vec![ProjectProvider { name: "nope".into(), model: None, max_parallel: None }], ..Default::default() };
        assert!(validate(&hub, &unknown).is_err());
    }

    #[test]
    fn source_rules() {
        assert!(validate_location(SourceKind::Git, Some("https://github.com/a/b.git")).is_ok());
        assert!(validate_location(SourceKind::Git, Some("git@github.com:a/b.git")).is_ok());
        assert!(validate_location(SourceKind::Git, Some("--upload-pack=touch /tmp/x")).is_err());
        assert!(validate_location(SourceKind::Git, Some("ext::sh -c touch% /tmp/x")).is_err());
        assert!(validate_location(SourceKind::Local, Some("relative/path")).is_err());
        assert!(validate_location(SourceKind::Local, Some("~/code/x")).is_ok());
        assert!(validate_location(SourceKind::Empty, None).is_err());
        assert!(validate_branch(Some("feat/i18n-th-en")).is_ok());
        assert!(validate_branch(Some("-x")).is_err());
        assert!(validate_branch(Some("a..b")).is_err());
    }
}
