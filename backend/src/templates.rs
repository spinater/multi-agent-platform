//! Project templates: a development process (task cards, the verify gate, docs folders, agents for every CLI) laid
//! into a project's folder, and the init agent that fills in the project-specific parts.
//!
//! A template is a folder with a `template.toml` and the files to lay down. One is built into the binary
//! (`backend/templates/team-process/`); more come from `templates.dirs` in `map.toml`. Laying one down never
//! overwrites a file: existing files are skipped, and `append` files (`.gitignore`) only get their missing lines.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::hub::Hub;
use crate::model::{CheckLine, InitPlan, InitStarted, Level, PlanAction, PlannedFile, Project, TemplateFile, TemplateInfo, TemplateSource};
use crate::store::NewTask;

/// The built-in template's files, embedded at build time. A test checks this list against the folder.
macro_rules! builtin {
    ($dir:literal: $($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/templates/", $dir, "/", $path)))),*]
    };
}

const TEAM_PROCESS: &[(&str, &str)] = builtin!["team-process":
    "template.toml",
    "CLAUDE.md",
    "AGENTS.md",
    "GEMINI.md",
    "gitignore",
    "tasks/README.md",
    "tasks/todo/.gitkeep",
    "tasks/todo-human/.gitkeep",
    "tasks/done/.gitkeep",
    ".docs/knowledge/domain/.gitkeep",
    ".docs/knowledge/ops/.gitkeep",
    ".docs/design/.gitkeep",
    ".docs/roadmap/.gitkeep",
    ".docs/runbook/.gitkeep",
    "scripts/verify.sh",
    "scripts/task-move.sh",
    "agents/code-reviewer.md",
    "agents/security-reviewer.md",
    "agents/architect.md",
    "agents/qa-tester.md",
    "agents/sa-requirements.md",
];

/// The init agent's instructions shipped with MAP; `templates.init_agent` in `map.toml` replaces them.
pub const INIT_AGENT: &str = include_str!("../templates/init-agent.md");

pub const INIT_TITLE: &str = "Init project";

const MANIFEST: &str = "template.toml";
/// Limits for a user template folder, so a mistyped path (a home folder) is refused rather than read whole.
const MAX_FILES: usize = 400;
const MAX_FILE_BYTES: u64 = 1 << 20;

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"MAP:FILL\s+([A-Za-z0-9_-]+)").unwrap());
static NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9-]{0,62}$").unwrap());

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    name: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    fill: Vec<String>,
    #[serde(default)]
    verify: Vec<String>,
    #[serde(default)]
    append: Vec<String>,
    /// Source path in the template → path in the project.
    #[serde(default)]
    rename: BTreeMap<String, String>,
    /// A folder of agent definitions rendered for every CLI instead of copied.
    agents: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Template {
    pub info: TemplateInfo,
    /// (path in the project, content), sorted by path; rendered agents included.
    files: Vec<(String, String)>,
}

/// The names of the `MAP:FILL` placeholders in `text`, in order, without repeats.
pub fn placeholders(text: &str) -> Vec<String> {
    let mut seen = BTreeSet::new();
    PLACEHOLDER.captures_iter(text).map(|c| c[1].to_string()).filter(|n| seen.insert(n.clone())).collect()
}

/// A relative path that stays inside the folder and never reaches into `.git`.
fn valid_path(path: &str) -> Result<()> {
    let p = Path::new(path);
    let ok = !path.is_empty() && path.len() <= 300 && !path.contains('\\') && p.components().all(|c| matches!(c, Component::Normal(s) if s != ".git"));
    if !ok {
        bail!("template path `{path}` must be relative, without `..` or `.git`");
    }
    Ok(())
}

// ---- agents: one definition, rendered for Claude Code, Codex and agy

struct Agent {
    name: String,
    description: String,
    read_only: bool,
    /// Claude Code's model (`opus`, `sonnet`, `inherit`…).
    model: String,
    body: String,
}

const READ_ONLY_NOTE: &str = "Do not modify files.";

fn parse_agent(path: &str, text: &str) -> Result<Agent> {
    let rest = text.strip_prefix("---\n").with_context(|| format!("{path}: an agent starts with a `---` frontmatter block"))?;
    let (head, body) = rest.split_once("\n---\n").with_context(|| format!("{path}: the frontmatter has no closing `---`"))?;
    let mut fields = BTreeMap::new();
    for line in head.lines().filter(|l| !l.trim().is_empty()) {
        let (k, v) = line.split_once(':').with_context(|| format!("{path}: frontmatter line `{line}` is not `key: value`"))?;
        fields.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
    }
    let take = |k: &str| fields.get(k).cloned().filter(|v| !v.is_empty());
    let name = take("name").with_context(|| format!("{path}: an agent needs a name"))?;
    if !NAME.is_match(&name) {
        bail!("{path}: agent name `{name}` must be a-z, 0-9 and -");
    }
    let read_only = match take("read_only").as_deref() {
        None | Some("false") => false,
        Some("true") => true,
        Some(other) => bail!("{path}: read_only must be true or false, not `{other}`"),
    };
    Ok(Agent {
        description: take("description").with_context(|| format!("{path}: an agent needs a description"))?,
        model: take("model").unwrap_or_else(|| "inherit".into()),
        body: body.trim().to_string(),
        read_only,
        name,
    })
}

impl Agent {
    fn instructions(&self) -> String {
        if self.read_only { format!("{}\n\n{READ_ONLY_NOTE}\n", self.body) } else { format!("{}\n", self.body) }
    }

    /// `.claude/agents/<name>.md`: YAML frontmatter (the description JSON-quoted, which YAML reads as is) and the body.
    fn claude(&self) -> String {
        let tools = if self.read_only { "Read, Grep, Glob, Bash" } else { "Read, Grep, Glob, Bash, Edit, Write" };
        let description = serde_json::to_string(&self.description).unwrap_or_default();
        format!("---\nname: {}\ndescription: {description}\ntools: {tools}\nmodel: {}\n---\n\n{}", self.name, self.model, self.instructions())
    }

    /// `.codex/agents/<name>.toml`.
    fn codex(&self) -> Result<String> {
        #[derive(Serialize)]
        struct CodexAgent<'a> {
            name: &'a str,
            description: &'a str,
            developer_instructions: String,
        }
        Ok(toml::to_string(&CodexAgent { name: &self.name, description: &self.description, developer_instructions: self.instructions() })?)
    }

    /// `.agents/agents/<name>.md` for agy: frontmatter with its own tool names.
    fn agy(&self) -> String {
        let tools = if self.read_only {
            "[view_file, list_dir, grep_search, find_by_name, run_command]"
        } else {
            "[view_file, list_dir, grep_search, find_by_name, write_to_file, replace_file_content, run_command]"
        };
        let description = serde_json::to_string(&self.description).unwrap_or_default();
        format!("---\nname: {}\ndescription: {description}\ntools: {tools}\nmodel: inherit\n---\n\n{}", self.name, self.instructions())
    }
}

// ---- parsing and listing

/// Builds a template from its files (`template.toml` among them), as paths relative to the template folder.
pub fn parse(source: TemplateSource, dir: Option<&Path>, files: Vec<(String, String)>) -> Result<Template> {
    let (manifest_text, sources): (Vec<_>, Vec<_>) = files.into_iter().partition(|(p, _)| p == MANIFEST);
    let manifest_text = manifest_text.into_iter().next().map(|(_, t)| t).context("a template needs a template.toml")?;
    let m: Manifest = toml::from_str(&manifest_text).context("parsing template.toml")?;
    if !NAME.is_match(&m.name) {
        bail!("template name `{}` must be a-z, 0-9 and - (up to 63)", m.name);
    }
    let agents_dir = m.agents.as_deref().map(|a| format!("{}/", a.trim_matches('/')));

    let mut out: BTreeMap<String, String> = BTreeMap::new();
    let mut put = |path: String, content: String| -> Result<()> {
        valid_path(&path)?;
        if out.insert(path.clone(), content).is_some() {
            bail!("two template files land on `{path}`");
        }
        Ok(())
    };
    for (path, content) in sources {
        valid_path(&path)?;
        if let Some(prefix) = &agents_dir
            && path.starts_with(prefix.as_str())
        {
            if !path.ends_with(".md") {
                continue;
            }
            let agent = parse_agent(&path, &content)?;
            put(format!(".claude/agents/{}.md", agent.name), agent.claude())?;
            put(format!(".codex/agents/{}.toml", agent.name), agent.codex()?)?;
            put(format!(".agents/agents/{}.md", agent.name), agent.agy())?;
            continue;
        }
        let target = m.rename.get(&path).cloned().unwrap_or(path);
        put(target, content)?;
    }
    for (what, list) in [("fill", &m.fill), ("append", &m.append)] {
        if let Some(missing) = list.iter().find(|p| !out.contains_key(p.as_str())) {
            bail!("`{what}` names `{missing}`, which the template does not lay down");
        }
    }
    if m.verify.iter().any(|c| c.trim().is_empty()) {
        bail!("verify commands cannot be empty");
    }
    let append: BTreeSet<&String> = m.append.iter().collect();
    let info = TemplateInfo {
        title: if m.title.trim().is_empty() { m.name.clone() } else { m.title.trim().to_string() },
        name: m.name,
        description: m.description.trim().to_string(),
        source,
        dir: dir.map(|d| d.display().to_string()),
        fill: m.fill.clone(),
        verify: m.verify,
        files: out.iter().map(|(path, content)| TemplateFile { path: path.clone(), placeholders: placeholders(content), append: append.contains(path) }).collect(),
    };
    Ok(Template { info, files: out.into_iter().collect() })
}

fn builtin() -> Template {
    let files = TEAM_PROCESS.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect();
    parse(TemplateSource::Builtin, None, files).expect("the built-in template parses")
}

/// Every file below `dir` (relative paths), skipping `.git` folders and symlinks.
fn read_folder(dir: &Path) -> Result<Vec<(String, String)>> {
    let mut out = vec![];
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(dir.join(&rel))?.collect::<std::io::Result<_>>()?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let kind = entry.file_type()?;
            let path = rel.join(entry.file_name());
            if kind.is_dir() && entry.file_name() != ".git" {
                stack.push(path);
            } else if kind.is_file() {
                if out.len() >= MAX_FILES {
                    bail!("{} has more than {MAX_FILES} files", dir.display());
                }
                if entry.metadata()?.len() > MAX_FILE_BYTES {
                    bail!("{} is larger than 1 MiB", path.display());
                }
                let text = std::fs::read_to_string(entry.path()).with_context(|| format!("{} is not UTF-8 text", path.display()))?;
                out.push((path.to_str().context("non-UTF-8 path")?.to_string(), text));
            }
        }
    }
    Ok(out)
}

/// The templates in one `templates.dirs` folder: each subfolder with a `template.toml`. Broken ones are logged and left out.
fn user_templates(root: &Path) -> Vec<Template> {
    let Ok(entries) = std::fs::read_dir(root) else { return vec![] };
    let mut dirs: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.join(MANIFEST).is_file()).collect();
    dirs.sort();
    dirs.into_iter()
        .filter_map(|dir| match read_folder(&dir).and_then(|files| parse(TemplateSource::User, Some(&dir), files)) {
            Ok(t) => Some(t),
            Err(err) => {
                tracing::warn!("template {}: {err:#}", dir.display());
                None
            }
        })
        .collect()
}

/// The built-in templates, then those from `templates.dirs`; a user template with a built-in's name replaces it.
pub fn list(hub: &Hub) -> Vec<Template> {
    let mut out = vec![builtin()];
    for dir in &hub.config.templates.dirs {
        for t in user_templates(dir) {
            match out.iter_mut().find(|o| o.info.name == t.info.name) {
                Some(slot) => *slot = t,
                None => out.push(t),
            }
        }
    }
    out
}

pub fn find(hub: &Hub, name: &str) -> Result<Template> {
    list(hub).into_iter().find(|t| t.info.name == name).with_context(|| format!("unknown template `{name}`"))
}

// ---- planning and laying down

/// Lines of `text` worth appending: not blank, not already in `existing` (compared trimmed).
fn missing_lines(existing: &str, text: &str) -> Vec<String> {
    let have: BTreeSet<&str> = existing.lines().map(str::trim).collect();
    let mut seen = BTreeSet::new();
    text.lines().map(str::trim).filter(|l| !l.is_empty() && !have.contains(l) && seen.insert(*l)).map(String::from).collect()
}

/// The folder holds nothing, or only `.git`.
pub fn folder_empty(folder: &Path) -> bool {
    std::fs::read_dir(folder).map(|mut it| it.all(|e| e.is_ok_and(|e| e.file_name() == ".git"))).unwrap_or(true)
}

/// What laying `t` into `folder` would do to each file. Never follows a symlink inside the folder.
pub fn plan(t: &Template, folder: &Path) -> Result<InitPlan> {
    if !folder.is_dir() {
        bail!("{} is not a folder", folder.display());
    }
    let mut files = vec![];
    for (path, content) in &t.files {
        let rel = Path::new(path);
        let append = t.info.files.iter().any(|f| &f.path == path && f.append);
        let mut file = PlannedFile { path: path.clone(), action: PlanAction::Create, placeholders: placeholders(content), lines: vec![], note: None };
        // Each folder on the way must be a real folder (or missing): a symlink could lead outside the project.
        let mut blocked = None;
        let mut at = folder.to_path_buf();
        for part in rel.parent().into_iter().flat_map(|p| p.components()) {
            at.push(part);
            match std::fs::symlink_metadata(&at) {
                Ok(m) if m.file_type().is_symlink() => bail!("{} is a symlink; the template does not write through it", at.display()),
                Ok(m) if !m.is_dir() => {
                    blocked = Some(format!("{} is a file, not a folder", at.strip_prefix(folder).unwrap_or(&at).display()));
                    break;
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        let target = folder.join(rel);
        match (blocked, std::fs::symlink_metadata(&target)) {
            (Some(why), _) => {
                file.action = PlanAction::Skip;
                file.placeholders = vec![];
                file.note = Some(why);
            }
            (None, Ok(m)) if m.file_type().is_symlink() => bail!("{} is a symlink; the template does not write through it", target.display()),
            (None, Ok(m)) => {
                let existing = if m.is_file() && m.len() <= MAX_FILE_BYTES { std::fs::read_to_string(&target).ok() } else { None };
                file.placeholders = existing.as_deref().map(placeholders).unwrap_or_default();
                file.action = PlanAction::Skip;
                if append && let Some(existing) = &existing {
                    file.lines = missing_lines(existing, content);
                    if file.lines.is_empty() {
                        file.note = Some("exists · already has every line".into());
                    } else {
                        file.action = PlanAction::Append;
                        file.note = Some(format!("exists · {} line{} added", file.lines.len(), if file.lines.len() == 1 { "" } else { "s" }));
                    }
                } else {
                    file.note = Some(if m.is_dir() { "a folder is there".into() } else { "exists · left as it is".into() });
                }
            }
            (None, Err(_)) => {}
        }
        files.push(file);
    }
    Ok(InitPlan { template: t.info.name.clone(), workspace: folder.display().to_string(), folder_empty: folder_empty(folder), files })
}

/// Writes what [`plan`] says: new files (opened create-new, so an existing one is never replaced) and the missing
/// lines of append files. Shell scripts are made executable.
pub fn apply(t: &Template, folder: &Path) -> Result<InitPlan> {
    use std::io::Write;
    let plan = plan(t, folder)?;
    for file in &plan.files {
        let target = folder.join(&file.path);
        match file.action {
            PlanAction::Skip => {}
            PlanAction::Create => {
                let content = t.files.iter().find(|(p, _)| p == &file.path).map(|(_, c)| c.as_str()).unwrap_or_default();
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
                }
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&target).with_context(|| format!("creating {}", target.display()))?;
                f.write_all(content.as_bytes())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if file.path.ends_with(".sh") {
                        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
                    }
                }
            }
            PlanAction::Append => {
                let existing = std::fs::read_to_string(&target)?;
                let mut f = std::fs::OpenOptions::new().append(true).open(&target).with_context(|| format!("opening {}", target.display()))?;
                let lead = if existing.is_empty() || existing.ends_with('\n') { "" } else { "\n" };
                writeln!(f, "{lead}{}", file.lines.join("\n"))?;
            }
        }
    }
    Ok(plan)
}

// ---- the init agent

/// Everything the init agent is told about this run, besides its instructions.
pub struct InitContext<'a> {
    pub project: &'a Project,
    pub workspace: &'a Path,
    pub description: &'a str,
    pub template: &'a TemplateInfo,
    pub plan: &'a InitPlan,
    /// The folder's top-level entries before the template, `.git` left out.
    pub before: &'a [String],
    /// The verify commands the orchestrator runs after the agent reports done.
    pub verify: &'a [String],
}

/// The init task's prompt: the instructions, then this run's facts (description, files, placeholders, checks).
pub fn init_prompt(instructions: &str, c: &InitContext) -> String {
    let mut p = format!("{}\n\n---\n# This run\n\n", instructions.trim());
    p.push_str(&format!("- Project: {} (`{}`), folder `{}`.\n", c.project.name, c.project.slug, c.workspace.display()));
    p.push_str(&format!("- Template: {} (`{}`). {}\n", c.template.title, c.template.name, c.template.description));
    p.push_str("\n## The folder\n\n");
    if c.plan.folder_empty {
        p.push_str("It was empty before the template (nothing but `.git`, if that). Only the template's files are there now.\n");
    } else {
        let shown: Vec<&str> = c.before.iter().take(40).map(String::as_str).collect();
        let more = if c.before.len() > shown.len() { format!(" and {} more", c.before.len() - shown.len()) } else { String::new() };
        p.push_str(&format!("It already had files before the template: {}{more}. Do not set up a skeleton here.\n", shown.join(", ")));
    }
    p.push_str("\n## The owner's description\n\n");
    p.push_str(c.description.trim());
    p.push_str("\n\n## Files laid down\n\n");
    for f in &c.plan.files {
        let line = match f.action {
            PlanAction::Create => format!("- created: `{}`\n", f.path),
            PlanAction::Append => format!("- appended to the existing `{}`: {}\n", f.path, f.lines.join(", ")),
            PlanAction::Skip => format!("- skipped, existed before (leave it alone): `{}`{}\n", f.path, f.note.as_deref().map(|n| format!(" ({n})")).unwrap_or_default()),
        };
        p.push_str(&line);
    }
    p.push_str("\n## Placeholders to fill\n\n");
    let open: Vec<&PlannedFile> = c.plan.files.iter().filter(|f| f.action != PlanAction::Skip && !f.placeholders.is_empty()).collect();
    if open.is_empty() {
        p.push_str("None in the files laid down this time.\n");
    }
    for f in open {
        p.push_str(&format!("- `{}`: {}\n", f.path, f.placeholders.join(", ")));
    }
    let left: Vec<&PlannedFile> = c.plan.files.iter().filter(|f| f.action == PlanAction::Skip && !f.placeholders.is_empty()).collect();
    if !left.is_empty() {
        p.push_str("\nStill unfilled from an earlier init (these files came from the template; fill them too):\n");
        for f in left {
            p.push_str(&format!("- `{}`: {}\n", f.path, f.placeholders.join(", ")));
        }
    }
    if !c.template.fill.is_empty() {
        p.push_str(&format!("\nThe template's files to complete: {}.\n", c.template.fill.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")));
    }
    p.push_str("\n## Checks after you report done\n\n");
    if c.verify.is_empty() {
        p.push_str("No verify commands are set for this project; run `bash scripts/verify.sh` yourself anyway.\n");
    } else {
        p.push_str(&format!("The orchestrator runs: {}. They must exit 0, or the task is retried with their output.\n", c.verify.iter().map(|v| format!("`{v}`")).collect::<Vec<_>>().join(", ")));
    }
    p
}

/// The init agent's instructions: `templates.init_agent` when set, else the built-in ones.
pub fn instructions(hub: &Hub) -> Result<String> {
    match &hub.config.templates.init_agent {
        Some(path) => std::fs::read_to_string(path).with_context(|| format!("reading templates.init_agent {}", path.display())),
        None => Ok(INIT_AGENT.to_string()),
    }
}

fn top_level(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|it| it.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n != ".git").collect())
        .unwrap_or_default();
    names.sort();
    names
}

pub struct InitRequest {
    pub template: String,
    pub description: String,
    pub provider: String,
    pub model: Option<String>,
    pub set_verify: bool,
}

fn check(checks: &mut Vec<CheckLine>, level: Level, text: impl Into<String>) {
    checks.push(CheckLine { level, text: text.into() });
}

/// Lays the template into the project's folder, optionally takes its verify commands for the project, and queues the
/// init agent as an orchestrator task pinned to the chosen provider, so its work is verified (and retried) like any task.
pub async fn init(hub: &Hub, slug: &str, req: InitRequest) -> Result<InitStarted> {
    let mut project = hub.store.project(slug).await?.with_context(|| format!("unknown project `{slug}`"))?;
    let description = req.description.trim();
    if description.is_empty() {
        bail!("describe the project: what it does, its language or stack, and anything the agents must know");
    }
    let spec = hub.spec(&req.provider)?;
    let model = req.model.map(|m| m.trim().to_string()).filter(|m| !m.is_empty());
    if model.is_some() {
        Hub::resolve_model(spec, model.as_deref())?;
    }
    if !hub.provider_available(&req.provider) {
        bail!("{} is not found on PATH; pick another provider or install it", req.provider);
    }
    let template = find(hub, &req.template)?;
    let instructions = instructions(hub)?;
    let workspace = hub.workspace_of(&project);
    let before = top_level(&workspace);
    let plan = apply(&template, &workspace)?;

    let mut checks = vec![];
    let count = |a: PlanAction| plan.files.iter().filter(|f| f.action == a).count();
    check(
        &mut checks,
        Level::Success,
        format!("template {} · {} created · {} appended · {} skipped (exist)", template.info.name, count(PlanAction::Create), count(PlanAction::Append), count(PlanAction::Skip)),
    );
    for f in plan.files.iter().filter(|f| f.action != PlanAction::Create) {
        let level = if f.action == PlanAction::Append { Level::Success } else { Level::Info };
        check(&mut checks, level, format!("{} · {}", f.path, f.note.as_deref().unwrap_or("skipped")));
    }

    if req.set_verify && !template.info.verify.is_empty() {
        project.settings.verify = Some(template.info.verify.clone());
        hub.put_project(&project).await?;
        check(&mut checks, Level::Success, format!("verify set for {slug}: {}", template.info.verify.join(" · ")));
    }
    let verify = project.settings.verify.clone().unwrap_or_else(|| hub.config.orchestrator.verify.clone());
    if !req.set_verify {
        let source = if project.settings.verify.is_some() { "the project's" } else { "map.toml's" };
        check(&mut checks, Level::Info, format!("verify: {} {source} command(s) run after the init agent{}", verify.len(), if verify.is_empty() { String::new() } else { format!(": {}", verify.join(" · ")) }));
    }

    let ctx = InitContext { project: &project, workspace: &workspace, description, template: &template.info, plan: &plan, before: &before, verify: &verify };
    let prompt = init_prompt(&instructions, &ctx);
    let task = hub
        .store
        .create_task(NewTask { title: INIT_TITLE.into(), prompt, depends_on: vec![], provider: Some(req.provider.clone()), model: model.clone(), project: Some(slug.to_string()) })
        .await?;
    hub.announce_task(&task);
    hub.event(Level::Info, "task.created", None, Some(task.id), format!("Task #{} created: {}", task.id, task.title)).await;
    hub.event(Level::Info, "project.init", None, Some(task.id), format!("{slug}: template {} laid down; init agent queued as task #{}", template.info.name, task.id)).await;
    let model_text = model.unwrap_or_else(|| "project or default model".into());
    check(&mut checks, Level::Success, format!("task #{} {INIT_TITLE} queued · {} ({model_text})", task.id, req.provider));
    if !hub.orchestrator_running() {
        check(&mut checks, Level::Warning, "the orchestrator is paused: press Run in the command centre to start the init agent");
    }
    if project.settings.paused {
        check(&mut checks, Level::Warning, format!("{slug} is paused: resume it so the init task can start"));
    }
    Ok(InitStarted { plan, checks, task })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::config::Config;
    use crate::model::{ProjectSettings, ProjectSource, SourceKind, TaskStatus};
    use crate::projects::{NewProject, create_with};
    use crate::store::Store;
    use crate::terminal::fake::FakeTerminal;

    /// `claude` counts as installed (`/bin/sh`); templates only from `<tmp>/templates`.
    async fn hub(extra: &str) -> (Arc<Hub>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[orchestrator]\nprovider_order = [\"claude\"]\nverify = [\"global-check\"]\n[orchestrator.audit]\nchain = [\"claude\"]\n[reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"/bin/sh\"\ncredentials = []\n[providers.codex]\nbinary = \"/nonexistent/codex\"\n[templates]\ndirs = [\"{0}/templates\"]\n{extra}\n",
            dir.path().display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        (Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default())), dir)
    }

    async fn project(hub: &Hub, folder: &Path) -> String {
        let settings = ProjectSettings { source: ProjectSource { kind: SourceKind::Local, location: Some(folder.display().to_string()), branch: None }, ..Default::default() };
        let new = NewProject { name: "App".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        create_with(hub, new, settings).await.unwrap().0.slug
    }

    fn request(description: &str) -> InitRequest {
        InitRequest { template: "team-process".into(), description: description.into(), provider: "claude".into(), model: Some("opus".into()), set_verify: true }
    }

    #[test]
    fn builtin_template_parses_and_lists_every_file() {
        let t = builtin();
        assert_eq!(t.info.name, "team-process");
        assert_eq!(t.info.verify, vec!["bash scripts/verify.sh"]);
        let paths: Vec<&str> = t.info.files.iter().map(|f| f.path.as_str()).collect();
        for p in ["CLAUDE.md", "AGENTS.md", "GEMINI.md", ".gitignore", "tasks/README.md", "tasks/todo-human/.gitkeep", ".docs/runbook/.gitkeep", "scripts/verify.sh", "scripts/task-move.sh"] {
            assert!(paths.contains(&p), "{p} missing from {paths:?}");
        }
        assert!(!paths.iter().any(|p| p.starts_with("agents/") || *p == "template.toml" || *p == "gitignore"), "{paths:?}");
        for agent in ["code-reviewer", "security-reviewer", "architect", "qa-tester", "sa-requirements"] {
            for p in [format!(".claude/agents/{agent}.md"), format!(".codex/agents/{agent}.toml"), format!(".agents/agents/{agent}.md")] {
                assert!(paths.contains(&p.as_str()), "{p}");
            }
        }
        let claude = t.info.files.iter().find(|f| f.path == "CLAUDE.md").unwrap();
        assert_eq!(claude.placeholders, ["what-this-is", "read-before-touching", "invariants", "stack-and-commands", "deploy", "review-paths"]);
        let verify = t.info.files.iter().find(|f| f.path == "scripts/verify.sh").unwrap();
        assert_eq!(verify.placeholders, ["stack-checks"]);
        assert!(t.info.files.iter().find(|f| f.path == ".gitignore").unwrap().append);
        // Only the fill files carry placeholders.
        let with: BTreeSet<&str> = t.info.files.iter().filter(|f| !f.placeholders.is_empty()).map(|f| f.path.as_str()).collect();
        assert_eq!(with, t.info.fill.iter().map(String::as_str).collect());
    }

    /// The embedded list matches the folder, so a template file added on disk is not silently left out.
    #[test]
    fn embedded_list_matches_the_folder() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("templates/team-process");
        let mut on_disk: Vec<String> = read_folder(&dir).unwrap().into_iter().map(|(p, _)| p).collect();
        let mut embedded: Vec<String> = TEAM_PROCESS.iter().map(|(p, _)| p.to_string()).collect();
        on_disk.sort();
        embedded.sort();
        assert_eq!(on_disk, embedded);
    }

    #[test]
    fn agents_render_for_each_cli() {
        let t = builtin();
        let file = |p: &str| t.files.iter().find(|(x, _)| x == p).map(|(_, c)| c.clone()).unwrap();
        let claude = file(".claude/agents/code-reviewer.md");
        assert!(claude.starts_with("---\nname: code-reviewer\ndescription: \""), "{claude}");
        assert!(claude.contains("\ntools: Read, Grep, Glob, Bash\nmodel: opus\n---\n") && claude.contains(READ_ONLY_NOTE));
        let qa = file(".claude/agents/qa-tester.md");
        assert!(qa.contains("tools: Read, Grep, Glob, Bash, Edit, Write") && !qa.contains(READ_ONLY_NOTE));

        #[derive(Deserialize)]
        struct Codex {
            name: String,
            description: String,
            developer_instructions: String,
        }
        let codex: Codex = toml::from_str(&file(".codex/agents/architect.toml")).unwrap();
        assert_eq!(codex.name, "architect");
        assert!(codex.description.contains("Read-only"));
        assert!(codex.developer_instructions.trim_end().ends_with(READ_ONLY_NOTE));
        let codex: Codex = toml::from_str(&file(".codex/agents/qa-tester.toml")).unwrap();
        assert!(!codex.developer_instructions.contains(READ_ONLY_NOTE));

        let agy = file(".agents/agents/sa-requirements.md");
        assert!(agy.contains("tools: [view_file, list_dir, grep_search, find_by_name, run_command]\nmodel: inherit\n"), "{agy}");
        assert!(file(".agents/agents/qa-tester.md").contains("write_to_file, replace_file_content"));
    }

    #[test]
    fn plan_and_apply_never_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        std::fs::write(folder.join("CLAUDE.md"), "# Mine\n").unwrap();
        std::fs::create_dir_all(folder.join("scripts")).unwrap();
        std::fs::write(folder.join("scripts/verify.sh"), "echo mine\n").unwrap();
        let t = builtin();

        let plan = plan(&t, folder).unwrap();
        assert!(!plan.folder_empty);
        let action = |plan: &InitPlan, p: &str| plan.files.iter().find(|f| f.path == p).unwrap().action;
        assert_eq!(action(&plan, "CLAUDE.md"), PlanAction::Skip);
        assert_eq!(action(&plan, "scripts/verify.sh"), PlanAction::Skip);
        assert_eq!(action(&plan, "AGENTS.md"), PlanAction::Create);
        assert_eq!(action(&plan, ".gitignore"), PlanAction::Create);
        assert!(plan.files.iter().find(|f| f.path == "CLAUDE.md").unwrap().placeholders.is_empty(), "the user's file has none");
        // Planning writes nothing.
        assert!(!folder.join("AGENTS.md").exists());

        let applied = apply(&t, folder).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join("CLAUDE.md")).unwrap(), "# Mine\n");
        assert_eq!(std::fs::read_to_string(folder.join("scripts/verify.sh")).unwrap(), "echo mine\n");
        assert!(folder.join("tasks/todo-human/.gitkeep").is_file());
        assert!(folder.join(".codex/agents/code-reviewer.toml").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(folder.join("scripts/task-move.sh")).unwrap().permissions().mode() & 0o111, 0o111);
        }
        assert_eq!(applied.files.len(), plan.files.len());

        // A second run skips everything, and a user's edit to a laid-down file survives it.
        std::fs::write(folder.join("AGENTS.md"), "edited\n").unwrap();
        let again = apply(&t, folder).unwrap();
        assert!(again.files.iter().all(|f| f.action == PlanAction::Skip), "{:?}", again.files.iter().filter(|f| f.action != PlanAction::Skip).collect::<Vec<_>>());
        assert_eq!(std::fs::read_to_string(folder.join("AGENTS.md")).unwrap(), "edited\n");
    }

    #[test]
    fn gitignore_append_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        std::fs::write(folder.join(".gitignore"), "node_modules/\n/.env").unwrap();
        let t = builtin();
        let plan = plan(&t, folder).unwrap();
        let gi = plan.files.iter().find(|f| f.path == ".gitignore").unwrap();
        assert_eq!(gi.action, PlanAction::Append);
        assert_eq!(gi.lines, vec!["/.scratch/"]);
        apply(&t, folder).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join(".gitignore")).unwrap(), "node_modules/\n/.env\n/.scratch/\n");
        let again = super::plan(&t, folder).unwrap();
        assert_eq!(again.files.iter().find(|f| f.path == ".gitignore").unwrap().action, PlanAction::Skip);
        apply(&t, folder).unwrap();
        assert_eq!(std::fs::read_to_string(folder.join(".gitignore")).unwrap(), "node_modules/\n/.env\n/.scratch/\n");
    }

    #[test]
    fn refuses_to_write_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let folder = dir.path();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), folder.join("tasks")).unwrap();
            assert!(plan(&builtin(), folder).is_err());
            assert!(apply(&builtin(), folder).is_err());
            assert_eq!(std::fs::read_dir(outside.path()).unwrap().count(), 0);
        }
    }

    #[test]
    fn empty_folder_and_git_only_count_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(folder_empty(dir.path()));
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert!(folder_empty(dir.path()));
        std::fs::write(dir.path().join("a.txt"), "").unwrap();
        assert!(!folder_empty(dir.path()));
    }

    #[test]
    fn user_template_rules() {
        let ok = |files: &[(&str, &str)]| parse(TemplateSource::User, None, files.iter().map(|(p, c)| (p.to_string(), c.to_string())).collect());
        assert!(ok(&[("README.md", "x")]).is_err(), "no template.toml");
        assert!(ok(&[("template.toml", "name = \"Bad Name\"")]).is_err());
        assert!(ok(&[("template.toml", "name = \"t\"\nfill = [\"nope.md\"]")]).is_err(), "fill names a missing file");
        assert!(ok(&[("template.toml", "name = \"t\""), ("../x", "")]).is_err());
        assert!(ok(&[("template.toml", "name = \"t\""), (".git/config", "")]).is_err());
        assert!(ok(&[("template.toml", "name = \"t\"\nrename = { \"b\" = \"a\" }"), ("a", ""), ("b", "")]).is_err(), "two files land on one path");
        let t = ok(&[("template.toml", "name = \"t\"\nagents = \"ag\""), ("ag/x.md", "---\nname: x\ndescription: d: with colon\n---\nbody")]).unwrap();
        assert_eq!(t.info.files.len(), 3);
        assert_eq!(t.info.title, "t");
        assert!(ok(&[("template.toml", "name = \"t\"\nagents = \"ag\""), ("ag/x.md", "no frontmatter")]).is_err());
    }

    #[tokio::test]
    async fn user_template_dir_is_listed_and_replaces_a_builtin_of_the_same_name() {
        let (hub, dir) = hub("").await;
        let mine = dir.path().join("templates/minimal");
        std::fs::create_dir_all(mine.join("docs")).unwrap();
        std::fs::write(mine.join("template.toml"), "name = \"minimal\"\ntitle = \"Minimal\"\nfill = [\"NOTES.md\"]\nverify = [\"make check\"]\n").unwrap();
        std::fs::write(mine.join("NOTES.md"), "<!-- MAP:FILL purpose: why -->\n").unwrap();
        std::fs::write(mine.join("docs/a.md"), "a").unwrap();
        // A broken template next to it is left out, not fatal.
        std::fs::create_dir_all(dir.path().join("templates/broken")).unwrap();
        std::fs::write(dir.path().join("templates/broken/template.toml"), "name = 3").unwrap();

        let names: Vec<String> = list(&hub).into_iter().map(|t| t.info.name).collect();
        assert_eq!(names, ["team-process", "minimal"]);
        let t = find(&hub, "minimal").unwrap();
        assert_eq!(t.info.source, TemplateSource::User);
        assert_eq!(t.info.dir.as_deref(), Some(mine.display().to_string().as_str()));
        assert_eq!(t.info.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["NOTES.md", "docs/a.md"]);
        assert_eq!(t.info.files[0].placeholders, ["purpose"]);

        let shadow = dir.path().join("templates/zz-shadow");
        std::fs::create_dir_all(&shadow).unwrap();
        std::fs::write(shadow.join("template.toml"), "name = \"team-process\"\ntitle = \"Mine\"\n").unwrap();
        let t = find(&hub, "team-process").unwrap();
        assert_eq!((t.info.title.as_str(), t.info.source), ("Mine", TemplateSource::User));
        assert!(find(&hub, "nope").is_err());
    }

    #[tokio::test]
    async fn init_lays_down_the_template_sets_verify_and_queues_the_init_task() {
        let (hub, dir) = hub("").await;
        let folder = dir.path().join("app");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("README.md"), "hello\n").unwrap();
        let slug = project(&hub, &folder).await;

        assert!(init(&hub, &slug, request("  ")).await.is_err(), "a description is required");
        assert!(init(&hub, &slug, InitRequest { provider: "codex".into(), ..request("x") }).await.is_err(), "codex is not installed");
        assert!(init(&hub, &slug, InitRequest { model: Some("gpt".into()), ..request("x") }).await.is_err(), "not a claude model");
        assert!(init(&hub, "nope", request("x")).await.is_err());
        assert!(!folder.join("CLAUDE.md").exists(), "a refused init writes nothing");

        let started = init(&hub, &slug, request("A booking API for gyms, in Rust with axum and Postgres.")).await.unwrap();
        assert!(folder.join("CLAUDE.md").is_file() && folder.join("tasks/done/.gitkeep").is_file());
        assert_eq!(std::fs::read_to_string(folder.join("README.md")).unwrap(), "hello\n");
        assert!(!started.plan.folder_empty);

        let task = &started.task;
        assert_eq!((task.title.as_str(), task.provider.as_deref(), task.model.as_deref(), task.project.as_deref()), (INIT_TITLE, Some("claude"), Some("opus"), Some(slug.as_str())));
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(task.prompt.contains("A booking API for gyms, in Rust with axum and Postgres."));
        assert!(task.prompt.contains("- `CLAUDE.md`: what-this-is, read-before-touching, invariants, stack-and-commands, deploy, review-paths"), "{}", task.prompt);
        assert!(task.prompt.contains("- `scripts/verify.sh`: stack-checks"));
        assert!(task.prompt.contains("already had files before the template: README.md"));
        assert!(task.prompt.contains("The orchestrator runs: `bash scripts/verify.sh`"));
        assert!(task.prompt.starts_with("# Init project"), "the built-in instructions come first");

        let p = hub.store.project(&slug).await.unwrap().unwrap();
        assert_eq!(p.settings.verify, Some(vec!["bash scripts/verify.sh".to_string()]));
        let text: Vec<&str> = started.checks.iter().map(|c| c.text.as_str()).collect();
        assert!(text[0].starts_with("template team-process · ") && text[0].ends_with("0 skipped (exist)"), "{text:?}");
        assert!(text.iter().any(|t| t.starts_with(&format!("task #{} Init project queued · claude (opus)", task.id))), "{text:?}");
        assert!(text.iter().any(|t| t.contains("orchestrator is paused")), "{text:?}");

        // Again, without taking the template's verify: everything is skipped and the project's commands stay.
        hub.store.save_project(&crate::model::Project { settings: ProjectSettings { verify: None, ..p.settings.clone() }, ..p }).await.unwrap();
        let again = init(&hub, &slug, InitRequest { set_verify: false, model: None, ..request("Same.") }).await.unwrap();
        assert!(again.plan.files.iter().all(|f| f.action == PlanAction::Skip));
        assert_eq!(hub.store.project(&slug).await.unwrap().unwrap().settings.verify, None);
        assert!(again.task.prompt.contains("Still unfilled from an earlier init"), "the placeholders are still in CLAUDE.md");
        assert!(again.task.prompt.contains("The orchestrator runs: `global-check`"));
        assert!(again.checks.iter().any(|c| c.text.starts_with("verify: 1 map.toml's command(s)")));
    }

    #[tokio::test]
    async fn init_agent_instructions_can_be_replaced_in_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("init.md");
        std::fs::write(&path, "# My own init\nDo it my way.\n").unwrap();
        let (hub, tmp) = hub(&format!("init_agent = \"{}\"", path.display())).await;
        let folder = tmp.path().join("fresh");
        std::fs::create_dir_all(folder.join(".git")).unwrap();
        let slug = project(&hub, &folder).await;
        let started = init(&hub, &slug, request("A CLI to rename photos, in Go.")).await.unwrap();
        assert!(started.plan.folder_empty);
        assert!(started.task.prompt.starts_with("# My own init\nDo it my way."));
        assert!(started.task.prompt.contains("It was empty before the template"));

        std::fs::remove_file(&path).unwrap();
        assert!(init(&hub, &slug, request("x")).await.is_err(), "a missing instructions file is an error, not a silent fallback");
    }
}
