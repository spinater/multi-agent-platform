//! The Knowledge screen's graph (`GET /api/projects/{slug}/knowledge/graph`), built from the index without an LLM:
//! every indexed document, the code files it cites (frontmatter `sources:` and code paths written in its text), and
//! its Markdown links to other documents. Documents are clustered into domains by the code folder most of their
//! citations point at (`api/billing`), else by their own folder; each domain gets a generated one-line summary.
//! Health findings colour the nodes: a stale card and the code it cites are `stale`, a cited file that is gone is
//! `missing`. Large projects keep the most useful documents (cards, requirements, rulebooks, docs, then the newest
//! task sheets) up to a cap.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::knowledge::{links, normalize};
use crate::model::{KnowledgeCheck, KnowledgeDoc, KnowledgeGraph, KnowledgeGraphDomain, KnowledgeGraphEdge, KnowledgeGraphNode, KnowledgeIssue, KnowledgeKind, Level};

/// At most this many documents, and this many code files, in one graph.
pub const MAX_DOCS: usize = 300;
pub const MAX_CODE: usize = 500;
/// Bytes of a document read for links and code mentions.
const MAX_READ: u64 = 256 * 1024;

/// Folder names that say nothing about a domain; skipped when naming one.
const CONTAINERS: &[&str] = &["src", "lib", "app", "apps", "packages", "crates", "internal", "pkg", "source", "sources", "main", "java", "kotlin", "scala", "go", "cmd", "components", "modules"];

/// `path/with/slash.ext` in backticks or plain text: something that looks like a file in the repo.
static CODE_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:^|[\s`(\[])((?:[\w.@()\[\]-]+/)+[\w.@\[\]-]+\.[A-Za-z0-9]{1,6})(?::\d+)?").unwrap());

fn rank(k: KnowledgeKind) -> u8 {
    match k {
        KnowledgeKind::Card => 0,
        KnowledgeKind::Requirement => 1,
        KnowledgeKind::Rulebook => 2,
        KnowledgeKind::Doc => 3,
        KnowledgeKind::Task => 4,
        KnowledgeKind::Memory => 5,
    }
}

fn is_doc_path(p: &str) -> bool {
    p.ends_with(".md") || p.ends_with(".mdx") || p.ends_with(".txt")
}

/// The domain of a code file: its first two meaningful folders (`api/src/billing/x.rs` → `api/billing`).
pub fn code_domain(path: &str) -> String {
    let dirs: Vec<&str> = path.split('/').collect();
    let dirs = &dirs[..dirs.len().saturating_sub(1)];
    let mut out: Vec<&str> = vec![];
    for (i, d) in dirs.iter().enumerate() {
        if i > 0 && CONTAINERS.contains(&d.to_ascii_lowercase().as_str()) {
            continue;
        }
        out.push(d);
        if out.len() == 2 {
            break;
        }
    }
    if out.is_empty() { "(root)".into() } else { out.join("/") }
}

/// A document's own folder as a domain (`docs: tasks/done`).
fn doc_folder(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, _)) => format!("docs: {dir}"),
        None => "docs: (root)".into(),
    }
}

fn worse(a: Option<Level>, b: Level) -> Option<Level> {
    let score = |l: Level| match l {
        Level::Error => 3,
        Level::Warning => 2,
        Level::Info => 1,
        Level::Success => 0,
    };
    match a {
        Some(x) if score(x) >= score(b) => Some(x),
        _ => Some(b),
    }
}

/// Code paths written in a document's text that exist in the project (resolved from the root, then from the
/// document's folder).
fn mentions(root: &Path, doc_path: &str, body: &str, exists: &mut HashMap<String, bool>) -> Vec<String> {
    let dir = Path::new(doc_path).parent().unwrap_or(Path::new(""));
    let mut out = vec![];
    let mut in_fence = false;
    for line in body.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        for cap in CODE_PATH.captures_iter(line) {
            let raw = cap[1].trim_start_matches("./");
            if raw.contains("://") || is_doc_path(raw) {
                continue;
            }
            for cand in [Path::new(raw).to_path_buf(), dir.join(raw)] {
                let Some(p) = normalize(&cand) else { continue };
                let key = p.to_string_lossy().to_string();
                let found = *exists.entry(key.clone()).or_insert_with(|| root.join(&key).is_file());
                if found {
                    if !out.contains(&key) {
                        out.push(key);
                    }
                    break;
                }
            }
        }
    }
    out
}

fn read_head(path: &Path) -> String {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(path) else { return String::new() };
    let mut s = String::new();
    let _ = f.take(MAX_READ).read_to_string(&mut s);
    s
}

/// Builds the graph from the index (`docs`) and its health findings. Reads the documents' text under `root`.
pub fn build(slug: &str, root: &Path, docs: &[KnowledgeDoc], issues: &[KnowledgeIssue]) -> KnowledgeGraph {
    // The most useful documents first: kind, then newest.
    let mut chosen: Vec<&KnowledgeDoc> = docs.iter().filter(|d| !d.path.starts_with("map://")).collect();
    chosen.sort_by(|a, b| rank(a.kind).cmp(&rank(b.kind)).then(b.modified.cmp(&a.modified)).then(a.path.cmp(&b.path)));
    let truncated = chosen.len() > MAX_DOCS;
    chosen.truncate(MAX_DOCS);
    let in_graph: HashSet<&str> = chosen.iter().map(|d| d.path.as_str()).collect();

    let mut level: HashMap<&str, Option<Level>> = HashMap::new();
    let mut stale_docs: HashSet<&str> = HashSet::new();
    for i in issues {
        let e = level.entry(i.path.as_str()).or_insert(None);
        *e = worse(*e, i.level);
        if i.check == KnowledgeCheck::Stale {
            stale_docs.insert(i.path.as_str());
        }
    }

    let mut exists: HashMap<String, bool> = HashMap::new();
    let mut edges: Vec<KnowledgeGraphEdge> = vec![];
    let mut code_refs: BTreeMap<String, (u32, bool, bool)> = BTreeMap::new(); // path → (degree, stale, missing)
    let mut seen_edge: HashSet<(String, String)> = HashSet::new();
    for d in &chosen {
        let from = format!("doc:{}", d.path);
        let stale = stale_docs.contains(d.path.as_str());
        let body = read_head(&root.join(&d.path));
        let mut push = |to: String, kind: &str, state: &str, edges: &mut Vec<KnowledgeGraphEdge>| {
            if seen_edge.insert((from.clone(), to.clone())) {
                edges.push(KnowledgeGraphEdge { from: from.clone(), to, kind: kind.into(), state: state.into() });
            }
        };
        for s in &d.sources {
            let Some(p) = normalize(Path::new(s.trim_start_matches("./"))) else { continue };
            let key = p.to_string_lossy().to_string();
            if is_doc_path(&key) && in_graph.contains(key.as_str()) {
                push(format!("doc:{key}"), "link", "ok", &mut edges);
                continue;
            }
            let found = *exists.entry(key.clone()).or_insert_with(|| root.join(&key).exists());
            let state = if !found { "missing" } else if stale { "stale" } else { "ok" };
            let e = code_refs.entry(key.clone()).or_insert((0, false, false));
            e.0 += 1;
            e.1 |= stale && found;
            e.2 |= !found;
            push(format!("code:{key}"), "source", state, &mut edges);
        }
        let dir = Path::new(&d.path).parent().unwrap_or(Path::new("")).to_path_buf();
        for l in links(&body) {
            let Some(p) = normalize(&dir.join(&l)) else { continue };
            let key = p.to_string_lossy().to_string();
            if key != d.path && in_graph.contains(key.as_str()) {
                push(format!("doc:{key}"), "link", "ok", &mut edges);
            }
        }
        for m in mentions(root, &d.path, &body, &mut exists) {
            let e = code_refs.entry(m.clone()).or_insert((0, false, false));
            e.0 += 1;
            push(format!("code:{m}"), "mention", "ok", &mut edges);
        }
    }

    // Keep the most cited code files.
    let mut code: Vec<(String, (u32, bool, bool))> = code_refs.into_iter().collect();
    code.sort_by(|a, b| b.1.0.cmp(&a.1.0).then(a.0.cmp(&b.0)));
    let truncated = truncated || code.len() > MAX_CODE;
    code.truncate(MAX_CODE);
    let kept: HashSet<String> = code.iter().map(|(p, _)| format!("code:{p}")).collect();
    edges.retain(|e| !e.to.starts_with("code:") || kept.contains(&e.to));

    // Domains: a document goes where most of its code points; code by its folder.
    let mut votes: HashMap<&str, BTreeMap<String, u32>> = HashMap::new();
    for e in &edges {
        if let Some(c) = e.to.strip_prefix("code:") {
            *votes.entry(e.from.as_str()).or_default().entry(code_domain(c)).or_default() += 1;
        }
    }
    let mut degree: HashMap<&str, u32> = HashMap::new();
    for e in &edges {
        *degree.entry(e.from.as_str()).or_default() += 1;
        *degree.entry(e.to.as_str()).or_default() += 1;
    }
    let mut nodes: Vec<KnowledgeGraphNode> = vec![];
    for d in &chosen {
        let id = format!("doc:{}", d.path);
        let domain = votes.get(id.as_str()).and_then(|v| v.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(k, _)| k.clone())).unwrap_or_else(|| doc_folder(&d.path));
        let stale = stale_docs.contains(d.path.as_str());
        let missing = edges.iter().any(|e| e.from == id && e.state == "missing");
        nodes.push(KnowledgeGraphNode {
            degree: degree.get(id.as_str()).copied().unwrap_or(0),
            id,
            kind: "doc".into(),
            label: d.title.clone(),
            path: d.path.clone(),
            doc_kind: Some(d.kind),
            domain,
            level: level.get(d.path.as_str()).copied().flatten(),
            state: if missing { "missing" } else if stale { "stale" } else { "ok" }.into(),
            lines: d.lines,
        });
    }
    for (p, (_, stale, missing)) in &code {
        let id = format!("code:{p}");
        nodes.push(KnowledgeGraphNode {
            degree: degree.get(id.as_str()).copied().unwrap_or(0),
            id,
            kind: "code".into(),
            label: p.rsplit('/').next().unwrap_or(p).to_string(),
            path: p.clone(),
            doc_kind: None,
            domain: code_domain(p),
            level: None,
            state: if *missing { "missing" } else if *stale { "stale" } else { "ok" }.into(),
            lines: 0,
        });
    }

    // Domain summaries from the counts and the best-connected documents.
    let mut by: BTreeMap<String, Vec<&KnowledgeGraphNode>> = BTreeMap::new();
    for n in &nodes {
        by.entry(n.domain.clone()).or_default().push(n);
    }
    let mut domains: Vec<KnowledgeGraphDomain> = by
        .into_iter()
        .map(|(id, members)| {
            let docs_n = members.iter().filter(|n| n.kind == "doc").count() as u32;
            let code_n = members.len() as u32 - docs_n;
            let flagged = members.iter().filter(|n| matches!(n.level, Some(Level::Warning | Level::Error)) || n.state != "ok").count() as u32;
            let mut top: Vec<&&KnowledgeGraphNode> = members.iter().filter(|n| n.kind == "doc").collect();
            top.sort_by(|a, b| b.degree.cmp(&a.degree).then(a.label.cmp(&b.label)));
            let names: Vec<String> = top.iter().take(3).map(|n| n.label.clone()).collect();
            let mut summary = format!("{docs_n} document{} and {code_n} code file{}", if docs_n == 1 { "" } else { "s" }, if code_n == 1 { "" } else { "s" });
            if !names.is_empty() {
                summary.push_str(&format!("; mainly {}", names.join(", ")));
            }
            if flagged > 0 {
                summary.push_str(&format!("; {flagged} need attention"));
            }
            let name = id.trim_start_matches("docs: ").to_string();
            KnowledgeGraphDomain { id, name, summary, docs: docs_n, code: code_n, flagged }
        })
        .collect();
    domains.sort_by(|a, b| (b.docs + b.code).cmp(&(a.docs + a.code)).then(a.id.cmp(&b.id)));

    KnowledgeGraph { slug: slug.into(), domains, nodes, edges, total_docs: docs.len() as u32, truncated }
}

#[cfg(test)]
mod tests;
