//! What agents and the owner took from a project's knowledge, and what it cost them in context: every search and
//! every document read, with the tokens MAP returned (about 4 characters each, `savings::estimate_tokens`). The
//! Knowledge screen sums them per reader, so a knowledge setup that floods the agents' context shows. A search
//! returns at most [`SEARCH_BUDGET_TOKENS`] of hits: the best first, the rest left out (at least one is kept).

use chrono::Utc;
use sqlx::Row;

use crate::hub::Hub;
use crate::model::{KnowledgeHit, KnowledgeQuery};
use crate::savings::estimate_tokens;

/// The most one search returns: about 6,000 characters of titles, paths and passages.
pub const SEARCH_BUDGET_TOKENS: i64 = 1500;
/// Rows kept per project.
const KEEP: i64 = 1000;

fn hit_tokens(h: &KnowledgeHit) -> i64 {
    // The JSON keys and the score cost a few tokens more than the text.
    estimate_tokens((h.path.chars().count() + h.title.chars().count() + h.snippet.chars().count()) as i64) + 12
}

/// `hits` cut to the budget, and the tokens of what is left.
pub fn within_budget(hits: Vec<KnowledgeHit>) -> (Vec<KnowledgeHit>, i64) {
    let mut total = 0;
    let mut kept = vec![];
    for h in hits {
        let t = hit_tokens(&h);
        if !kept.is_empty() && total + t > SEARCH_BUDGET_TOKENS {
            break;
        }
        total += t;
        kept.push(h);
    }
    (kept, total)
}

/// Logs one search (`kind` `search`, the query) or document read (`doc`, the path); keeps the newest [`KEEP`].
pub async fn record(hub: &Hub, slug: &str, kind: &str, query: &str, hits: usize, tokens: i64, by: &str) {
    let query: String = query.chars().take(300).collect();
    let _ = sqlx::query("INSERT INTO knowledge_queries (project, at, query, hits, by_whom, kind, tokens) VALUES (?, ?, ?, ?, ?, ?, ?)")
        .bind(slug)
        .bind(Utc::now())
        .bind(query)
        .bind(hits as i64)
        .bind(by)
        .bind(kind)
        .bind(tokens)
        .execute(&hub.store.pool)
        .await;
    let _ = sqlx::query("DELETE FROM knowledge_queries WHERE project = ? AND id NOT IN (SELECT id FROM knowledge_queries WHERE project = ? ORDER BY id DESC LIMIT ?)")
        .bind(slug)
        .bind(slug)
        .bind(KEEP)
        .execute(&hub.store.pool)
        .await;
}

/// A document read: its text's tokens.
pub async fn record_doc(hub: &Hub, slug: &str, path: &str, body: &str, by: &str) {
    record(hub, slug, "doc", path, 1, estimate_tokens(body.chars().count() as i64), by).await;
}

/// The newest rows, newest first: searches only, or document reads too.
pub async fn recent(hub: &Hub, slug: &str, limit: i64, with_reads: bool) -> Vec<KnowledgeQuery> {
    let sql = if with_reads {
        "SELECT at, query, hits, by_whom, kind, tokens FROM knowledge_queries WHERE project = ? ORDER BY id DESC LIMIT ?"
    } else {
        "SELECT at, query, hits, by_whom, kind, tokens FROM knowledge_queries WHERE project = ? AND kind = 'search' ORDER BY id DESC LIMIT ?"
    };
    let rows = sqlx::query(sql).bind(slug).bind(limit).fetch_all(&hub.store.pool).await.unwrap_or_default();
    rows.iter()
        .map(|r| KnowledgeQuery {
            at: r.get("at"),
            query: r.get("query"),
            hits: r.get::<i64, _>("hits") as u32,
            by: r.get("by_whom"),
            kind: r.get("kind"),
            tokens: r.get::<i64, _>("tokens").max(0) as u32,
        })
        .collect()
}

/// Searches only, as the task traces read them.
pub async fn recent_queries(hub: &Hub, slug: &str, limit: i64) -> Vec<KnowledgeQuery> {
    recent(hub, slug, limit, false).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::KnowledgeKind;

    fn hit(chars: usize) -> KnowledgeHit {
        KnowledgeHit { path: "a.md".into(), kind: KnowledgeKind::Doc, title: "A".into(), snippet: "x".repeat(chars), score: 1.0 }
    }

    #[test]
    fn the_budget_keeps_the_best_hits_and_at_least_one() {
        let (kept, tokens) = within_budget(vec![hit(2000), hit(2000), hit(2000), hit(2000)]);
        assert_eq!(kept.len(), 2, "two of ~514 tokens fit in 1500, a third does not");
        assert!(tokens <= SEARCH_BUDGET_TOKENS);
        let (kept, tokens) = within_budget(vec![hit(20_000)]);
        assert_eq!(kept.len(), 1);
        assert!(tokens > SEARCH_BUDGET_TOKENS);
        assert_eq!(within_budget(vec![]).0.len(), 0);
    }
}
