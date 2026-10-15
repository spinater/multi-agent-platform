//! A project's knowledge: the Markdown and text docs in its folder (knowledge cards with their index, requirements,
//! task sheets, rulebooks) plus what MAP remembers itself (finished tasks' outcomes, approved plans), indexed in
//! SQLite FTS5 with the trigram tokenizer so a substring search works for Thai as well as English.
//!
//! The health checks follow the team-process conventions (from a real project's `scripts/check-knowledge.sh`): a
//! card lists the files it summarises in a `sources:` frontmatter list and is stale when one of them was committed
//! after it; cards have a line cap; the index mentions every card; rulebooks loaded on every request have a byte
//! budget; scratch folders must not grow huge or hold copies of the repo. Every finding carries a task prompt that
//! fixes it. Agents search with their project token (`$MAP_API_URL/projects/<slug>/knowledge/search`), and the
//! searches are logged so the owner sees what they looked up.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use regex::Regex;
use sqlx::Row;

use crate::hub::Hub;
use crate::model::{
    KindCount, KnowledgeCheck, KnowledgeDoc, KnowledgeDocBody, KnowledgeHit, KnowledgeIssue, KnowledgeKind, KnowledgeOverview, KnowledgeQuery,
    KnowledgeSearch, KnowledgeSettings, Level, PlanStatus, Project, KnowledgeStats, KnowledgeSuggestion, RulebookSize, ScratchInfo, SuggestVerdict, TaskStatus,
};

/// An index younger than this is used as it is.
const FRESH: Duration = Duration::from_secs(120);
const MAX_FILES: usize = 4000;
const MAX_BODY: usize = 1024 * 1024;
const TEXT: &[&str] = &["md", "markdown", "mdx", "txt"];
const SKIP_DIRS: &[&str] = &[".git", "node_modules", "target", ".next", "dist", "build", ".venv", "venv", "__pycache__"];

/// A document found on disk or in MAP's memory, with its text.
struct Scanned {
    doc: KnowledgeDoc,
    body: String,
}

fn kind_str(k: KnowledgeKind) -> &'static str {
    match k {
        KnowledgeKind::Card => "card",
        KnowledgeKind::Requirement => "requirement",
        KnowledgeKind::Task => "task",
        KnowledgeKind::Rulebook => "rulebook",
        KnowledgeKind::Doc => "doc",
        KnowledgeKind::Memory => "memory",
    }
}

pub fn parse_kind(s: &str) -> Option<KnowledgeKind> {
    serde_json::from_value(serde_json::json!(s)).ok()
}

/// A path inside the project, without `..` or a root.
fn relative_ok(path: &str) -> bool {
    !path.is_empty() && Path::new(path).components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

// ---- frontmatter, titles, links

/// The `sources:` list of a `---` frontmatter block: `sources: [a, b]`, `sources: a, b`, or `- item` lines below it.
pub fn sources(body: &str) -> Vec<String> {
    let Some(rest) = body.strip_prefix("---\n").or_else(|| body.strip_prefix("---\r\n")) else { return vec![] };
    let Some(end) = rest.find("\n---") else { return vec![] };
    let mut out = vec![];
    let mut in_list = false;
    for line in rest[..end].lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("sources:") {
            let value = value.trim().trim_start_matches('[').trim_end_matches(']');
            out.extend(value.split(',').map(clean).filter(|s| !s.is_empty()));
            in_list = true;
        } else if in_list && let Some(item) = trimmed.strip_prefix("- ") {
            out.push(clean(item));
        } else if in_list && !trimmed.is_empty() {
            in_list = false;
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

fn clean(s: &str) -> String {
    // `path — note` or `path # note`: the path is the first word.
    let s = s.trim().trim_matches(|c| c == '"' || c == '\'' || c == '`');
    s.split(|c: char| c.is_whitespace()).next().unwrap_or("").trim_matches(|c| c == '"' || c == '\'' || c == '`').to_string()
}

fn title_of(body: &str, path: &str) -> String {
    let after_front = match body.strip_prefix("---\n").and_then(|r| r.find("\n---").map(|e| &r[e + 4..])) {
        Some(rest) => rest,
        None => body,
    };
    after_front
        .lines()
        .find_map(|l| l.strip_prefix("# ").map(|t| t.trim().to_string()))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| Path::new(path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| path.to_string()))
}

static LINK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\]\(([^)\s]+?)(?:\s+\x22[^)]*\x22)?\)").unwrap());

/// Relative link targets in Markdown text (no URLs, mail links or bare anchors), without their `#anchor`.
pub(crate) fn links(body: &str) -> Vec<String> {
    let mut in_code = false;
    let mut out = vec![];
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }
        for cap in LINK.captures_iter(line) {
            let target = cap[1].split('#').next().unwrap_or("").trim_start_matches('<').trim_end_matches('>');
            if target.is_empty() || target.contains("://") || target.starts_with("mailto:") || target.starts_with('/') {
                continue;
            }
            out.push(target.to_string());
        }
    }
    out
}

/// `a/b/../c` → `a/c`; `None` when it climbs out of the project.
pub(crate) fn normalize(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Normal(p) => out.push(p),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

// ---- scanning

fn classify(path: &str, s: &KnowledgeSettings) -> KnowledgeKind {
    let card_dir = Path::new(&s.index).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let lower = path.to_lowercase();
    if s.rulebooks.iter().any(|r| r == path) {
        KnowledgeKind::Rulebook
    } else if !card_dir.is_empty() && (path == s.index || path.starts_with(&format!("{card_dir}/"))) {
        KnowledgeKind::Card
    } else if lower.starts_with("tasks/") {
        KnowledgeKind::Task
    } else if ["requirement", "customer", "spec", "prd"].iter().any(|w| lower.contains(w)) {
        KnowledgeKind::Requirement
    } else {
        KnowledgeKind::Doc
    }
}

/// The text files under `settings.dirs` and the rulebooks, skipping build folders, scratch folders and anything a
/// symlink points at.
fn scan_files(root: &Path, s: &KnowledgeSettings) -> Vec<Scanned> {
    let scratch: HashSet<PathBuf> = s.scratch.iter().filter(|p| relative_ok(p)).map(|p| root.join(p)).collect();
    let mut found: BTreeMap<String, PathBuf> = BTreeMap::new();
    for r in s.rulebooks.iter().filter(|r| relative_ok(r)) {
        let p = root.join(r);
        if p.is_file() {
            found.insert(r.clone(), p);
        }
    }
    let mut stack: Vec<PathBuf> = s.dirs.iter().filter(|d| relative_ok(d)).map(|d| root.join(d)).filter(|d| d.is_dir()).collect();
    while let Some(dir) = stack.pop() {
        if found.len() >= MAX_FILES {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if t.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !scratch.contains(&path) {
                    stack.push(path);
                }
            } else if t.is_file()
                && path.extension().is_some_and(|x| TEXT.contains(&x.to_string_lossy().to_lowercase().as_str()))
                && let Ok(rel) = path.strip_prefix(root)
            {
                found.insert(rel.to_string_lossy().to_string(), path.clone());
            }
        }
    }
    let index_body = std::fs::read_to_string(root.join(&s.index)).unwrap_or_default();
    found
        .into_iter()
        .filter_map(|(rel, abs)| {
            let meta = std::fs::metadata(&abs).ok()?;
            let mut body = std::fs::read(&abs).ok()?;
            body.truncate(MAX_BODY);
            let body = String::from_utf8_lossy(&body).to_string();
            let kind = classify(&rel, s);
            let name = Path::new(&rel).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let in_index = kind == KnowledgeKind::Card && (rel == s.index || index_body.contains(&name));
            let doc = KnowledgeDoc {
                title: title_of(&body, &rel),
                lines: body.lines().count() as u32,
                bytes: meta.len(),
                modified: meta.modified().ok().map(DateTime::<Utc>::from),
                sources: if kind == KnowledgeKind::Card { sources(&body) } else { vec![] },
                in_index,
                kind,
                path: rel,
            };
            Some(Scanned { doc, body })
        })
        .collect()
}

/// What MAP remembers of the project: each finished or failed task (what was asked, what came of it, what went wrong
/// on the way) and each approved plan.
async fn memory(hub: &Hub, slug: &str) -> Vec<Scanned> {
    let mut out = vec![];
    for t in hub.store.tasks().await.unwrap_or_default().into_iter().filter(|t| t.project.as_deref() == Some(slug)) {
        if !matches!(t.status, TaskStatus::Done | TaskStatus::Failed) {
            continue;
        }
        let mut body = format!("# Task #{}: {}\n\nstatus: {} · attempts: {}", t.id, t.title, t.status.as_str(), t.attempts);
        if let Some(p) = &t.provider {
            body.push_str(&format!(" · provider: {p}"));
        }
        if let Some(g) = &t.git {
            body.push_str(&format!("\nbranch: {} from {}", g.branch, g.base));
            if let Some(m) = &g.merged {
                body.push_str(&format!(" · merged as {}", &m[..m.len().min(10)]));
            }
            if let Some(u) = &g.pr_url {
                body.push_str(&format!(" · pull request {u}"));
            }
        }
        body.push_str(&format!("\nfinished: {}\n\n## Asked\n\n{}\n\n## Outcome\n\n{}\n", t.updated_at.format("%Y-%m-%d"), cut(&t.prompt, 2000), cut(t.result.as_deref().unwrap_or("(no summary)"), 3000)));
        let trail: Vec<String> = hub
            .store
            .decision_events(t.id, 300)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|e| ["verify.fail", "audit.fail", "land.conflict", "land.reverify_fail", "land.blocked", "watch.failed", "watch.timeout", "review.send_back"].iter().any(|k| e.kind.ends_with(k)))
            .take(12)
            .map(|e| format!("- {} {}: {}", e.at.format("%Y-%m-%d"), e.kind.trim_start_matches("decision."), cut(&e.message, 300)))
            .collect();
        if !trail.is_empty() {
            body.push_str(&format!("\n## What went wrong on the way\n\n{}\n", trail.join("\n")));
        }
        out.push(memory_doc(format!("map://tasks/{}", t.id), format!("Task #{}: {}", t.id, t.title), body, t.updated_at));
    }
    for p in hub.store.plans(Some(slug), 500).await.unwrap_or_default().into_iter().filter(|p| p.status == PlanStatus::Approved) {
        let tasks: Vec<String> = p.tasks.iter().map(|t| format!("- {}{}", t.title, t.task_id.map(|id| format!(" (task #{id})")).unwrap_or_default())).collect();
        let body = format!("# Plan #{}: {}\n\napproved plan, revision {}\n\n{}\n\n## Tasks\n\n{}\n", p.id, p.title, p.revision, cut(&p.summary, 4000), tasks.join("\n"));
        out.push(memory_doc(format!("map://plans/{}", p.id), format!("Plan #{}: {}", p.id, p.title), body, p.created_at));
    }
    out
}

fn memory_doc(path: String, title: String, body: String, at: DateTime<Utc>) -> Scanned {
    let doc = KnowledgeDoc { path, kind: KnowledgeKind::Memory, title, lines: body.lines().count() as u32, bytes: body.len() as u64, modified: Some(at), sources: vec![], in_index: false };
    Scanned { doc, body }
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { format!("{}…", s.chars().take(max).collect::<String>()) }
}

// ---- the index

/// One reindex at a time per project.
fn index_lock(slug: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Default::default);
    Arc::clone(LOCKS.lock().unwrap().entry(slug.to_string()).or_default())
}

async fn indexed_at(hub: &Hub, slug: &str) -> Option<DateTime<Utc>> {
    sqlx::query_scalar("SELECT indexed_at FROM knowledge_meta WHERE project = ?").bind(slug).fetch_optional(&hub.store.pool).await.ok().flatten()
}

/// Reindexes the project when its index is older than [`FRESH`] (or always, with `force`).
pub async fn ensure_index(hub: &Hub, project: &Project, force: bool) -> Result<()> {
    let lock = index_lock(&project.slug);
    let _one = lock.lock().await;
    if !force && indexed_at(hub, &project.slug).await.is_some_and(|at| Utc::now() - at < chrono::Duration::from_std(FRESH).unwrap()) {
        return Ok(());
    }
    let root = hub.workspace_of(project);
    let settings = project.settings.knowledge.clone();
    let mut docs = tokio::task::spawn_blocking(move || scan_files(&root, &settings)).await?;
    docs.extend(memory(hub, &project.slug).await);
    let mut tx = hub.store.pool.begin().await?;
    sqlx::query("DELETE FROM knowledge_docs WHERE project = ?").bind(&project.slug).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM knowledge_fts WHERE project = ?").bind(&project.slug).execute(&mut *tx).await?;
    for s in &docs {
        let d = &s.doc;
        sqlx::query("INSERT INTO knowledge_docs (project, path, kind, title, lines, bytes, modified, sources, in_index) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(&project.slug)
            .bind(&d.path)
            .bind(kind_str(d.kind))
            .bind(&d.title)
            .bind(d.lines as i64)
            .bind(d.bytes as i64)
            .bind(d.modified)
            .bind(serde_json::to_string(&d.sources)?)
            .bind(d.in_index)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO knowledge_fts (project, path, kind, title, body) VALUES (?, ?, ?, ?, ?)")
            .bind(&project.slug)
            .bind(&d.path)
            .bind(kind_str(d.kind))
            .bind(&d.title)
            .bind(&s.body)
            .execute(&mut *tx)
            .await?;
    }
    sqlx::query("INSERT INTO knowledge_meta (project, indexed_at) VALUES (?, ?) ON CONFLICT(project) DO UPDATE SET indexed_at = excluded.indexed_at")
        .bind(&project.slug)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn docs(hub: &Hub, slug: &str) -> Result<Vec<KnowledgeDoc>> {
    let rows = sqlx::query("SELECT * FROM knowledge_docs WHERE project = ? ORDER BY path").bind(slug).fetch_all(&hub.store.pool).await?;
    rows.iter()
        .map(|r| {
            Ok(KnowledgeDoc {
                path: r.get("path"),
                kind: parse_kind(r.get("kind")).unwrap_or(KnowledgeKind::Doc),
                title: r.get("title"),
                lines: r.get::<i64, _>("lines") as u32,
                bytes: r.get::<i64, _>("bytes") as u64,
                modified: r.get("modified"),
                sources: serde_json::from_str(r.get("sources"))?,
                in_index: r.get("in_index"),
            })
        })
        .collect()
}

/// Forgets a deleted project's index and searches.
pub async fn forget(hub: &Hub, slug: &str) {
    for sql in [
        "DELETE FROM knowledge_docs WHERE project = ?",
        "DELETE FROM knowledge_fts WHERE project = ?",
        "DELETE FROM knowledge_meta WHERE project = ?",
        "DELETE FROM knowledge_queries WHERE project = ?",
    ] {
        let _ = sqlx::query(sql).bind(slug).execute(&hub.store.pool).await;
    }
}

// ---- the overview and its health checks

pub async fn overview(hub: &Hub, project: &Project, force: bool) -> Result<KnowledgeOverview> {
    ensure_index(hub, project, force).await?;
    let root = hub.workspace_of(project);
    let s = project.settings.knowledge.clone();
    let docs = docs(hub, &project.slug).await?;
    let mut counts: BTreeMap<&'static str, (KnowledgeKind, u32)> = BTreeMap::new();
    for d in &docs {
        counts.entry(kind_str(d.kind)).or_insert((d.kind, 0)).1 += 1;
    }
    let commit_times = commit_times(hub, &root).await;
    let (issues, rulebooks, scratch) = {
        let (root, s, docs) = (root.clone(), s.clone(), docs.clone());
        tokio::task::spawn_blocking(move || health(&root, &s, &docs, &commit_times)).await?
    };
    let index_path = root.join(&s.index).is_file().then(|| s.index.clone());
    let suggestion = suggest(hub, project, &root, &s).await;
    Ok(KnowledgeOverview {
        slug: project.slug.clone(),
        root: root.display().to_string(),
        settings: s,
        index_path,
        indexed_at: indexed_at(hub, &project.slug).await,
        counts: counts.into_values().map(|(kind, count)| KindCount { kind, count }).collect(),
        docs,
        issues,
        rulebooks,
        scratch,
        recent_queries: recent_queries(hub, &project.slug, 50).await,
        suggestion,
    })
}

// ---- should the project have knowledge yet?

/// A project this big or this busy gets the suggestion to set up its knowledge.
pub const SUGGEST_FILES: u32 = 30;
pub const SUGGEST_LINES: u64 = 3000;
pub const SUGGEST_DONE: u32 = 3;
pub const SUGGEST_DOCS: u32 = 10;
const CODE: &[&str] = &[
    "rs", "ts", "tsx", "js", "jsx", "mjs", "py", "go", "java", "kt", "swift", "rb", "php", "cs", "c", "h", "cpp", "hpp", "vue", "svelte", "dart", "scala", "ex",
    "exs", "sql", "sh", "css", "scss", "html", "prisma", "proto",
];
const SCAN_FILES: u32 = 20_000;

/// Counts source files and lines under `root` (build, dependency and scratch folders skipped).
fn source_stats(root: &Path, s: &KnowledgeSettings) -> (u32, u64) {
    let scratch: HashSet<PathBuf> = s.scratch.iter().filter(|p| relative_ok(p)).map(|p| root.join(p)).collect();
    let (mut files, mut lines) = (0u32, 0u64);
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let path = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if t.is_dir() {
                if !SKIP_DIRS.contains(&name.as_str()) && !name.starts_with('.') && !scratch.contains(&path) {
                    stack.push(path);
                }
            } else if t.is_file() && path.extension().is_some_and(|x| CODE.contains(&x.to_string_lossy().to_lowercase().as_str())) {
                files += 1;
                if e.metadata().is_ok_and(|m| m.len() < 2_000_000)
                    && let Ok(text) = std::fs::read(&path)
                {
                    lines += text.iter().filter(|b| **b == b'\n').count() as u64;
                }
                if files >= SCAN_FILES {
                    return (files, lines);
                }
            }
        }
    }
    (files, lines)
}

fn runtime_key(slug: &str, what: &str) -> String {
    format!("knowledge.{slug}.{what}")
}

/// What accepting the suggestion does, in order (the steps of `bootstrap_prompt`).
pub fn plan(s: &KnowledgeSettings) -> Vec<String> {
    let dir = Path::new(&s.index).parent().map(|p| p.display().to_string()).unwrap_or_default();
    vec![
        "MAP queues one task for an agent in this project (you see it on the board; nothing is written before it runs).".into(),
        "The agent surveys the code, the docs, the requirements and the past tasks in MAP's memory.".into(),
        format!("It writes one card per area in {dir}/domain/ (business rules) and {dir}/ops/ (running, deploying, debugging): 40–120 lines, at most {}, each with `sources:` listing the files it summarises.", s.card_line_cap),
        format!("It writes {} with the conventions and one row per card.", s.index),
        format!("It points CLAUDE.md / AGENTS.md at the index (read it first) and moves long sections out of them into cards, keeping each under {} KB.", s.rulebook_byte_warn / 1000),
        "One commit, verified like any task. From then on every agent searches the knowledge before it starts, and this screen checks it for stale cards.".into(),
    ]
}

/// The setup task's prompt: MAP's knowledge practice (the team-process conventions).
pub fn bootstrap_prompt(s: &KnowledgeSettings) -> String {
    let dir = Path::new(&s.index).parent().map(|p| p.display().to_string()).unwrap_or_default();
    format!(
        "Set up this project's knowledge, following MAP's knowledge practice.\n\n\
         1. Survey: read the README, `docs/`, `.docs/`, `tasks/`, the rulebooks ({rulebooks}) and the code's top-level layout. Search MAP's memory of past tasks \
         and their outcomes: `curl -s -G -H \"Authorization: Bearer $MAP_PROJECT_TOKEN\" \"$MAP_KNOWLEDGE_URL/search\" --data-urlencode \"q=<area>\" --data-urlencode kind=memory`.\n\
         2. Cards: write one card per area an agent would need to understand before changing it. Business rules and domain concepts go in `{dir}/domain/`, \
         running, deploying, configuring and debugging in `{dir}/ops/`. A card is a curated map, 40–120 lines and never more than {cap}: what the area does, \
         where it lives, the rules and the traps (from past tasks' failures too). It points at code and docs; it never restates a schema or copies code. \
         Every card starts with\n---\nsources:\n  - <files or folders it summarises>\n---\n# <Title>\n\
         3. Index: write `{index}` with a short Conventions section (the rules above, plus: a commit that changes a card's sources updates the card in the same \
         commit; a card that outgrows the cap is split, never silently cut) and a `## Cards` table with one row per card: `| [Title](domain/file.md) | one-line scope |`.\n\
         4. Rulebooks: add near the top of each rulebook that exists a line telling agents to read `{index}` first and to search `$MAP_KNOWLEDGE_URL` before \
         grepping. If a rulebook is over {warn} bytes, move its long reference sections into cards and leave a one-line pointer; keep the rules themselves.\n\
         5. Do not touch code, do not copy secrets or credentials into any card, and commit everything in one commit.",
        rulebooks = s.rulebooks.join(", "),
        index = s.index,
        cap = s.card_line_cap,
        warn = s.rulebook_byte_warn,
    )
}

pub const BOOTSTRAP_TITLE: &str = "Set up the project's knowledge";

async fn suggest(hub: &Hub, project: &Project, root: &Path, s: &KnowledgeSettings) -> KnowledgeSuggestion {
    let docs = docs(hub, &project.slug).await.unwrap_or_default();
    let cards = docs.iter().filter(|d| d.kind == KnowledgeKind::Card && d.path != s.index).count() as u32;
    let doc_count = docs.iter().filter(|d| matches!(d.kind, KnowledgeKind::Doc | KnowledgeKind::Requirement | KnowledgeKind::Task)).count() as u32;
    let rulebook_bytes = s.rulebooks.iter().filter(|r| relative_ok(r)).filter_map(|r| std::fs::metadata(root.join(r)).ok()).map(|m| m.len()).max().unwrap_or(0);
    let tasks = hub.store.tasks().await.unwrap_or_default();
    let done_tasks = tasks.iter().filter(|t| t.project.as_deref() == Some(project.slug.as_str()) && t.status == TaskStatus::Done).count() as u32;
    let (source_files, source_lines) = {
        let (root, s) = (root.to_path_buf(), s.clone());
        tokio::task::spawn_blocking(move || source_stats(&root, &s)).await.unwrap_or((0, 0))
    };
    let stats = KnowledgeStats { source_files, source_lines, done_tasks, docs: doc_count, cards, rulebook_bytes };
    let task_id = hub.store.runtime_setting(&runtime_key(&project.slug, "bootstrap_task")).await.ok().flatten().and_then(|v| v.parse::<i64>().ok());
    let task = match task_id {
        Some(id) => tasks.iter().find(|t| t.id == id),
        None => None,
    };
    let dismissed = hub.store.runtime_setting(&runtime_key(&project.slug, "dismissed")).await.ok().flatten().is_some();
    let plan = plan(s);
    let has_index = root.join(&s.index).is_file();
    let mk = |verdict, reasons: Vec<String>| KnowledgeSuggestion { verdict, reasons, stats: stats.clone(), plan: plan.clone(), task: task.map(|t| t.id) };

    if has_index {
        return mk(SuggestVerdict::InUse, vec![format!("{} card(s) in {}; agents search them before they start.", cards, s.index)]);
    }
    if let Some(t) = task.filter(|t| !t.status.is_terminal() || t.status == TaskStatus::Done) {
        return mk(SuggestVerdict::InProgress, vec![format!("Task #{} is setting it up ({}).", t.id, t.status.as_str())]);
    }
    let mut why = vec![];
    if source_files >= SUGGEST_FILES {
        why.push(format!("{source_files} source files ({source_lines} lines): more than an agent can read before each task."));
    } else if source_lines >= SUGGEST_LINES {
        why.push(format!("{source_lines} lines of source: more than an agent can read before each task."));
    }
    if done_tasks >= SUGGEST_DONE {
        why.push(format!("{done_tasks} tasks done: what they learned is only in MAP's memory, not in the repo where agents look first."));
    }
    if doc_count >= SUGGEST_DOCS {
        why.push(format!("{doc_count} docs with no index: agents grep them instead of reading a map."));
    }
    if rulebook_bytes > s.rulebook_byte_warn {
        why.push(format!("The largest rulebook is {} KB (budget {} KB) and is sent with every request: cards can hold its reference parts.", rulebook_bytes / 1000, s.rulebook_byte_cap / 1000));
    }
    if why.is_empty() {
        return mk(
            SuggestVerdict::NotYet,
            vec![format!(
                "Still small: {source_files} source files, {source_lines} lines, {done_tasks} tasks done. Cards would cost more than they save; MAP suggests it again from {SUGGEST_FILES} files, {SUGGEST_LINES} lines or {SUGGEST_DONE} tasks done."
            )],
        );
    }
    if dismissed {
        return mk(SuggestVerdict::Dismissed, why);
    }
    mk(SuggestVerdict::Recommended, why)
}

/// Accepting the suggestion: queues the setup task (once) and returns it.
pub async fn accept(hub: &Hub, project: &Project) -> Result<crate::model::Task> {
    let key = runtime_key(&project.slug, "bootstrap_task");
    if let Some(id) = hub.store.runtime_setting(&key).await?.and_then(|v| v.parse::<i64>().ok())
        && let Some(t) = hub.store.task(id).await?
        && !matches!(t.status, TaskStatus::Failed | TaskStatus::Cancelled)
    {
        return Ok(t);
    }
    let task = hub
        .store
        .create_task(crate::store::NewTask {
            title: BOOTSTRAP_TITLE.into(),
            prompt: bootstrap_prompt(&project.settings.knowledge),
            depends_on: vec![],
            provider: None,
            model: None,
            project: Some(project.slug.clone()),
            workflow: None,
        })
        .await?;
    hub.store.set_runtime_setting(&key, Some(&task.id.to_string())).await?;
    hub.store.set_runtime_setting(&runtime_key(&project.slug, "dismissed"), None).await?;
    hub.announce_task(&task);
    hub.event(Level::Info, "task.created", None, Some(task.id), format!("Task #{} created: {} (knowledge suggestion accepted)", task.id, task.title)).await;
    Ok(task)
}

/// "Not now" (or undo it).
pub async fn dismiss(hub: &Hub, slug: &str, dismissed: bool) -> Result<()> {
    hub.store.set_runtime_setting(&runtime_key(slug, "dismissed"), dismissed.then_some("1")).await
}

/// Each path's latest commit time, from one `git log` over the recent history (newest first, so the first time a
/// path appears is its latest).
async fn commit_times(hub: &Hub, root: &Path) -> HashMap<String, i64> {
    let git = crate::worktrees::Git::new(hub, root);
    let Ok(run) = git.run(&["log", "-n", "5000", "--format=@%ct", "--name-only", "--no-renames"], Duration::from_secs(20)).await else { return HashMap::new() };
    if !run.ok {
        return HashMap::new();
    }
    let mut out = HashMap::new();
    let mut at = 0;
    for line in run.output.lines() {
        if let Some(t) = line.strip_prefix('@') {
            at = t.parse().unwrap_or(0);
        } else if !line.is_empty() {
            out.entry(line.to_string()).or_insert(at);
        }
    }
    out
}

/// The latest commit of `path`, or of anything under it for a folder.
fn latest(times: &HashMap<String, i64>, path: &str) -> Option<i64> {
    if let Some(t) = times.get(path) {
        return Some(*t);
    }
    let prefix = format!("{}/", path.trim_end_matches('/'));
    times.iter().filter(|(p, _)| p.starts_with(&prefix)).map(|(_, t)| *t).max()
}

fn issue(level: Level, check: KnowledgeCheck, path: &str, detail: String, fix: Option<(String, String)>) -> KnowledgeIssue {
    let (fix_title, fix_prompt) = fix.map_or((None, None), |(t, p)| (Some(t), Some(p)));
    KnowledgeIssue { level, check, path: path.to_string(), detail, fix_title, fix_prompt }
}

fn day(t: i64) -> String {
    DateTime::<Utc>::from_timestamp(t, 0).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default()
}

fn health(root: &Path, s: &KnowledgeSettings, docs: &[KnowledgeDoc], times: &HashMap<String, i64>) -> (Vec<KnowledgeIssue>, Vec<RulebookSize>, Vec<ScratchInfo>) {
    let mut out = vec![];
    let cards: Vec<&KnowledgeDoc> = docs.iter().filter(|d| d.kind == KnowledgeKind::Card && d.path != s.index).collect();
    let conventions = format!("Follow the card conventions in `{}` (frontmatter `sources:` lists the files a card summarises; at most {} lines).", s.index, s.card_line_cap);

    if !root.join(&s.index).is_file() {
        let fix = ("Set up the knowledge index".to_string(), bootstrap_prompt(s));
        out.push(issue(Level::Info, KnowledgeCheck::NoIndex, &s.index, format!("No knowledge index at {}: agents have no map of the project's knowledge.", s.index), Some(fix)));
    }

    let mut stale = vec![];
    for c in &cards {
        let card_at = latest(times, &c.path);
        let mut newer = vec![];
        for src in &c.sources {
            let exists = relative_ok(src) && root.join(src).exists();
            if !exists {
                out.push(issue(
                    Level::Error,
                    KnowledgeCheck::MissingSource,
                    &c.path,
                    format!("lists source `{src}`, which does not exist"),
                    Some((format!("Fix the sources of {}", c.path), format!("The knowledge card `{}` lists the source `{src}`, which does not exist (moved, renamed or deleted). Find where that content lives now and update the card's `sources:` and text, or drop the source if it is gone. {conventions} Commit only the card.", c.path))),
                ));
                continue;
            }
            if let (Some(card), Some(src_at)) = (card_at, latest(times, src))
                && src_at > card
            {
                newer.push(format!("`{src}` ({})", day(src_at)));
            }
        }
        if !newer.is_empty() {
            stale.push(c.path.clone());
            let card = card_at.map(day).unwrap_or_default();
            out.push(issue(
                Level::Warning,
                KnowledgeCheck::Stale,
                &c.path,
                format!("last updated {card}; newer commits in {}", newer.join(", ")),
                Some((
                    format!("Refresh knowledge card {}", c.path),
                    format!("The knowledge card `{}` (last committed {card}) is older than its sources: {}. Read what changed in them since then (`git log --since={card} -p -- <source>`) and update the card so it is accurate again. {conventions} Commit only the card.", c.path, newer.join(", ")),
                )),
            ));
        }
        if c.lines > s.card_line_cap {
            out.push(issue(Level::Error, KnowledgeCheck::OverCap, &c.path, format!("{} lines, cap {}", c.lines, s.card_line_cap), Some((format!("Split knowledge card {}", c.path), format!("The knowledge card `{}` has {} lines, over the cap of {}. Split it into focused cards by topic (never trim history by deleting it), narrow each card's `sources:`, and update `{}` with a row per new card. Do the split as its own commit.", c.path, c.lines, s.card_line_cap, s.index)))));
        } else if c.lines > s.card_line_warn {
            out.push(issue(Level::Warning, KnowledgeCheck::NearCap, &c.path, format!("{} lines, warning at {}, cap {}", c.lines, s.card_line_warn, s.card_line_cap), None));
        }
        if !c.in_index && root.join(&s.index).is_file() {
            out.push(issue(Level::Warning, KnowledgeCheck::NotInIndex, &c.path, format!("not mentioned in {}", s.index), Some((format!("Add {} to the knowledge index", c.path), format!("The knowledge card `{}` is not in `{}`. Add a row for it (link + one-line scope) in the right section. Commit only the index.", c.path, s.index)))));
        }
    }

    // Broken relative links in cards, the index and the rulebooks.
    let mut broken = 0;
    for d in docs.iter().filter(|d| matches!(d.kind, KnowledgeKind::Card | KnowledgeKind::Rulebook)) {
        let Ok(body) = std::fs::read_to_string(root.join(&d.path)) else { continue };
        let base = Path::new(&d.path).parent().unwrap_or(Path::new(""));
        let mut missing: Vec<String> = vec![];
        for target in links(&body) {
            let decoded = target.replace("%20", " ");
            match normalize(&base.join(&decoded)) {
                Some(p) if root.join(&p).exists() => {}
                _ => missing.push(target),
            }
        }
        missing.dedup();
        if !missing.is_empty() && broken < 200 {
            broken += 1;
            out.push(issue(Level::Warning, KnowledgeCheck::BrokenLink, &d.path, format!("links to missing {}", missing.iter().take(5).map(|m| format!("`{m}`")).collect::<Vec<_>>().join(", ")), Some((format!("Fix broken links in {}", d.path), format!("`{}` links to files that do not exist: {}. Point each link at where the content lives now, or remove it. Commit only this file.", d.path, missing.join(", "))))));
        }
    }

    let mut rulebooks = vec![];
    for r in s.rulebooks.iter().filter(|r| relative_ok(r)) {
        let Ok(meta) = std::fs::metadata(root.join(r)) else { continue };
        let bytes = meta.len();
        rulebooks.push(RulebookSize { path: r.clone(), bytes, cap: s.rulebook_byte_cap, warn: s.rulebook_byte_warn });
        if bytes > s.rulebook_byte_warn {
            let level = if bytes > s.rulebook_byte_cap { Level::Error } else { Level::Warning };
            out.push(issue(
                level,
                KnowledgeCheck::RulebookSize,
                r,
                format!("{} KB of a {} KB budget: it is sent with every agent request", bytes / 1000, s.rulebook_byte_cap / 1000),
                Some((format!("Slim down {r}"), format!("`{r}` is {bytes} bytes; its budget is {} (warning at {}). It is loaded into every agent request, so every byte costs tokens on every call. Keep the rules and move history, measurements, rejected options and long explanations into knowledge cards (with a pointer from the rule). Do not change what any rule says. Commit it as one change.", s.rulebook_byte_cap, s.rulebook_byte_warn))),
            ));
        }
    }

    let mut scratch = vec![];
    for sc in s.scratch.iter().filter(|p| relative_ok(p)) {
        let dir = root.join(sc);
        if !dir.is_dir() {
            continue;
        }
        let bytes = crate::worktree_estimate::size_blocking(&dir, Duration::from_secs(4));
        let copies = repo_copies(&dir, root);
        if bytes > s.scratch_byte_warn {
            out.push(issue(Level::Warning, KnowledgeCheck::ScratchSize, sc, format!("{} in the scratch folder", crate::worktree_estimate::human(bytes)), None));
        }
        if !copies.is_empty() {
            out.push(issue(
                Level::Warning,
                KnowledgeCheck::NestedCopy,
                sc,
                format!("{} cop{} of the repo inside: a grep from the repo root finds their stale docs ({})", copies.len(), if copies.len() == 1 { "y" } else { "ies" }, copies.iter().take(4).cloned().collect::<Vec<_>>().join(", ")),
                None,
            ));
        }
        scratch.push(ScratchInfo { path: sc.clone(), bytes, copies });
    }

    if stale.len() > 1 {
        // One task for all of them is cheaper than one per card.
        let list = stale.iter().map(|p| format!("- `{p}`")).collect::<Vec<_>>().join("\n");
        out.insert(0, issue(Level::Warning, KnowledgeCheck::Stale, "(all stale cards)", format!("{} cards are older than their sources", stale.len()), Some(("Refresh all stale knowledge cards".into(), format!("These knowledge cards are older than one of their `sources:` (a source was committed after the card):\n{list}\nFor each, read what changed in its sources since the card's last commit and update the card. {conventions} One commit per card.")))));
    }
    let rank = |l: Level| match l {
        Level::Error => 0,
        Level::Warning => 1,
        _ => 2,
    };
    out.sort_by_key(|i| rank(i.level));
    (out, rulebooks, scratch)
}

/// Folders inside a scratch folder (three levels deep at most) that are copies of the repo: a `.git`, or the
/// project's own rulebook or docs.
fn repo_copies(scratch: &Path, root: &Path) -> Vec<String> {
    let mut out = vec![];
    let mut stack = vec![(scratch.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if out.len() >= 20 {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
            let p = e.path();
            let looks = p.join(".git").exists() || (p.join("CLAUDE.md").is_file() && (p.join(".docs").is_dir() || p.join("tasks").is_dir()));
            if looks {
                out.push(p.strip_prefix(root).unwrap_or(&p).display().to_string());
            } else if depth < 2 && !SKIP_DIRS.contains(&e.file_name().to_string_lossy().as_ref()) {
                stack.push((p, depth + 1));
            }
        }
    }
    out.sort();
    out
}

// ---- search and reading

/// The FTS5 query for `q`: each term of three characters or more as a quoted phrase (the trigram tokenizer needs
/// three), joined by `op`. `None` when no term is long enough.
fn fts_query(q: &str, op: &str) -> Option<String> {
    let terms: Vec<String> = q.split_whitespace().filter(|t| t.chars().count() >= 3).map(|t| format!("\"{}\"", t.replace('"', "\"\""))).collect();
    (!terms.is_empty()).then(|| terms.join(&format!(" {op} ")))
}

pub async fn search(hub: &Hub, project: &Project, q: &str, kind: Option<KnowledgeKind>, limit: u32, by: &str) -> Result<KnowledgeSearch> {
    let started = Instant::now();
    ensure_index(hub, project, false).await?;
    let q = q.trim();
    let limit = limit.clamp(1, 100) as i64;
    let mut hits = vec![];
    if !q.is_empty() {
        // All terms first; when nothing has them all, any of them.
        for op in ["AND", "OR"] {
            hits = match fts_query(q, op) {
                Some(m) => fts(hub, &project.slug, &m, kind, limit).await?,
                None => like(hub, &project.slug, q, kind, limit).await?,
            };
            if !hits.is_empty() || !q.contains(char::is_whitespace) {
                break;
            }
        }
    }
    if !q.is_empty() {
        let _ = sqlx::query("INSERT INTO knowledge_queries (project, at, query, hits, by_whom) VALUES (?, ?, ?, ?, ?)")
            .bind(&project.slug)
            .bind(Utc::now())
            .bind(cut(q, 300))
            .bind(hits.len() as i64)
            .bind(by)
            .execute(&hub.store.pool)
            .await;
        let _ = sqlx::query("DELETE FROM knowledge_queries WHERE project = ? AND id NOT IN (SELECT id FROM knowledge_queries WHERE project = ? ORDER BY id DESC LIMIT 500)")
            .bind(&project.slug)
            .bind(&project.slug)
            .execute(&hub.store.pool)
            .await;
    }
    Ok(KnowledgeSearch { query: q.to_string(), hits, took_ms: started.elapsed().as_millis() as u64 })
}

async fn fts(hub: &Hub, slug: &str, m: &str, kind: Option<KnowledgeKind>, limit: i64) -> Result<Vec<KnowledgeHit>> {
    let rows = sqlx::query(
        "SELECT path, kind, title, snippet(knowledge_fts, 4, '[', ']', '…', 24) AS snip, bm25(knowledge_fts, 0, 0, 0, 4.0, 1.0) AS rank
         FROM knowledge_fts WHERE knowledge_fts MATCH ? AND project = ? AND (? IS NULL OR kind = ?) ORDER BY rank LIMIT ?",
    )
    .bind(m)
    .bind(slug)
    .bind(kind.map(kind_str))
    .bind(kind.map(kind_str))
    .bind(limit)
    .fetch_all(&hub.store.pool)
    .await?;
    Ok(rows
        .iter()
        .map(|r| KnowledgeHit {
            path: r.get("path"),
            kind: parse_kind(r.get("kind")).unwrap_or(KnowledgeKind::Doc),
            title: r.get("title"),
            snippet: r.get("snip"),
            score: -r.get::<f64, _>("rank"),
        })
        .collect())
}

/// For queries of one or two characters, which the trigram index cannot match.
async fn like(hub: &Hub, slug: &str, q: &str, kind: Option<KnowledgeKind>, limit: i64) -> Result<Vec<KnowledgeHit>> {
    let pattern = format!("%{}%", q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let rows = sqlx::query("SELECT path, kind, title, body FROM knowledge_fts WHERE project = ? AND (? IS NULL OR kind = ?) AND (title LIKE ? ESCAPE '\\' OR body LIKE ? ESCAPE '\\') LIMIT ?")
        .bind(slug)
        .bind(kind.map(kind_str))
        .bind(kind.map(kind_str))
        .bind(&pattern)
        .bind(&pattern)
        .bind(limit)
        .fetch_all(&hub.store.pool)
        .await?;
    Ok(rows
        .iter()
        .map(|r| {
            let body: String = r.get("body");
            let snippet = match body.find(q) {
                Some(i) => {
                    let start = body[..i].char_indices().rev().nth(60).map_or(0, |(j, _)| j);
                    let end = body[i + q.len()..].char_indices().nth(60).map_or(body.len(), |(j, _)| i + q.len() + j);
                    format!("{}[{}]{}", &body[start..i], q, &body[i + q.len()..end]).replace('\n', " ")
                }
                None => String::new(),
            };
            KnowledgeHit { path: r.get("path"), kind: parse_kind(r.get("kind")).unwrap_or(KnowledgeKind::Doc), title: r.get("title"), snippet, score: 0.0 }
        })
        .collect())
}

/// One indexed document's text: read fresh from the folder (only paths the index holds), or MAP's own memory.
pub async fn doc(hub: &Hub, project: &Project, path: &str) -> Result<Option<KnowledgeDocBody>> {
    ensure_index(hub, project, false).await?;
    let Some(doc) = docs(hub, &project.slug).await?.into_iter().find(|d| d.path == path) else { return Ok(None) };
    let body = if doc.kind == KnowledgeKind::Memory {
        sqlx::query_scalar("SELECT body FROM knowledge_fts WHERE project = ? AND path = ?").bind(&project.slug).bind(path).fetch_optional(&hub.store.pool).await?.unwrap_or_default()
    } else {
        let file = hub.workspace_of(project).join(path);
        // Never through a symlink out of the project.
        if std::fs::symlink_metadata(&file).map(|m| m.file_type().is_symlink()).unwrap_or(true) {
            return Ok(None);
        }
        let mut bytes = std::fs::read(&file).unwrap_or_default();
        bytes.truncate(MAX_BODY);
        String::from_utf8_lossy(&bytes).to_string()
    };
    Ok(Some(KnowledgeDocBody { doc, body }))
}

pub async fn recent_queries(hub: &Hub, slug: &str, limit: i64) -> Vec<KnowledgeQuery> {
    let rows = sqlx::query("SELECT at, query, hits, by_whom FROM knowledge_queries WHERE project = ? ORDER BY id DESC LIMIT ?").bind(slug).bind(limit).fetch_all(&hub.store.pool).await.unwrap_or_default();
    rows.iter().map(|r| KnowledgeQuery { at: r.get("at"), query: r.get("query"), hits: r.get::<i64, _>("hits") as u32, by: r.get("by_whom") }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_sources_in_every_shape() {
        assert_eq!(sources("---\nsources:\n  - backend/src/a.rs\n  - \"docs/b.md\" — the API\nowner: x\n---\n# A\n"), ["backend/src/a.rs", "docs/b.md"]);
        assert_eq!(sources("---\nsources: [a.md, b/c.rs]\n---\n"), ["a.md", "b/c.rs"]);
        assert_eq!(sources("---\ntitle: x\n---\n"), Vec::<String>::new());
        assert_eq!(sources("# no frontmatter\nsources:\n - a"), Vec::<String>::new());
    }

    #[test]
    fn titles_and_links() {
        assert_eq!(title_of("---\nsources: []\n---\n\n# Billing rules\n", "x/billing.md"), "Billing rules");
        assert_eq!(title_of("no heading", "x/billing.md"), "billing");
        let body = "See [a](../code/a.md#top), [web](https://x.y), [b](b.md \"t\"), [mail](mailto:a@b)\n```\n[c](c.md)\n```\n";
        assert_eq!(links(body), ["../code/a.md", "b.md"]);
        assert_eq!(normalize(Path::new(".docs/knowledge/../x.md")), Some(PathBuf::from(".docs/x.md")));
        assert_eq!(normalize(Path::new("../x.md")), None);
    }

    #[test]
    fn queries_need_three_characters_per_term() {
        assert_eq!(fts_query("นัดหมาย booking", "AND").as_deref(), Some("\"นัดหมาย\" AND \"booking\""));
        assert_eq!(fts_query("a \"x\"y", "OR").as_deref(), Some("\"\"\"x\"\"y\""));
        assert_eq!(fts_query("ab", "AND"), None);
    }

    fn git(dir: &Path, args: &[&str], at: i64) {
        let date = format!("@{at} +0000");
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=T", "-c", "user.email=t@t"])
            .args(args)
            .env("GIT_AUTHOR_DATE", &date)
            .env("GIT_COMMITTER_DATE", &date)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    }

    #[tokio::test]
    async fn indexes_checks_and_searches_a_project() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\n",
            dir.path().display()
        );
        let config = crate::config::Config::parse(&text, Path::new("/")).unwrap();
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        let root = dir.path().join("clinic");
        let w = |p: &str, body: &str| {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, body).unwrap();
        };
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"], 1_000);
        w("src/booking.rs", "fn book() {}\n");
        w(".docs/knowledge/domain/booking.md", "---\nsources:\n  - src/booking.rs\n  - src/gone.rs\n---\n# Booking\n\nระบบนัดหมายคลินิก: appointment slots. See [billing](billing.md).\n");
        w(".docs/knowledge/domain/orphan.md", &format!("# Orphan\n{}", "line\n".repeat(205)));
        w(".docs/knowledge/index.md", "# Index\n| [booking](domain/booking.md) | appointments |\n");
        w("CLAUDE.md", &"x".repeat(53_000));
        w("tasks/done/001-first.md", "# First task\n- status: done\n");
        w(".scratch/copy/CLAUDE.md", "old");
        std::fs::create_dir_all(root.join(".scratch/copy/tasks")).unwrap();
        git(&root, &["add", "."], 1_000);
        git(&root, &["commit", "-qm", "docs"], 1_000);
        // The source changes after the card.
        w("src/booking.rs", "fn book() { /* v2 */ }\n");
        git(&root, &["commit", "-qam", "code"], 2_000);

        let settings = crate::model::ProjectSettings {
            source: crate::model::ProjectSource { kind: crate::model::SourceKind::Local, location: Some(root.display().to_string()), ..Default::default() },
            ..Default::default()
        };
        let new = crate::projects::NewProject { name: "Clinic".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let (project, _) = crate::projects::create_with(&hub, new, settings).await.unwrap();

        let o = overview(&hub, &project, true).await.unwrap();
        let kinds: HashMap<&str, KnowledgeKind> = o.docs.iter().map(|d| (d.path.as_str(), d.kind)).collect();
        assert_eq!(kinds[".docs/knowledge/domain/booking.md"], KnowledgeKind::Card);
        assert_eq!(kinds["CLAUDE.md"], KnowledgeKind::Rulebook);
        assert_eq!(kinds["tasks/done/001-first.md"], KnowledgeKind::Task);
        assert!(!kinds.contains_key(".scratch/copy/CLAUDE.md"), "scratch folders are not indexed");
        let checks = |c: KnowledgeCheck| o.issues.iter().filter(|i| i.check == c).map(|i| i.path.clone()).collect::<Vec<_>>();
        assert_eq!(checks(KnowledgeCheck::Stale), [".docs/knowledge/domain/booking.md"]);
        assert_eq!(checks(KnowledgeCheck::MissingSource), [".docs/knowledge/domain/booking.md"]);
        assert_eq!(checks(KnowledgeCheck::OverCap), [".docs/knowledge/domain/orphan.md"]);
        assert_eq!(checks(KnowledgeCheck::NotInIndex), [".docs/knowledge/domain/orphan.md"]);
        assert_eq!(checks(KnowledgeCheck::BrokenLink), [".docs/knowledge/domain/booking.md"]);
        assert_eq!(checks(KnowledgeCheck::RulebookSize), ["CLAUDE.md"]);
        assert_eq!(checks(KnowledgeCheck::NestedCopy), [".scratch"]);
        let stale = o.issues.iter().find(|i| i.check == KnowledgeCheck::Stale).unwrap();
        assert!(stale.fix_prompt.as_deref().unwrap().contains("src/booking.rs"), "{stale:?}");
        assert_eq!(o.issues[0].level, Level::Error, "most serious first");

        // Thai substring, English word, and kind filter; the search is logged.
        let s = search(&hub, &project, "นัดหมาย", None, 10, "agent").await.unwrap();
        assert_eq!(s.hits.len(), 1);
        assert!(s.hits[0].snippet.contains("[นัดหมาย]"), "{:?}", s.hits[0]);
        assert_eq!(search(&hub, &project, "appointment", Some(KnowledgeKind::Task), 10, "owner").await.unwrap().hits.len(), 0);
        assert_eq!(search(&hub, &project, "ab", None, 10, "owner").await.unwrap().query, "ab");
        let q = recent_queries(&hub, &project.slug, 10).await;
        assert_eq!((q.len(), q.last().unwrap().by.as_str()), (3, "agent"));

        // MAP remembers finished tasks.
        let mut t = hub.store.create_task(crate::store::NewTask { title: "Add slots".into(), prompt: "add appointment slots".into(), depends_on: vec![], provider: None, model: None, project: Some(project.slug.clone()), workflow: None }).await.unwrap();
        t.status = TaskStatus::Done;
        t.result = Some("slots added with a uniqueness check".into());
        hub.store.save_task(&t).await.unwrap();
        ensure_index(&hub, &project, true).await.unwrap();
        let s = search(&hub, &project, "uniqueness", Some(KnowledgeKind::Memory), 10, "owner").await.unwrap();
        assert_eq!(s.hits.first().map(|h| h.path.clone()), Some(format!("map://tasks/{}", t.id)));
        let body = doc(&hub, &project, &format!("map://tasks/{}", t.id)).await.unwrap().unwrap().body;
        assert!(body.contains("## Outcome") && body.contains("uniqueness"), "{body}");
        assert!(doc(&hub, &project, "../../etc/passwd").await.unwrap().is_none(), "only indexed paths are readable");
    }

    #[tokio::test]
    async fn suggests_knowledge_once_the_project_is_big_enough() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\n",
            dir.path().display()
        );
        let config = crate::config::Config::parse(&text, Path::new("/")).unwrap();
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        let root = dir.path().join("shop");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let settings = crate::model::ProjectSettings {
            source: crate::model::ProjectSource { kind: crate::model::SourceKind::Local, location: Some(root.display().to_string()), ..Default::default() },
            ..Default::default()
        };
        let new = crate::projects::NewProject { name: "Shop".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let (project, _) = crate::projects::create_with(&hub, new, settings).await.unwrap();

        let s = overview(&hub, &project, true).await.unwrap().suggestion;
        assert_eq!(s.verdict, SuggestVerdict::NotYet, "{:?}", s.reasons);
        assert_eq!((s.stats.source_files, s.stats.source_lines), (1, 1));
        assert!(s.reasons[0].starts_with("Still small"));

        // Big enough (build output and node_modules do not count).
        for i in 0..SUGGEST_FILES {
            std::fs::write(root.join(format!("src/m{i}.rs")), "fn f() {}\n".repeat(5)).unwrap();
        }
        std::fs::create_dir_all(root.join("node_modules/x")).unwrap();
        std::fs::write(root.join("node_modules/x/i.js"), "x").unwrap();
        let s = overview(&hub, &project, true).await.unwrap().suggestion;
        assert_eq!(s.verdict, SuggestVerdict::Recommended);
        assert_eq!(s.stats.source_files, SUGGEST_FILES + 1);
        assert!(s.reasons[0].contains("source files") && s.plan.len() == 6, "{:?}", s.reasons);

        dismiss(&hub, &project.slug, true).await.unwrap();
        assert_eq!(overview(&hub, &project, false).await.unwrap().suggestion.verdict, SuggestVerdict::Dismissed);

        // Accept: one task with MAP's practice, once.
        let t = accept(&hub, &project).await.unwrap();
        assert_eq!((t.title.as_str(), t.project.as_deref()), (BOOTSTRAP_TITLE, Some(project.slug.as_str())));
        assert!(t.prompt.contains("sources:") && t.prompt.contains(".docs/knowledge/index.md") && t.prompt.contains("$MAP_KNOWLEDGE_URL"));
        assert_eq!(accept(&hub, &project).await.unwrap().id, t.id);
        let s = overview(&hub, &project, false).await.unwrap().suggestion;
        assert_eq!((s.verdict, s.task), (SuggestVerdict::InProgress, Some(t.id)));

        // The index is there: in use.
        std::fs::create_dir_all(root.join(".docs/knowledge")).unwrap();
        std::fs::write(root.join(".docs/knowledge/index.md"), "# Knowledge index\n").unwrap();
        assert_eq!(overview(&hub, &project, true).await.unwrap().suggestion.verdict, SuggestVerdict::InUse);
    }
}
