//! Configure → Rules: the context files agents read at session start. In a project folder: each provider's root
//! file (`[skills] context_files`: `CLAUDE.md`, `AGENTS.md`, `GEMINI.md`, …), the files those import with Claude
//! Code's and Gemini CLI's `@path` syntax, and rule files MAP created there (`docs/rules/<name>.md`, kept in
//! `runtime_settings` under `rules.extra:<slug>`). On the machine: each CLI's own file in its home folder
//! ([`MACHINE_FILES`], resolved with `Hub::provider_home`, so in isolated mode MAP's agent-home folders).
//!
//! Writes stay inside the project folder (relative `.md` paths only, no `..`, no symlink out, never `.git/`) or are
//! one of the known machine files. Who reads a file is changed only by adding or removing a whole `@<path>` line in
//! a provider's root file; everything else in that file is left as it is.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::hub::Hub;
use crate::model::{ConfigSource, Level, RuleFile, RuleReader, RulesOverview};
use crate::skills::IMPORT;

/// Largest body returned in a listing, and accepted on a write.
pub const MAX_BYTES: usize = 256 * 1024;
/// Providers whose root file follows `@path` imports (Claude Code, Gemini CLI). Codex and the rest read only their
/// own file.
pub const IMPORT_PROVIDERS: &[&str] = &["claude", "gemini"];
/// Each CLI's own rule file in its home folder.
pub const MACHINE_FILES: &[(&str, &str)] = &[("claude", "CLAUDE.md"), ("codex", "AGENTS.md"), ("gemini", "GEMINI.md")];
/// How deep imports are followed (Claude Code's limit).
const IMPORT_DEPTH: usize = 5;

fn extras_key(slug: &str) -> String {
    format!("rules.extra:{slug}")
}

#[derive(Debug, Clone)]
pub struct MachineFile {
    pub provider: String,
    pub abs: PathBuf,
    pub display: String,
}

/// What a rules call works on, gathered before the blocking file work.
#[derive(Debug, Clone)]
pub struct Scope {
    pub project: Option<String>,
    /// The project folder, canonical; `None` in machine scope.
    pub folder: Option<PathBuf>,
    /// Provider → root file, relative to the project folder (`[skills] context_files`).
    pub context_files: BTreeMap<String, String>,
    pub machine: Vec<MachineFile>,
    /// Rule files MAP created in the project, relative.
    pub extras: Vec<String>,
}

fn home_display(abs: &Path) -> String {
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from)
        && let Ok(rel) = abs.strip_prefix(&home)
    {
        return format!("~/{}", rel.display());
    }
    abs.display().to_string()
}

pub async fn scope(hub: &Hub, project: Option<&str>) -> Result<Scope> {
    let folder = match project {
        Some(slug) => {
            let dir = hub.work_dir(Some(slug)).await;
            Some(dir.canonicalize().with_context(|| format!("the project folder {} does not exist", dir.display()))?)
        }
        None => None,
    };
    let machine = MACHINE_FILES
        .iter()
        .filter(|(p, _)| hub.config.providers.contains_key(*p))
        .filter_map(|(p, name)| {
            let abs = hub.provider_home(p)?.join(name);
            Some(MachineFile { provider: p.to_string(), display: home_display(&abs), abs })
        })
        .collect();
    let extras = match project {
        Some(slug) => hub.store.runtime_setting(&extras_key(slug)).await?.and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default(),
        None => vec![],
    };
    Ok(Scope { project: project.map(String::from), folder, context_files: hub.config.skills.context_files.clone(), machine, extras })
}

// ---- paths

/// A rule file path from a request: relative, `.md`, no `..`, not in `.git`.
pub fn check_rel(path: &str) -> Result<PathBuf> {
    let p = path.trim();
    if p.is_empty() {
        bail!("give the rule file's path");
    }
    if p.starts_with('/') || p.starts_with('~') || p.contains('\\') {
        bail!("`{p}`: give a path relative to the project folder");
    }
    let mut out = PathBuf::new();
    for c in Path::new(p).components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            Component::ParentDir => bail!("`{p}`: `..` is not allowed"),
            _ => bail!("`{p}`: give a path relative to the project folder"),
        }
    }
    if out.as_os_str().is_empty() {
        bail!("give the rule file's path");
    }
    if out.components().next() == Some(Component::Normal(".git".as_ref())) {
        bail!("`{p}`: not inside .git");
    }
    if !out.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
        bail!("`{p}`: rule files are Markdown (.md)");
    }
    Ok(out)
}

/// Where `rel` really is, refused when that is outside the project folder (a symlink out) or a folder.
fn inside(folder: &Path, rel: &Path) -> Result<PathBuf> {
    let real = crate::roots::real_path(&folder.join(rel))?;
    if !real.starts_with(folder) {
        bail!("`{}` leads outside the project folder", rel.display());
    }
    if real.is_dir() {
        bail!("`{}` is a folder", rel.display());
    }
    Ok(real)
}

/// `target` (as written in an import) from the folder `base`, both relative to the project folder; `None` when it
/// climbs out of it or is absolute.
fn resolve(base: &Path, target: &str) -> Option<PathBuf> {
    if target.starts_with('/') || target.starts_with('~') {
        return None;
    }
    let mut out = base.to_path_buf();
    for c in Path::new(target).components() {
        match c {
            Component::Normal(n) => out.push(n),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// The import path that names `to` from a file in the folder `from` (both relative to the project folder).
fn import_path(from: &Path, to: &Path) -> String {
    let from: Vec<_> = from.components().collect();
    let to_c: Vec<_> = to.components().collect();
    let common = from.iter().zip(&to_c).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for c in &to_c[common..] {
        out.push(c);
    }
    out.display().to_string()
}

fn parent(p: &Path) -> PathBuf {
    p.parent().map(Path::to_path_buf).unwrap_or_default()
}

// ---- imports

#[derive(Debug, Clone, PartialEq)]
pub struct Import {
    /// As written, `./` dropped.
    pub target: String,
    /// The line is just `@target`: it can be removed without touching anything else.
    pub whole_line: bool,
}

/// The `@path` imports of a context file (outside code fences); `@word` without a dot or slash is a mention.
pub fn imports(text: &str) -> Vec<Import> {
    let mut out: Vec<Import> = vec![];
    let mut fence = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with("```") {
            fence = !fence;
            continue;
        }
        if fence {
            continue;
        }
        for cap in IMPORT.captures_iter(line) {
            let target = cap[1].trim_start_matches("./").to_string();
            if !(target.contains('.') || target.contains('/')) {
                continue;
            }
            let whole_line = line == format!("@{}", &cap[1]);
            match out.iter_mut().find(|i| i.target == target) {
                Some(i) => i.whole_line |= whole_line,
                None => out.push(Import { target, whole_line }),
            }
        }
    }
    out
}

/// `text` without its whole `@…` lines that name `target` from a file in `base`; how many went.
pub fn remove_import_lines(text: &str, base: &Path, target: &Path) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let (mut fence, mut removed) = (false, 0);
    for line in text.split_inclusive('\n') {
        let t = line.trim();
        if t.starts_with("```") {
            fence = !fence;
        } else if !fence
            && let Some(x) = t.strip_prefix('@')
            && !x.is_empty()
            && !x.contains(char::is_whitespace)
            && resolve(base, x.trim_start_matches("./")).as_deref() == Some(target)
        {
            removed += 1;
            continue;
        }
        out.push_str(line);
    }
    (out, removed)
}

/// `text` with a `@<import>` line appended.
fn append_import(text: &str, import: &str) -> String {
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&format!("@{import}\n"));
    out
}

// ---- reading

struct Contents {
    exists: bool,
    bytes: u64,
    lines: u32,
    body: String,
    truncated: bool,
}

fn read(abs: &Path) -> Contents {
    let Ok(raw) = std::fs::read(abs) else { return Contents { exists: false, bytes: 0, lines: 0, body: String::new(), truncated: false } };
    let truncated = raw.len() > MAX_BYTES;
    let all = String::from_utf8_lossy(&raw);
    let mut body = String::from_utf8_lossy(&raw[..raw.len().min(MAX_BYTES)]).into_owned();
    if truncated {
        // A cut in the middle of a character shows up as U+FFFD at the end.
        body = body.trim_end_matches('\u{FFFD}').to_string();
    }
    Contents { exists: true, bytes: raw.len() as u64, lines: all.lines().count() as u32, body, truncated }
}

fn read_text(folder: &Path, rel: &Path) -> Option<String> {
    let real = inside(folder, rel).ok()?;
    let raw = std::fs::read(real).ok()?;
    Some(String::from_utf8_lossy(&raw[..raw.len().min(MAX_BYTES)]).into_owned())
}

/// How a file is reached from a provider's root file.
#[derive(Debug, Clone)]
struct Reach {
    /// The file whose import reaches it first.
    via: PathBuf,
    /// The root file itself imports it.
    direct: bool,
    /// ... on a line of its own.
    whole_line: bool,
}

/// Every file reachable from `root` through imports (up to [`IMPORT_DEPTH`] hops), inside the project folder.
fn reach(folder: &Path, root: &Path) -> HashMap<PathBuf, Reach> {
    let mut out: HashMap<PathBuf, Reach> = HashMap::new();
    let mut queue = VecDeque::from([(root.to_path_buf(), 0usize)]);
    while let Some((file, depth)) = queue.pop_front() {
        if depth >= IMPORT_DEPTH {
            continue;
        }
        let Some(text) = read_text(folder, &file) else { continue };
        for imp in imports(&text) {
            let Some(target) = resolve(&parent(&file), &imp.target) else { continue };
            if target == root {
                continue;
            }
            let direct = file == root;
            match out.get_mut(&target) {
                Some(r) if direct => r.whole_line |= imp.whole_line,
                Some(_) => {}
                None => {
                    out.insert(target.clone(), Reach { via: file.clone(), direct, whole_line: direct && imp.whole_line });
                    queue.push_back((target, depth + 1));
                }
            }
        }
    }
    out
}

fn root_rel(root: &str) -> PathBuf {
    resolve(Path::new(""), root).unwrap_or_else(|| PathBuf::from(root))
}

/// Who reads the project file `rel`, per provider with a context file.
fn project_readers(scope: &Scope, reaches: &BTreeMap<String, HashMap<PathBuf, Reach>>, rel: &Path) -> Vec<RuleReader> {
    let shown = |p: &Path| p.display().to_string();
    scope
        .context_files
        .iter()
        .map(|(provider, root)| {
            let root = root_rel(root);
            let r = |reads: bool, how: String, fixed: bool, note: String| RuleReader { provider: provider.clone(), reads, how, fixed, note };
            if rel == root {
                return r(true, "own file".into(), true, format!("{provider} reads its own {} at session start", shown(&root)));
            }
            if !IMPORT_PROVIDERS.contains(&provider.as_str()) {
                return r(false, String::new(), true, format!("{provider} reads only its own {}; it has no import syntax", shown(&root)));
            }
            let import = import_path(&parent(&root), rel);
            match reaches.get(provider).and_then(|m| m.get(rel)) {
                Some(x) if x.direct && x.whole_line => r(true, format!("via {}", shown(&root)), false, format!("unticking removes the `@{import}` line from {}", shown(&root))),
                Some(x) if x.direct => r(true, format!("via {}", shown(&root)), true, format!("imported inside a line of {}; edit that line by hand", shown(&root))),
                Some(x) => r(true, format!("via {}", shown(&x.via)), true, format!("imported by {}, which {} imports; remove the import there", shown(&x.via), shown(&root))),
                None => r(false, String::new(), false, format!("ticking adds `@{import}` to {}", shown(&root))),
            }
        })
        .collect()
}

fn machine_readers(scope: &Scope, m: &MachineFile) -> Vec<RuleReader> {
    let mut providers: Vec<String> = scope.context_files.keys().cloned().collect();
    if !providers.contains(&m.provider) {
        providers.push(m.provider.clone());
    }
    providers
        .into_iter()
        .map(|p| {
            let own = p == m.provider;
            let note = if own { format!("{p} reads its home folder's file in every project") } else { format!("machine files are read only by their own CLI ({})", m.provider) };
            RuleReader { provider: p, reads: own, how: if own { "own file".into() } else { String::new() }, fixed: true, note }
        })
        .collect()
}

fn rule_file(path: String, abs: &Path, source: ConfigSource, readers: Vec<RuleReader>, root_of: Vec<String>) -> RuleFile {
    let c = read(abs);
    let imports = imports(&c.body).into_iter().map(|i| i.target).collect();
    RuleFile {
        path,
        abs_path: abs.display().to_string(),
        source,
        exists: c.exists,
        lines: c.lines,
        bytes: c.bytes,
        readers: readers.iter().filter(|r| r.reads).map(|r| r.provider.clone()).collect(),
        reader_options: readers,
        imports,
        root_of,
        body: c.body,
        truncated: c.truncated,
    }
}

fn reaches(scope: &Scope, folder: &Path) -> BTreeMap<String, HashMap<PathBuf, Reach>> {
    scope.context_files.iter().filter(|(p, _)| IMPORT_PROVIDERS.contains(&p.as_str())).map(|(p, root)| (p.clone(), reach(folder, &root_rel(root)))).collect()
}

/// The listing: project files (roots that exist, then imported and MAP-made files, then missing roots), then the
/// machine files.
pub fn list(scope: &Scope) -> RulesOverview {
    let mut files = vec![];
    if let Some(folder) = &scope.folder {
        let reaches = reaches(scope, folder);
        let roots: Vec<PathBuf> = scope.context_files.values().map(|r| root_rel(r)).collect();
        let mut seen: Vec<PathBuf> = vec![];
        let mut cands: Vec<(u8, PathBuf)> = vec![];
        for r in &roots {
            if !seen.contains(r) {
                seen.push(r.clone());
                let exists = inside(folder, r).is_ok_and(|p| p.is_file());
                cands.push((if exists { 0 } else { 2 }, r.clone()));
            }
        }
        let mut others: Vec<PathBuf> = reaches.values().flat_map(|m| m.keys().cloned()).collect();
        others.extend(scope.extras.iter().filter_map(|e| check_rel(e).ok()));
        others.sort();
        for o in others {
            let md = o.extension().is_some_and(|e| e.eq_ignore_ascii_case("md"));
            if md && !seen.contains(&o) && inside(folder, &o).is_ok_and(|p| p.is_file()) {
                seen.push(o.clone());
                cands.push((1, o));
            }
        }
        cands.sort_by_key(|(group, _)| *group);
        for (_, rel) in cands {
            let Ok(abs) = inside(folder, &rel) else { continue };
            let root_of = scope.context_files.iter().filter(|(_, r)| root_rel(r) == rel).map(|(p, _)| p.clone()).collect();
            files.push(rule_file(rel.display().to_string(), &abs, ConfigSource::Project, project_readers(scope, &reaches, &rel), root_of));
        }
    }
    for m in &scope.machine {
        files.push(rule_file(m.display.clone(), &m.abs, ConfigSource::Machine, machine_readers(scope, m), vec![m.provider.clone()]));
    }
    RulesOverview {
        project: scope.project.clone(),
        folder: scope.folder.as_ref().map(|f| f.display().to_string()).unwrap_or_default(),
        files,
        max_bytes: MAX_BYTES as u64,
    }
}

enum Target {
    Project(PathBuf),
    Machine(usize),
}

fn target(scope: &Scope, path: &str) -> Result<Target> {
    let p = path.trim();
    if let Some(i) = scope.machine.iter().position(|m| m.display == p || m.abs == Path::new(p) || m.abs == crate::login::expand_home(p)) {
        return Ok(Target::Machine(i));
    }
    if scope.folder.is_none() {
        let known = scope.machine.iter().map(|m| m.display.as_str()).collect::<Vec<_>>().join(", ");
        bail!("`{p}` is not a machine rule file ({known}); give `project` for a file in a project folder");
    }
    Ok(Target::Project(check_rel(p)?))
}

fn write_file(path: &Path, body: &str) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    std::fs::write(path, body).with_context(|| format!("writing {}", path.display()))
}

/// Creates or replaces a rule file; returns the project-relative path when it is a project file that is not a root
/// file (MAP keeps it in the listing).
pub fn write(scope: &Scope, path: &str, body: &str) -> Result<Option<String>> {
    if body.len() > MAX_BYTES {
        bail!("a rule file can be at most {} KB", MAX_BYTES / 1024);
    }
    if body.contains('\0') {
        bail!("rule files are text");
    }
    match target(scope, path)? {
        Target::Machine(i) => {
            write_file(&scope.machine[i].abs, body)?;
            Ok(None)
        }
        Target::Project(rel) => {
            let folder = scope.folder.as_deref().context("no project folder")?;
            write_file(&inside(folder, &rel)?, body)?;
            let is_root = scope.context_files.values().any(|r| root_rel(r) == rel);
            Ok((!is_root).then(|| rel.display().to_string()))
        }
    }
}

/// Deletes a rule file, and the whole `@…` lines that import it from the import providers' root files; returns the
/// root files edited.
pub fn delete(scope: &Scope, path: &str) -> Result<Vec<String>> {
    match target(scope, path)? {
        Target::Machine(i) => {
            let m = &scope.machine[i];
            std::fs::remove_file(&m.abs).with_context(|| format!("deleting {}", m.display))?;
            Ok(vec![])
        }
        Target::Project(rel) => {
            let folder = scope.folder.as_deref().context("no project folder")?;
            let real = inside(folder, &rel)?;
            if !real.is_file() {
                bail!("`{}` does not exist", rel.display());
            }
            std::fs::remove_file(&real).with_context(|| format!("deleting {}", rel.display()))?;
            let mut edited = vec![];
            for (provider, root) in &scope.context_files {
                let root = root_rel(root);
                if !IMPORT_PROVIDERS.contains(&provider.as_str()) || root == rel || edited.contains(&root.display().to_string()) {
                    continue;
                }
                let Ok(root_abs) = inside(folder, &root) else { continue };
                let Ok(text) = std::fs::read_to_string(&root_abs) else { continue };
                let (kept, removed) = remove_import_lines(&text, &parent(&root), &rel);
                if removed > 0 {
                    write_file(&root_abs, &kept)?;
                    edited.push(root.display().to_string());
                }
            }
            Ok(edited)
        }
    }
}

/// Makes exactly `readers` read the file, by adding or removing `@<path>` lines in root files; returns what changed
/// (`+claude via CLAUDE.md`). A change to a fixed reader is refused before anything is written.
pub fn set_readers(scope: &Scope, path: &str, readers: &[String]) -> Result<Vec<String>> {
    for r in readers {
        if !scope.context_files.contains_key(r) {
            bail!("`{r}` has no context file here (known: {})", scope.context_files.keys().cloned().collect::<Vec<_>>().join(", "));
        }
    }
    let rel = match target(scope, path)? {
        Target::Machine(i) => {
            let m = &scope.machine[i];
            for o in machine_readers(scope, m) {
                if o.reads != readers.contains(&o.provider) {
                    bail!("{}: {}", o.provider, o.note);
                }
            }
            return Ok(vec![]);
        }
        Target::Project(rel) => rel,
    };
    let folder = scope.folder.as_deref().context("no project folder")?;
    let real = inside(folder, &rel)?;
    if !real.is_file() {
        bail!("`{}` does not exist yet; save it first", rel.display());
    }
    let reaches = reaches(scope, folder);
    let mut changes = vec![];
    for o in project_readers(scope, &reaches, &rel) {
        let want = readers.contains(&o.provider);
        if want == o.reads {
            continue;
        }
        if o.fixed {
            bail!("{}: {}", o.provider, o.note);
        }
        changes.push((o.provider, want));
    }
    let mut done = vec![];
    for (provider, want) in changes {
        let root = root_rel(&scope.context_files[&provider]);
        let root_abs = inside(folder, &root)?;
        let text = std::fs::read_to_string(&root_abs).unwrap_or_default();
        let base = parent(&root);
        let new = if want { append_import(&text, &import_path(&base, &rel)) } else { remove_import_lines(&text, &base, &rel).0 };
        write_file(&root_abs, &new)?;
        done.push(format!("{}{provider} via {}", if want { "+" } else { "-" }, root.display()));
    }
    Ok(done)
}

// ---- entry points for the API

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.context("rules task panicked")?
}

pub async fn overview(hub: &Hub, project: Option<&str>) -> Result<RulesOverview> {
    let scope = scope(hub, project).await?;
    blocking(move || Ok(list(&scope))).await
}

fn place(project: Option<&str>) -> String {
    project.map_or("the machine".to_string(), |p| format!("project {p}"))
}

pub async fn put(hub: &Hub, project: Option<&str>, path: String, body: String) -> Result<RulesOverview> {
    let s = scope(hub, project).await?;
    let (s2, p2) = (s.clone(), path.clone());
    let extra = blocking(move || write(&s2, &p2, &body)).await?;
    if let (Some(slug), Some(extra)) = (project, extra)
        && !s.extras.contains(&extra)
    {
        let mut extras = s.extras.clone();
        extras.push(extra);
        hub.store.set_runtime_setting(&extras_key(slug), Some(&serde_json::to_string(&extras)?)).await?;
    }
    hub.event(Level::Info, "config.rules", None, None, format!("Rule file {} saved in {}", path.trim(), place(project))).await;
    overview(hub, project).await
}

pub async fn remove(hub: &Hub, project: Option<&str>, path: String) -> Result<RulesOverview> {
    let s = scope(hub, project).await?;
    let (s2, p2) = (s.clone(), path.clone());
    let edited = blocking(move || delete(&s2, &p2)).await?;
    if let Some(slug) = project
        && let Ok(rel) = check_rel(&path)
    {
        let rel = rel.display().to_string();
        if s.extras.contains(&rel) {
            let extras: Vec<&String> = s.extras.iter().filter(|e| **e != rel).collect();
            hub.store.set_runtime_setting(&extras_key(slug), Some(&serde_json::to_string(&extras)?)).await?;
        }
    }
    let also = if edited.is_empty() { String::new() } else { format!("; its import removed from {}", edited.join(", ")) };
    hub.event(Level::Warning, "config.rules", None, None, format!("Rule file {} deleted in {}{also}", path.trim(), place(project))).await;
    overview(hub, project).await
}

pub async fn readers(hub: &Hub, project: Option<&str>, path: String, readers: Vec<String>) -> Result<RulesOverview> {
    let s = scope(hub, project).await?;
    let p2 = path.clone();
    let done = blocking(move || set_readers(&s, &p2, &readers)).await?;
    if !done.is_empty() {
        hub.event(Level::Info, "config.rules", None, None, format!("Readers of {} in {}: {}", path.trim(), place(project), done.join(", "))).await;
    }
    overview(hub, project).await
}

#[cfg(test)]
mod tests;
