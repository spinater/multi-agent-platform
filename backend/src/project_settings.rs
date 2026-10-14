//! A project's own orchestrator settings (providers, verify, audit, paused) and the helpers the orchestrator asks.
//! Tasks without a project, and projects that leave a field unset, get the `[orchestrator]` values from `map.toml`.

use std::collections::{BTreeSet, HashSet};

use anyhow::{Context, Result, bail};

use crate::hub::Hub;
use crate::model::{ProjectDefaults, ProjectSettings, ProviderTiers, ReflexGates, Role, SourceKind, Task};

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
        tiers: o
            .tiers
            .iter()
            .filter(|(_, t)| !t.is_empty())
            .map(|(p, t)| ProviderTiers { provider: p.clone(), light: t.light.clone(), standard: t.standard.clone(), heavy: t.heavy.clone() })
            .collect(),
        trust_folders: hub.config.workspace.trust_folders,
        trust_providers: crate::trust::pretrust_providers(hub),
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

/// The project's Reflex gates; all off for the default workspace.
pub async fn gates(hub: &Hub, task: &Task) -> ReflexGates {
    of(hub, task).await.map(|s| s.reflex_gates).unwrap_or_default()
}

pub async fn audit_chain(hub: &Hub, task: &Task) -> Vec<String> {
    match of(hub, task).await.and_then(|s| s.audit).and_then(|a| a.provider) {
        Some(p) => vec![p],
        None => hub.config.orchestrator.audit.chain.clone(),
    }
}

/// A folder on this machine as an absolute path; for a legacy `git` project, a git URL that is never an option or a
/// transport that runs commands (`git_access::validate_url`).
pub fn validate_location(kind: SourceKind, location: Option<&str>) -> Result<()> {
    match kind {
        SourceKind::Git => crate::git_access::validate_url(location.unwrap_or_default()),
        SourceKind::Empty | SourceKind::Local | SourceKind::Clone => {
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
    match s.source.remote.as_deref() {
        Some(url) => crate::git_access::validate_url(url)?,
        None if s.source.kind == SourceKind::Clone => bail!("a clone needs the repo's URL"),
        None => {}
    }
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
    validate_branch(s.branches.base.as_deref())?;
    if s.parallel.max == Some(0) {
        bail!("a project's parallel limit must be at least 1 (empty: no project limit)");
    }
    if let Some(p) = s.parallel.analyst_provider.as_deref()
        && hub.spec(p)?.headless.is_empty()
    {
        bail!("provider `{p}` has no headless command, so it cannot be the parallel analyst");
    }
    for c in &s.branches.copy {
        crate::worktrees::validate_copy(c)?;
    }
    if s.branches.mode == crate::model::BranchMode::Worktree && s.source.kind == SourceKind::Git {
        bail!("a legacy git project cannot use worktrees; move it to a folder first");
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
    s.source.remote = blank(s.source.remote);
    for p in &mut s.providers {
        p.model = blank(p.model.take());
    }
    if let Some(v) = &mut s.verify {
        *v = v.iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
    }
    if let Some(a) = &mut s.audit {
        a.provider = blank(a.provider.take());
    }
    s.branches.base = blank(s.branches.base.take());
    s.parallel.analyst_provider = blank(s.parallel.analyst_provider.take());
    for list in [&mut s.branches.setup, &mut s.branches.copy] {
        *list = list.iter().map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
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
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[orchestrator]\nprovider_order = [\"claude\", \"codex\"]\nmax_parallel = 4\nverify = [\"global-check\"]\n{extra}\n[reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"/bin/sh\"\ncredentials = []\n[providers.codex]\nbinary = \"/bin/sh\"\ncredentials = []\n",
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
        // Cleared to share the folder, so only the provider limit holds b back.
        let scope = |id, path: &str| crate::parallel::SubmittedScope { id, paths: vec![path.into()], independent: true, reason: String::new() };
        crate::parallel::save_scopes(&hub, Some(&slug), vec![scope(a.id, "src/a/"), scope(b.id, "src/b/")], "test").await.unwrap();
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
        let local = |path: &Path| ProjectSettings { source: ProjectSource { kind: SourceKind::Local, location: Some(path.display().to_string()), ..Default::default() }, ..Default::default() };
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
        let git = ProjectSettings { source: ProjectSource { kind: SourceKind::Git, location: Some("https://github.com/a/b.git".into()), ..Default::default() }, ..Default::default() };
        assert!(create_with(&hub, new("Cloned"), git).await.is_err());

        // A new folder is created, and deleting the project leaves the folder and its files alone.
        let fresh = dir.path().join("fresh");
        let empty = ProjectSettings { source: ProjectSource { kind: SourceKind::Empty, location: Some(fresh.display().to_string()), ..Default::default() }, ..Default::default() };
        let (q, _) = create_with(&hub, new("Fresh"), empty).await.unwrap();
        assert!(fresh.is_dir());
        std::fs::write(fresh.join("work.txt"), "x").unwrap();
        crate::projects::delete(&hub, &q.slug).await.unwrap();
        assert!(fresh.join("work.txt").exists());
        crate::projects::delete(&hub, &p.slug).await.unwrap();
        assert!(folder.join("Cargo.toml").exists());
    }

    #[tokio::test]
    async fn clones_a_repo_into_a_folder_and_agents_get_maps_gitconfig() {
        let (hub, dir) = hub("").await;
        let git = |cwd: &Path, args: &[&str]| {
            let ok = std::process::Command::new("git").current_dir(cwd).args(args).env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").status().unwrap().success();
            assert!(ok, "git {args:?}");
        };
        let seed = dir.path().join("seed");
        std::fs::create_dir_all(&seed).unwrap();
        std::fs::write(seed.join("README.md"), "hi").unwrap();
        git(&seed, &["init", "-q", "-b", "main"]);
        git(&seed, &["add", "."]);
        git(&seed, &["commit", "-q", "-m", "one"]);
        git(dir.path(), &["clone", "-q", "--bare", "seed", "remote.git"]);
        let url = format!("file://{}", dir.path().join("remote.git").display());

        let new = |name: &str| NewProject { name: name.into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let clone = |folder: &Path, remote: Option<&str>| ProjectSettings {
            source: ProjectSource { kind: SourceKind::Clone, location: Some(folder.display().to_string()), remote: remote.map(String::from), branch: None },
            ..Default::default()
        };
        let folder = dir.path().join("code/app");
        let (p, checks) = create_with(&hub, new("App"), clone(&folder, Some(&url))).await.unwrap();
        assert_eq!((p.settings.source.kind, p.settings.source.remote.as_deref()), (SourceKind::Local, Some(url.as_str())), "stored as a folder project");
        assert!(folder.join("README.md").is_file() && folder.join(".git").is_dir());
        assert!(checks[0].text.starts_with(&format!("cloned {url} into")) && checks[0].text.contains("branch main · 1 commit"), "{:?}", checks[0]);

        // The folder must be missing or empty; the URL must be safe; a failed clone leaves nothing behind.
        assert!(create_with(&hub, new("Again"), clone(&dir.path().join("seed"), Some(&url))).await.is_err());
        assert!(create_with(&hub, new("Opt"), clone(&dir.path().join("x"), Some("--upload-pack=touch /tmp/x"))).await.is_err());
        assert!(create_with(&hub, new("None"), clone(&dir.path().join("y"), None)).await.is_err());
        let gone = dir.path().join("gone");
        assert!(create_with(&hub, new("Gone"), clone(&gone, Some("file:///nonexistent/r.git"))).await.is_err());
        assert!(!gone.exists() && !hub.project_dir("gone").exists());

        // Agents in the project get MAP's gitconfig; so do MAP's own git commands.
        let session = hub
            .launch(crate::hub::LaunchRequest { name: None, provider: "claude".into(), model: None, role: Role::Manual, task_id: None, project: Some(p.slug.clone()), prompt: None })
            .await
            .unwrap();
        let script = std::fs::read_to_string(hub.run_dir(&session.name).join("launch.sh")).unwrap();
        let config = hub.config.server.data_dir.join("git/gitconfig");
        assert!(script.contains(&format!("export GIT_CONFIG_GLOBAL={}\n", config.display())), "{script}");
        assert!(config.is_file());
        let status = crate::project_git::status(&hub, &p.slug).await.unwrap();
        assert_eq!((status.is_repo, status.upstream.as_deref(), status.busy), (true, Some("origin/main"), None));

        // While one of its tasks runs, only fetch is allowed.
        let mut t = hub.store.create_task(task("work", Some(&p.slug))).await.unwrap();
        t.status = TaskStatus::Running;
        hub.put_task(&mut t).await.unwrap();
        assert!(crate::project_git::status(&hub, &p.slug).await.unwrap().busy.is_some_and(|b| b.contains(&format!("#{}", t.id))));
        assert!(matches!(crate::project_git::run_op(&hub, &p.slug, crate::project_git::Op::Pull, None).await, Err(crate::project_git::OpError::Refused(_))));
        assert!(crate::project_git::run_op(&hub, &p.slug, crate::project_git::Op::Fetch, None).await.unwrap().ok);
    }

    #[tokio::test]
    async fn every_source_must_be_inside_the_project_folder() {
        let (hub, dir) = hub("").await;
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(outside.path().join("app")).unwrap();
        let new = |name: &str| NewProject { name: name.into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let src = |kind, path: &Path, remote: Option<&str>| ProjectSettings {
            source: ProjectSource { kind, location: Some(path.display().to_string()), remote: remote.map(String::from), branch: None },
            ..Default::default()
        };
        let url = "file:///nonexistent/r.git";
        for (kind, path, remote) in [
            (SourceKind::Local, outside.path().join("app"), None),
            (SourceKind::Empty, outside.path().join("fresh"), None),
            (SourceKind::Clone, outside.path().join("clone"), Some(url)),
            // The project folder itself, `..`, and a symlink out of it.
            (SourceKind::Local, dir.path().to_path_buf(), None),
            (SourceKind::Empty, dir.path().join("a/../../x"), None),
        ] {
            let err = create_with(&hub, new("Out"), src(kind, &path, remote)).await.unwrap_err().to_string();
            assert!(err.contains("outside the project folder") || err.contains("project folder itself") || err.contains("`..`"), "{kind:?} {}: {err}", path.display());
        }
        std::os::unix::fs::symlink(outside.path(), dir.path().join("escape")).unwrap();
        for (kind, path) in [(SourceKind::Local, dir.path().join("escape/app")), (SourceKind::Empty, dir.path().join("escape/new")), (SourceKind::Clone, dir.path().join("escape/c"))] {
            let err = create_with(&hub, new("Esc"), src(kind, &path, Some(url))).await.unwrap_err().to_string();
            assert!(err.contains("outside the project folder"), "{err}");
        }
        assert!(!outside.path().join("fresh").exists() && !outside.path().join("escape").exists());
        assert!(hub.store.projects().await.unwrap().is_empty());

        // A new folder under sub-folders that do not exist yet is fine, and stored as its real path.
        let (p, _) = create_with(&hub, new("Deep"), src(SourceKind::Empty, &dir.path().join("group/team/deep"), None)).await.unwrap();
        assert!(dir.path().join("group/team/deep").is_dir());
        let info = hub.project_info(p).await.unwrap();
        assert!(!info.outside_roots && info.root.is_some());

        // A project made before the project folder was set keeps loading, with the warning.
        let mut old = hub.store.project(&info.project.slug).await.unwrap().unwrap();
        old.slug = "legacy".into();
        old.port += 1;
        old.settings.source.location = Some(outside.path().join("app").display().to_string());
        hub.put_project(&old).await.unwrap();
        let legacy = hub.projects().await.unwrap().into_iter().find(|i| i.project.slug == "legacy").unwrap();
        assert!(legacy.outside_roots && legacy.root.is_none());
        assert_eq!(legacy.workspace, outside.path().join("app").display().to_string());
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
