//! The knowledge graph from a small project folder: sources, mentions, links, domains, stale and missing.

use super::*;

fn doc(path: &str, kind: KnowledgeKind, sources: &[&str]) -> KnowledgeDoc {
    KnowledgeDoc { path: path.into(), kind, title: path.rsplit('/').next().unwrap().trim_end_matches(".md").into(), lines: 10, bytes: 100, modified: None, sources: sources.iter().map(|s| s.to_string()).collect(), in_index: true }
}

#[test]
fn domains_skip_container_folders() {
    assert_eq!(code_domain("api/src/billing/refund.rs"), "api/billing");
    assert_eq!(code_domain("web/app/(clinic)/refund/page.tsx"), "web/(clinic)");
    assert_eq!(code_domain("src/main.rs"), "src");
    assert_eq!(code_domain("README"), "(root)");
}

#[test]
fn builds_nodes_edges_and_states() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let w = |p: &str, s: &str| {
        let f = root.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, s).unwrap();
    };
    w("api/src/billing/refund.rs", "fn x() {}");
    w("api/src/billing/total.rs", "fn y() {}");
    w(".docs/knowledge/refunds.md", "---\nsources: [api/src/billing/refund.rs, api/src/gone.rs]\n---\n# Refunds\nSee [totals](totals.md) and `api/src/billing/total.rs:12`.\n```\nignored/in/fence.rs\n```\n");
    w(".docs/knowledge/totals.md", "# Totals\nNothing cited.\n");
    let docs = vec![doc(".docs/knowledge/refunds.md", KnowledgeKind::Card, &["api/src/billing/refund.rs", "api/src/gone.rs"]), doc(".docs/knowledge/totals.md", KnowledgeKind::Card, &[])];
    let issues = vec![KnowledgeIssue { level: Level::Warning, check: KnowledgeCheck::Stale, path: ".docs/knowledge/refunds.md".into(), detail: "x".into(), fix_title: None, fix_prompt: None }];
    let g = build("app", root, &docs, &issues);

    let n = |id: &str| g.nodes.iter().find(|n| n.id == id).unwrap_or_else(|| panic!("{id} in {:?}", g.nodes.iter().map(|n| &n.id).collect::<Vec<_>>()));
    let refunds = n("doc:.docs/knowledge/refunds.md");
    assert_eq!(refunds.domain, "api/billing");
    assert_eq!(refunds.state, "missing", "a cited file is gone");
    assert_eq!(refunds.level, Some(Level::Warning));
    assert_eq!(n("code:api/src/billing/refund.rs").state, "stale");
    assert_eq!(n("code:api/src/gone.rs").state, "missing");
    assert_eq!(n("code:api/src/billing/total.rs").state, "ok", "a mention is not stale");
    assert_eq!(n("doc:.docs/knowledge/totals.md").domain, "docs: .docs/knowledge");
    assert!(g.nodes.iter().all(|x| !x.path.contains("fence")), "fenced code is skipped");
    let kinds: HashSet<(&str, &str)> = g.edges.iter().map(|e| (e.to.as_str(), e.kind.as_str())).collect();
    assert!(kinds.contains(&("doc:.docs/knowledge/totals.md", "link")));
    assert!(kinds.contains(&("code:api/src/billing/total.rs", "mention")));
    assert!(kinds.contains(&("code:api/src/billing/refund.rs", "source")));
    let billing = g.domains.iter().find(|d| d.id == "api/billing").unwrap();
    assert_eq!((billing.docs, billing.code), (1, 2));
    assert!(billing.summary.contains("Refunds") || billing.summary.contains("refunds"), "{}", billing.summary);
    assert!(!g.truncated);
}

#[test]
fn caps_documents_by_usefulness() {
    let dir = tempfile::tempdir().unwrap();
    let mut docs: Vec<KnowledgeDoc> = (0..MAX_DOCS + 20).map(|i| doc(&format!("tasks/done/{i:04}.md"), KnowledgeKind::Task, &[])).collect();
    docs.push(doc("docs/req.md", KnowledgeKind::Requirement, &[]));
    let g = build("app", dir.path(), &docs, &[]);
    assert!(g.truncated);
    assert_eq!(g.nodes.iter().filter(|n| n.kind == "doc").count(), MAX_DOCS);
    assert!(g.nodes.iter().any(|n| n.path == "docs/req.md"), "a requirement beats task sheets");
    assert_eq!(g.total_docs as usize, MAX_DOCS + 21);
}

