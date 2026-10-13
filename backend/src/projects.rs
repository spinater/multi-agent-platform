//! Creating, changing and removing projects. A project is two bare repos (code and corpus), the workspace agents
//! use, and a Reflex engine; this module keeps the four in step.

use std::collections::BTreeSet;

use anyhow::{Context, Result, bail};
use chrono::Utc;

use crate::hub::Hub;
use crate::model::{CheckLine, Level, Project, ProjectSettings, Question, QuestionKind, ReflexState, SourceKind};
use crate::{project_settings, reflex, repo};

pub struct NewProject {
    pub name: String,
    pub slug: Option<String>,
    pub description: String,
    pub fallback: Option<String>,
    pub model: Option<String>,
}

pub fn slugify(name: &str) -> String {
    let mut out = String::new();
    for c in name.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    out.trim_end_matches('-').chars().take(40).collect::<String>().trim_end_matches('-').to_string()
}

pub fn validate_slug(slug: &str) -> Result<()> {
    let ok = (2..=40).contains(&slug.len())
        && slug.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if !ok {
        bail!("a project slug is 2–40 characters of a-z, 0-9 and - (not at either end)");
    }
    Ok(())
}

/// Reflex's own request rules, so a question that only reaches the fallback is held to the same contract.
pub fn validate_questions(questions: &[Question]) -> Result<()> {
    let mut ids = BTreeSet::new();
    for q in questions {
        if q.id.trim().is_empty() || !ids.insert(q.id.as_str()) {
            bail!("question id `{}` is empty or used twice", q.id);
        }
        if q.prompt.trim().is_empty() {
            bail!("question `{}` has an empty prompt", q.id);
        }
        let options = q.options.as_deref().unwrap_or_default();
        match q.kind {
            QuestionKind::Choice | QuestionKind::Score if options.len() < 2 => bail!("question `{}` needs at least 2 options", q.id),
            QuestionKind::Choice | QuestionKind::Score if options.iter().any(|o| o.trim().is_empty()) => bail!("question `{}` has an empty option", q.id),
            QuestionKind::Noul if q.options.is_some() => bail!("yes/no question `{}` takes no options", q.id),
            _ => {}
        }
    }
    Ok(())
}

/// `none`, `api`, or a provider that can run headless.
pub fn validate_fallback(hub: &Hub, fallback: &str) -> Result<()> {
    match fallback {
        "none" | "api" => Ok(()),
        name => {
            let spec = hub.spec(name).with_context(|| format!("fallback must be none, api or a provider; `{name}` is neither"))?;
            if spec.headless.is_empty() {
                bail!("provider `{name}` has no headless command, so it cannot answer decisions");
            }
            Ok(())
        }
    }
}

/// Any repo path: relative, no `..`, no `.git` segments.
pub fn validate_repo_path(path: &str) -> Result<()> {
    let ok = !path.is_empty()
        && path.len() <= 200
        && path.split('/').all(|seg| {
            !seg.is_empty() && seg != "." && seg != ".." && !seg.starts_with(".git") && seg.chars().all(|c| c.is_alphanumeric() || "._- ".contains(c))
        });
    if !ok {
        bail!("invalid path `{path}`");
    }
    Ok(())
}

/// Corpus files are `<domain>.md` or `<domain>/<file>.md`, the layout Reflex loads.
pub fn validate_corpus_path(path: &str) -> Result<()> {
    validate_repo_path(path)?;
    let name = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let md = |f: &str| f.strip_suffix(".md").is_some_and(name);
    let ok = match path.split('/').collect::<Vec<_>>().as_slice() {
        [file] => md(file),
        [domain, file] => name(domain) && md(file),
        _ => false,
    };
    if !ok {
        bail!("corpus files are <domain>.md or <domain>/<file>.md, named with a-z, 0-9, _ and -");
    }
    Ok(())
}

fn domain_of(path: &str) -> &str {
    path.split('/').next().unwrap_or("").trim_end_matches(".md")
}

fn corpus_seed() -> Vec<(String, Option<String>)> {
    vec![(
        "example/example.md".into(),
        Some(
            "Replace this folder with your own domains. Each top-level folder is one Reflex domain (1 to 8), and each .md \
             file in it is prose describing situations that belong to that domain and how they are handled, written like a runbook.\n"
                .into(),
        ),
    )]
}

fn code_seed(p: &Project) -> Vec<(String, Option<String>)> {
    let readme = format!(
        "# {name}\n\n{desc}\n\nCreated by multi-agent-platform. Its decision engine is the project's Reflex, fed by the sibling corpus repo \
         (`/git/{slug}/corpus.git` on the platform).\n",
        name = p.name,
        desc = p.description,
        slug = p.slug,
    );
    let claude = "# Decisions\n\n\
        Before spending an LLM call to classify, route, triage or decide something, ask this project's decision endpoint. \
        Agents started by the platform have its URL in `$MAP_DECIDE_URL`.\n\n\
        ```sh\ncurl -s \"$MAP_DECIDE_URL\" -H 'content-type: application/json' -d '{\n  \"state\": \"the facts of the situation\",\n  \
        \"questions\": [{\"id\": \"route\", \"kind\": \"choice\", \"prompt\": \"Route this ticket\", \"options\": [\"billing\", \"shipping\"]}]\n}'\n```\n\n\
        - Kinds: `choice` (2+ options), `score` (2+ levels, lowest first), `noul` (yes/no, no options).\n\
        - Put the facts in `state`: a thin state makes Reflex abstain.\n\
        - Each item in `answers` has `answer` and `lane`: `reflex` (no LLM used), `llm` (the fallback answered) or `abstain` (nobody did: handle it).\n\
        - Corpus changes go to the corpus repo; a push to `main` reloads Reflex.\n";
    vec![("README.md".into(), Some(readme)), ("CLAUDE.md".into(), Some(claude.into()))]
}

#[cfg(test)]
pub async fn create(hub: &Hub, req: NewProject) -> Result<Project> {
    create_with(hub, req, ProjectSettings::default()).await.map(|(p, _)| p)
}

fn check(checks: &mut Vec<CheckLine>, level: Level, text: impl Into<String>) {
    checks.push(CheckLine { level, text: text.into() });
}

/// Creates the project's code repo (new, or imported from `settings.source`), its corpus repo, the agents'
/// workspace and its Reflex, and reports each step as a check line.
pub async fn create_with(hub: &Hub, req: NewProject, settings: ProjectSettings) -> Result<(Project, Vec<CheckLine>)> {
    let mut settings = project_settings::normalize(settings);
    project_settings::validate(hub, &settings)?;
    let name = req.name.trim().to_string();
    if name.is_empty() {
        bail!("a project needs a name");
    }
    let slug = req.slug.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| slugify(&name));
    validate_slug(&slug)?;
    let fallback = req.fallback.filter(|f| !f.is_empty()).unwrap_or_else(|| "api".into());
    validate_fallback(hub, &fallback)?;
    if hub.store.project(&slug).await?.is_some() || hub.project_dir(&slug).exists() {
        bail!("project `{slug}` already exists");
    }

    let project = Project {
        slug: slug.clone(),
        name,
        description: req.description.trim().to_string(),
        port: hub.store.free_port(hub.config.reflex.port_base).await?,
        threshold: None,
        fallback,
        model: req.model.filter(|m| !m.is_empty()),
        questions: vec![],
        created_at: Utc::now(),
        settings: ProjectSettings::default(),
    };
    let mut checks = vec![];
    let setup = async {
        let code = hub.code_repo(&slug);
        let corpus = hub.corpus_repo(&slug);
        match settings.source.kind {
            SourceKind::Empty => {
                repo::init_bare(&code).await?;
                repo::commit(&code, &code_seed(&project), "Initial commit").await?;
                check(&mut checks, Level::Success, format!("code repo created · /git/{slug}/code.git · README.md, CLAUDE.md"));
            }
            SourceKind::Git | SourceKind::Local => {
                let location = settings.source.location.clone().unwrap_or_default();
                let source = if settings.source.kind == SourceKind::Local {
                    let path = crate::login::expand_home(&location);
                    if !path.is_dir() {
                        bail!("{} is not a directory", path.display());
                    }
                    path.display().to_string()
                } else {
                    location.clone()
                };
                std::fs::create_dir_all(hub.project_dir(&slug))?;
                let (branch, commits) = repo::clone_bare(&source, settings.source.branch.as_deref(), &code).await?;
                check(&mut checks, Level::Success, format!("imported {commits} commit{} from {location} @ {branch} as main · /git/{slug}/code.git", if commits == 1 { "" } else { "s" }));
                settings.source.branch = Some(branch);
                if repo::read(&code, "CLAUDE.md").await.is_none() {
                    let claude = code_seed(&project).into_iter().filter(|(p, _)| p == "CLAUDE.md").collect::<Vec<_>>();
                    repo::commit(&code, &claude, "Add the platform's decision notes").await?;
                    check(&mut checks, Level::Success, "added CLAUDE.md with the project's decision endpoint");
                } else {
                    check(&mut checks, Level::Warning, "kept the repo's own CLAUDE.md; agents still get $MAP_DECIDE_URL in their prompt");
                }
            }
        }
        repo::init_bare(&corpus).await?;
        repo::commit(&corpus, &corpus_seed(), "Initial corpus").await?;
        check(&mut checks, Level::Success, format!("corpus repo created · /git/{slug}/corpus.git · 1 example domain"));
        let workspace = hub.project_workspace(&slug);
        repo::sync_workspace(&code, &workspace).await?;
        check(&mut checks, Level::Success, format!("workspace checked out at {}", workspace.display()));
        hub.put_project(&Project { settings: settings.clone(), ..project.clone() }).await
    };
    if let Err(err) = setup.await {
        let _ = std::fs::remove_dir_all(hub.project_dir(&slug));
        let _ = hub.store.delete_project(&slug).await;
        return Err(err);
    }
    let project = Project { settings, ..project };
    hub.event(Level::Info, "project.created", None, None, format!("Project {slug} created")).await;

    match hub.restart_reflex(&slug).await {
        Ok(s) => {
            let first = s.message.lines().next().unwrap_or("").to_string();
            match s.state {
                ReflexState::Running => check(&mut checks, Level::Success, format!("Reflex running on 127.0.0.1:{} · {} domain(s)", project.port, s.domains.len())),
                ReflexState::NoCorpus => check(&mut checks, Level::Warning, "Reflex not started: the corpus has no domains yet"),
                state => check(&mut checks, Level::Warning, format!("Reflex {}: {first}", format!("{state:?}").to_lowercase())),
            }
        }
        Err(err) => check(&mut checks, Level::Warning, format!("Reflex not started: {err:#}")),
    }

    let providers = hub.providers();
    let order = if project.settings.providers.is_empty() { hub.config.orchestrator.provider_order.clone() } else { project.settings.providers.iter().map(|p| p.name.clone()).collect() };
    for name in order {
        let Some(p) = providers.iter().find(|p| p.name == name) else { continue };
        let model = project.settings.providers.iter().find(|x| x.name == name).and_then(|x| x.model.clone()).unwrap_or(p.default_model.clone());
        let (level, state) = match (p.available, p.signed_in) {
            (false, _) => (Level::Error, "not found on PATH"),
            (true, Some(true)) => (Level::Success, "found · signed in"),
            (true, Some(false)) => (Level::Warning, "found · signed out"),
            (true, None) => (Level::Info, "found · sign-in unknown"),
        };
        check(&mut checks, level, format!("{name} · {model} · {state}"));
    }
    let verify = project.settings.verify.clone().unwrap_or_else(|| hub.config.orchestrator.verify.clone());
    let audit = project.settings.audit.as_ref().map_or(hub.config.orchestrator.audit.enabled, |a| a.enabled);
    check(&mut checks, Level::Info, format!("verify: {} command(s) · audit {}{}", verify.len(), if audit { "on" } else { "off" }, if project.settings.paused { " · paused" } else { "" }));
    Ok((project, checks))
}

/// Commits one file to a project repo (`None` deletes it); a corpus change restarts Reflex.
pub async fn write_file(hub: &Hub, slug: &str, repo_name: &str, path: &str, content: Option<String>, message: &str) -> Result<String> {
    let repo_path = match repo_name {
        "code" => {
            validate_repo_path(path)?;
            hub.code_repo(slug)
        }
        "corpus" => {
            validate_corpus_path(path)?;
            let repo_path = hub.corpus_repo(slug);
            if content.is_some() {
                let mut domains: BTreeSet<String> = repo::files(&repo_path)
                    .await?
                    .iter()
                    .filter(|f| validate_corpus_path(&f.path).is_ok() && domain_of(&f.path) != repo::ARCHIVE_DIR)
                    .map(|f| domain_of(&f.path).to_string())
                    .collect();
                domains.insert(domain_of(path).to_string());
                if domains.len() > reflex::MAX_DOMAINS {
                    bail!("Reflex takes at most {} domains per corpus", reflex::MAX_DOMAINS);
                }
            }
            repo_path
        }
        other => bail!("unknown repo `{other}`; use code or corpus"),
    };
    let message = if message.trim().is_empty() { format!("{} {path}", if content.is_some() { "Update" } else { "Delete" }) } else { message.trim().to_string() };
    let commit = repo::commit(&repo_path, &[(path.to_string(), content)], &message).await?;
    if repo_name == "corpus" {
        hub.restart_reflex(slug).await?;
    }
    Ok(commit)
}

pub async fn delete(hub: &Hub, slug: &str) -> Result<()> {
    hub.store.project(slug).await?.with_context(|| format!("unknown project `{slug}`"))?;
    let live: Vec<String> = hub
        .store
        .tasks()
        .await?
        .into_iter()
        .filter(|t| t.project.as_deref() == Some(slug) && !t.status.is_terminal() && t.status != crate::model::TaskStatus::Pending)
        .map(|t| format!("#{}", t.id))
        .collect();
    if !live.is_empty() {
        bail!("tasks {} are running in this project; cancel them first", live.join(", "));
    }
    hub.reflex.forget(slug).await;
    hub.store.delete_project(slug).await?;
    std::fs::remove_dir_all(hub.project_dir(slug)).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })?;
    hub.announce_project_removed(slug);
    hub.event(Level::Warning, "project.deleted", None, None, format!("Project {slug} deleted")).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("Customer Support!"), "customer-support");
        assert_eq!(slugify("  ระบบ ABC "), "abc");
        assert!(validate_slug("ab").is_ok());
        assert!(validate_slug("-ab").is_err());
        assert!(validate_slug("a").is_err());
        assert!(validate_slug("Ab").is_err());
    }

    #[test]
    fn corpus_paths() {
        assert!(validate_corpus_path("refund/policy.md").is_ok());
        assert!(validate_corpus_path("refund.md").is_ok());
        assert!(validate_corpus_path("a/b/c.md").is_err());
        assert!(validate_corpus_path("../x.md").is_err());
        assert!(validate_corpus_path("refund/notes.txt").is_err());
        assert!(validate_repo_path(".git/config").is_err());
        assert!(validate_repo_path("src/main.rs").is_ok());
    }

    #[test]
    fn question_rules() {
        let q = |kind, options: Option<Vec<&str>>| Question {
            id: "q".into(),
            kind,
            prompt: "p".into(),
            options: options.map(|o| o.into_iter().map(String::from).collect()),
            criteria: None,
        };
        assert!(validate_questions(&[q(QuestionKind::Choice, Some(vec!["a", "b"]))]).is_ok());
        assert!(validate_questions(&[q(QuestionKind::Choice, Some(vec!["a"]))]).is_err());
        assert!(validate_questions(&[q(QuestionKind::Noul, Some(vec!["a", "b"]))]).is_err());
        assert!(validate_questions(&[q(QuestionKind::Noul, None), q(QuestionKind::Noul, None)]).is_err());
    }
}
