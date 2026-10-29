//! RAG for a project's agents, on or off (`settings.knowledge.rag`, the switch on the Knowledge screen). On: every
//! task's prompt tells the agent to search the project's knowledge before it starts and to read only the parts it
//! needs; off: the prompt says nothing of it and agents' searches and reads are refused (the owner's still work).
//!
//! With RAG on, the task's prompt also nudges the agent to save what it learned as a knowledge card ([`card_nudge`]).
//!
//! A document an agent reads comes back in parts: up to [`DOC_BUDGET_TOKENS`] it is whole; above that it is its
//! outline (each heading with its size) and the text before the first heading, and `section=<heading>` reads one
//! section. `full=1` reads it whole anyway. A rulebook read whole cost ~14,000 tokens; one section, a few hundred.

use crate::hub::Hub;
use crate::model::{Project, Task};
use crate::savings::estimate_tokens;

/// A document an agent reads is sent whole up to this size.
pub const DOC_BUDGET_TOKENS: i64 = 2000;
/// The text before the first heading sent with an outline.
const INTRO_CHARS: usize = 1200;

pub const TASK_HINT: &str = "- Before you start, search the project's knowledge (its docs, requirements, past tasks and their outcomes, approved \
     plans): `curl -s -G -H \"Authorization: Bearer $MAP_PROJECT_TOKEN\" \"$MAP_KNOWLEDGE_URL/search\" --data-urlencode \"q=<a few words>\"` \
     (terms of 3+ characters, Thai or English; `&kind=card|requirement|task|rulebook|doc|memory`). Read a hit with \
     `curl -s -G -H \"Authorization: Bearer $MAP_PROJECT_TOKEN\" \"$MAP_KNOWLEDGE_URL/doc\" --data-urlencode \"path=<path>\"`: a long document \
     comes back as its outline, so read only the section you need with `--data-urlencode \"section=<heading>\"` (`full=1` for all of it). \
     Prefer this to opening whole documents: it keeps your context small.\n";

pub fn on(project: &Project) -> bool {
    project.settings.knowledge.rag
}

/// `prompt` with the RAG instructions when the task is in a project with RAG on.
pub async fn with_hint(hub: &Hub, task: &Task, mut prompt: String) -> String {
    let Some(slug) = &task.project else { return prompt };
    if let Some(p) = hub.store.project(slug).await.ok().flatten().filter(on) {
        prompt.push_str(TASK_HINT);
        prompt.push_str(&card_nudge(&p));
    }
    prompt
}

/// The memory nudge (Hermes Agent's idea): save what the next task would need as a knowledge card, in the folder of
/// the cards' index, listed in the index. Empty with RAG off.
pub fn card_nudge(project: &Project) -> String {
    let index = project.settings.knowledge.index.trim();
    if !on(project) || index.is_empty() {
        return String::new();
    }
    let folder = std::path::Path::new(index).parent().map(|d| d.display().to_string()).filter(|d| !d.is_empty()).unwrap_or_else(|| ".".into());
    format!(
        "- Before you finish: if you learned a fact about this project the next task would need and no card holds yet (a trap, a rule, \
         where something lives, why a choice was made), save it as a knowledge card: a short Markdown file under `{folder}/` with a \
         `sources:` frontmatter list of the files it is about, linked from `{index}`, committed with your change. One fact per card; \
         update a card that already covers it instead of adding a second.\n"
    )
}

/// Why `by` may not search or read the project's knowledge: RAG is off and it is not the owner.
pub fn refused(project: &Project, by: &str) -> Option<String> {
    (!on(project) && by != "owner").then(|| format!("RAG is off for project `{}`: its knowledge is not served to agents. Work from the repository.", project.slug))
}

/// A Markdown heading line outside code fences: (level, text).
fn heading(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|c| *c == '#').count();
    let text = line.get(level..)?;
    ((1..=6).contains(&level) && text.starts_with(' ')).then(|| (level, text.trim()))
}

/// (line index, level, text) of every heading.
fn headings(lines: &[&str]) -> Vec<(usize, usize, String)> {
    let mut fenced = false;
    let mut out = vec![];
    for (i, line) in lines.iter().enumerate() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        } else if !fenced && let Some((level, text)) = heading(line) {
            out.push((i, level, text.to_string()));
        }
    }
    out
}

/// The line where the section starting at heading `k` ends: the next heading of the same or a higher level.
fn section_end(hs: &[(usize, usize, String)], k: usize, total: usize) -> usize {
    hs[k + 1..].iter().find(|h| h.1 <= hs[k].1).map_or(total, |h| h.0)
}

/// What an agent gets of a document: one section, the whole text, or the outline of a long one.
pub fn shaped(path: &str, body: &str, section: Option<&str>, full: bool) -> String {
    let lines: Vec<&str> = body.lines().collect();
    let hs = headings(&lines);
    let tokens = estimate_tokens(body.chars().count() as i64);
    if let Some(want) = section.map(str::trim).filter(|s| !s.is_empty()) {
        let want = want.trim_start_matches('#').trim().to_lowercase();
        let k = hs.iter().position(|h| h.2.to_lowercase() == want).or_else(|| hs.iter().position(|h| h.2.to_lowercase().contains(&want)));
        return match k {
            Some(k) => lines[hs[k].0..section_end(&hs, k, lines.len())].join("\n"),
            None => format!("No section `{want}` in {path}.\n\n{}", outline(path, &lines, &hs, tokens)),
        };
    }
    if full || tokens <= DOC_BUDGET_TOKENS {
        return body.to_string();
    }
    if hs.is_empty() {
        let cut: String = body.chars().take((DOC_BUDGET_TOKENS * 4) as usize).collect();
        return format!("{cut}\n\n[{path} is ~{tokens} tokens and has no headings: this is its start. Pass full=1 for all of it.]");
    }
    outline(path, &lines, &hs, tokens)
}

fn outline(path: &str, lines: &[&str], hs: &[(usize, usize, String)], tokens: i64) -> String {
    let mut out = format!("[{path} is ~{tokens} tokens. Its outline follows; read one section with section=<heading>, or full=1 for all.]\n\n");
    let intro: String = lines[..hs.first().map_or(0, |h| h.0)].join("\n").chars().take(INTRO_CHARS).collect();
    if !intro.trim().is_empty() {
        out.push_str(intro.trim());
        out.push_str("\n\n");
    }
    for (k, (i, level, text)) in hs.iter().enumerate() {
        let size = estimate_tokens(lines[*i..section_end(hs, k, lines.len())].join("\n").chars().count() as i64);
        out.push_str(&format!("{}- {text} (~{size} tokens)\n", "  ".repeat(level - 1)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc() -> String {
        let filler = "word ".repeat(2000);
        format!("Intro line.\n\n# Rules\n\n## Refunds\nRefund within 7 days.\n\n```\n# not a heading\n```\n\n## Bookings\n{filler}\n\n# Ops\nDeploy.\n")
    }

    #[test]
    fn a_long_document_is_its_outline_and_a_section_reads_alone() {
        let d = doc();
        let o = shaped("CLAUDE.md", &d, None, false);
        assert!(o.starts_with("[CLAUDE.md is ~"), "{o}");
        assert!(o.contains("Intro line.") && o.contains("- Rules (~") && o.contains("  - Refunds (~") && o.contains("- Ops (~"));
        assert!(!o.contains("not a heading"), "a # line in a code fence is not a heading");
        let s = shaped("CLAUDE.md", &d, Some("refunds"), false);
        assert!(s.starts_with("## Refunds") && s.contains("7 days") && !s.contains("Bookings"), "{s}");
        assert!(shaped("CLAUDE.md", &d, Some("Rules"), false).contains("## Bookings"), "a section holds its sub-sections");
        assert!(shaped("CLAUDE.md", &d, Some("book"), false).starts_with("## Bookings"), "a part of the heading matches");
        assert!(shaped("CLAUDE.md", &d, Some("nope"), false).starts_with("No section `nope`"));
        assert_eq!(shaped("CLAUDE.md", &d, None, true), d);
        assert_eq!(shaped("a.md", "# Short\nok", None, false), "# Short\nok");
    }

    #[test]
    fn a_long_document_without_headings_is_cut() {
        let d = "x".repeat(20_000);
        let s = shaped("a.txt", &d, None, false);
        assert!(s.len() < 9000 && s.contains("has no headings"));
    }

    #[test]
    fn the_card_nudge_names_the_index_and_follows_the_switch() {
        let mut p = Project {
            slug: "s".into(),
            name: "S".into(),
            description: String::new(),
            port: 0,
            threshold: None,
            fallback: "none".into(),
            model: None,
            questions: vec![],
            created_at: chrono::Utc::now(),
            settings: Default::default(),
        };
        let n = card_nudge(&p);
        assert!(n.contains("`.docs/knowledge/`") && n.contains("`.docs/knowledge/index.md`"), "{n}");
        p.settings.knowledge.rag = false;
        assert_eq!(card_nudge(&p), "");
    }
}
