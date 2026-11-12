//! A project's corpus learns from the answers it can trust. Reflex answers only from its corpus, so a project starts
//! with every question going to the fallback. When the fallback panel answers a choice question unanimously (two or
//! more members) or the owner confirms or corrects an answer, the facts become one example line in the domain named
//! like the answer (`<option>/learned.md` in the corpus repo), and Reflex restarts on it a minute after the last
//! lesson. The next question like it Reflex can answer itself.
//!
//! Each lesson is logged with what Reflex said before it learned (`corpus_lessons`): a held-out score, so while the
//! project has no threshold, enough lessons fit one (the ρ-quantile of Reflex's confidence on the ones it got right,
//! kept only if Reflex is right on at least 90% of the answers it would then give). An option that is not a valid
//! domain name, Thai the glossary cannot read, or a ninth domain is not learned.
//!
//! The Reflex screen lists the lessons (`GET /projects/{slug}/corpus/lessons`); removing one (`DELETE …/{id}`)
//! takes its line out of the corpus and the lesson out of the threshold's data, so a wrong panel answer is undone.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::Row;
use tokio::time::Instant;
use ts_rs::TS;

use crate::decide::{for_reflex, quantile_threshold, same};
use crate::hub::Hub;
use crate::model::{Decision, Level, Project, QuestionKind};
use crate::{projects, reflex, repo};

/// Panel members that must give the same answer before it is learned.
pub const MIN_VOTERS: usize = 2;
const FILE: &str = "learned.md";
/// Example lines kept per domain, the newest.
const KEEP: usize = 200;
const LINE_CHARS: usize = 600;
/// Lessons before a threshold is fitted, and the precision it must reach.
const MIN_LESSONS: usize = 16;
const MIN_PRECISION: f64 = 0.9;
const RHO: f64 = 0.3;
/// Reflex restarts this long after a project's last lesson (a burst of lessons is one restart).
const SETTLE: Duration = Duration::from_secs(60);

static PENDING: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

/// One answer the corpus learned, with what Reflex said before it learned it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct CorpusLesson {
    #[ts(type = "number")]
    pub id: i64,
    pub at: DateTime<Utc>,
    #[ts(type = "number")]
    pub decision_id: i64,
    pub domain: String,
    /// `panel` or `owner`.
    pub by: String,
    pub reflex_answer: Option<String>,
    pub confidence: Option<f64>,
    /// Reflex's own answer was this one already.
    pub correct: bool,
    /// The line it added to `<domain>/learned.md`.
    pub example: String,
}

/// The corpus domain for an option: lower case, spaces as `_`; `None` when it is not a valid domain name.
pub fn domain(option: &str) -> Option<String> {
    let d = option.trim().to_lowercase().replace([' ', '/'], "_");
    projects::validate_corpus_path(&format!("{d}/{FILE}")).is_ok().then_some(d).filter(|d| d != repo::ARCHIVE_DIR)
}

/// One example line: the facts on one line, cut to [`LINE_CHARS`].
fn line(state: &str) -> String {
    let flat = state.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("- {}", flat.chars().take(LINE_CHARS).collect::<String>())
}

/// `existing` (a `learned.md`, or nothing) with `example` added: `None` when it is already there.
fn with_example(existing: Option<&str>, domain: &str, example: &str) -> Option<String> {
    let mut lines: Vec<String> = existing.unwrap_or("").lines().filter(|l| l.starts_with("- ")).map(String::from).collect();
    if lines.iter().any(|l| l == example) {
        return None;
    }
    lines.push(example.to_string());
    let skip = lines.len().saturating_sub(KEEP);
    Some(format!(
        "# Learned: {domain}\n\nSituations the fallback panel agreed on, or the owner confirmed, as `{domain}`; MAP adds one line per answer.\n\n{}\n",
        lines[skip..].join("\n")
    ))
}

/// Learns `answer` for decision `d` (`by`: `panel` or `owner`). Never fails the decision: a lesson that cannot be
/// written is logged and skipped.
pub async fn learn(hub: &Hub, project: &Project, d: &Decision, answer: &Value, by: &str) {
    if let Err(err) = try_learn(hub, project, d, answer, by).await {
        tracing::warn!("{}: lesson from decision {} not learned: {err:#}", project.slug, d.id);
    }
}

async fn try_learn(hub: &Hub, project: &Project, d: &Decision, answer: &Value, by: &str) -> Result<()> {
    let (QuestionKind::Choice, Some(option)) = (&d.question.kind, answer.as_str()) else { return Ok(()) };
    let Some(domain) = domain(option) else { return Ok(()) };
    let read = for_reflex(project, &d.state, std::slice::from_ref(&d.question));
    if read.unreadable().is_some() || read.state.trim().is_empty() {
        return Ok(());
    }
    let corpus = hub.corpus_repo(&project.slug);
    let mut domains = projects::corpus_domains(hub, &project.slug).await?;
    let placeholder = domains.contains("example") && repo::read(&corpus, "example/example.md").await == projects::corpus_seed()[0].1;
    if placeholder {
        domains.remove("example");
    }
    domains.insert(domain.clone());
    if domains.len() > reflex::MAX_DOMAINS {
        let why = format!("Reflex did not learn `{domain}`: the corpus already has {} domains; archive one on the Reflex screen", reflex::MAX_DOMAINS);
        hub.event(Level::Warning, "reflex.learn", None, None, format!("{}: {why}", project.slug)).await;
        return Ok(());
    }
    let path = format!("{domain}/{FILE}");
    let example = line(&read.state);
    let Some(content) = with_example(repo::read(&corpus, &path).await.as_deref(), &domain, &example) else { return Ok(()) };
    let mut files = vec![(path, Some(content))];
    if placeholder {
        files.push(("example/example.md".into(), None));
    }
    repo::commit(&corpus, &files, &format!("Learn `{domain}` from decision {} ({by})", d.id)).await?;
    record(hub, project, d, answer, &domain, by, &example).await;
    hub.event(Level::Info, "reflex.learn", None, None, format!("{}: Reflex learned `{domain}` from decision {} ({by})", project.slug, d.id)).await;
    PENDING.lock().unwrap().insert(project.slug.clone(), Instant::now());
    if project.threshold.is_none() {
        fit_threshold(hub, &project.slug).await;
    }
    crate::laya::fit(hub, &project.slug).await;
    Ok(())
}

async fn record(hub: &Hub, project: &Project, d: &Decision, answer: &Value, domain: &str, by: &str, example: &str) {
    let reflex_said = d.reflex_answer.as_str().map(String::from);
    let sql = "INSERT INTO corpus_lessons (project, at, decision_id, domain, by_whom, reflex_answer, confidence, correct, example) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)";
    let _ = sqlx::query(sql)
        .bind(&project.slug)
        .bind(Utc::now())
        .bind(d.id)
        .bind(domain)
        .bind(by)
        .bind(reflex_said)
        .bind(d.confidence)
        .bind(!d.reflex_answer.is_null() && same(&d.reflex_answer, answer))
        .bind(example)
        .execute(&hub.store.pool)
        .await;
}

/// The project's lessons, newest first.
pub async fn lessons(hub: &Hub, slug: &str, limit: i64) -> Result<Vec<CorpusLesson>> {
    let rows = sqlx::query("SELECT * FROM corpus_lessons WHERE project = ? ORDER BY id DESC LIMIT ?").bind(slug).bind(limit).fetch_all(&hub.store.pool).await?;
    Ok(rows
        .iter()
        .map(|r| CorpusLesson {
            id: r.get("id"),
            at: r.get("at"),
            decision_id: r.get("decision_id"),
            domain: r.get("domain"),
            by: r.get("by_whom"),
            reflex_answer: r.get("reflex_answer"),
            confidence: r.get("confidence"),
            correct: r.get("correct"),
            example: r.get("example"),
        })
        .collect())
}

/// `learned` without the line `example`, or `None` when no example is left (the file goes).
fn without_example(learned: &str, example: &str) -> Option<String> {
    let kept: Vec<&str> = learned.lines().filter(|l| *l != example).collect();
    kept.iter().any(|l| l.starts_with("- ")).then(|| format!("{}\n", kept.join("\n")))
}

/// Removes lesson `id`: its line from the corpus (Reflex restarts on it) and its row from the threshold's data.
pub async fn remove(hub: &Hub, slug: &str, id: i64) -> Result<Option<CorpusLesson>> {
    let Some(lesson) = lessons(hub, slug, 1000).await?.into_iter().find(|l| l.id == id) else { return Ok(None) };
    let mut example = lesson.example.clone();
    if example.is_empty() {
        // A lesson from before examples were kept: its line is the decision's facts, as they were learned.
        let project = hub.store.project(slug).await?.context("no such project")?;
        let d = hub.store.decision(slug, lesson.decision_id).await?.context("its decision is gone")?;
        example = line(&for_reflex(&project, &d.state, std::slice::from_ref(&d.question)).state);
    }
    let (corpus, path) = (hub.corpus_repo(slug), format!("{}/{FILE}", lesson.domain));
    if let Some(learned) = repo::read(&corpus, &path).await.filter(|t| t.lines().any(|l| l == example)) {
        repo::commit(&corpus, &[(path, without_example(&learned, &example))], &format!("Unlearn `{}` from decision {}", lesson.domain, lesson.decision_id)).await?;
        PENDING.lock().unwrap().insert(slug.to_string(), Instant::now());
    }
    sqlx::query("DELETE FROM corpus_lessons WHERE project = ? AND id = ?").bind(slug).bind(id).execute(&hub.store.pool).await?;
    hub.event(Level::Info, "reflex.learn", None, None, format!("{slug}: lesson `{}` from decision {} removed", lesson.domain, lesson.decision_id)).await;
    Ok(Some(lesson))
}

type ApiResult<T> = std::result::Result<Json<T>, (StatusCode, String)>;

fn failed(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}"))
}

/// `GET /api/projects/{slug}/corpus/lessons`: the newest 200.
pub async fn list_api(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<Vec<CorpusLesson>> {
    lessons(&hub, &slug, 200).await.map(Json).map_err(failed)
}

/// `DELETE /api/projects/{slug}/corpus/lessons/{id}`.
pub async fn remove_api(State(hub): State<Arc<Hub>>, UrlPath((slug, id)): UrlPath<(String, i64)>) -> ApiResult<CorpusLesson> {
    match remove(&hub, &slug, id).await {
        Ok(Some(l)) => Ok(Json(l)),
        Ok(None) => Err((StatusCode::NOT_FOUND, "no such lesson".into())),
        Err(err) => Err(failed(err)),
    }
}

/// The threshold the held-out lessons support: (confidence, Reflex was right) of the lessons Reflex answered.
pub fn threshold_from(lessons: &[(f64, bool)]) -> Option<f64> {
    if lessons.len() < MIN_LESSONS {
        return None;
    }
    let t = quantile_threshold(lessons.iter().filter(|l| l.1).map(|l| l.0).collect(), RHO)?;
    let passing: Vec<bool> = lessons.iter().filter(|l| l.0 >= t).map(|l| l.1).collect();
    let precision = passing.iter().filter(|c| **c).count() as f64 / passing.len().max(1) as f64;
    (precision >= MIN_PRECISION).then_some(t)
}

/// Fits and saves a threshold from the project's lessons, when they support one.
async fn fit_threshold(hub: &Hub, slug: &str) {
    let rows = sqlx::query("SELECT confidence, correct FROM corpus_lessons WHERE project = ? AND reflex_answer IS NOT NULL AND confidence IS NOT NULL ORDER BY id DESC LIMIT 500")
        .bind(slug)
        .fetch_all(&hub.store.pool)
        .await
        .unwrap_or_default();
    let lessons: Vec<(f64, bool)> = rows.iter().map(|r| (r.get("confidence"), r.get("correct"))).collect();
    let Some(t) = threshold_from(&lessons) else { return };
    let Ok(Some(mut p)) = hub.store.project(slug).await else { return };
    if p.threshold.is_some() {
        return;
    }
    p.threshold = Some(t);
    if hub.put_project(&p).await.is_ok() {
        let msg = format!("{slug}: threshold {} fitted from {} lessons; Reflex now answers above it", crate::decide::fmt_conf(t), lessons.len());
        hub.event(Level::Success, "reflex.learn", None, None, msg).await;
    }
}

/// Restarts each project's Reflex once its lessons have settled.
pub async fn run(hub: Arc<Hub>) {
    loop {
        tokio::time::sleep(Duration::from_secs(15)).await;
        let due: Vec<String> = {
            let mut pending = PENDING.lock().unwrap();
            let due: Vec<String> = pending.iter().filter(|(_, at)| at.elapsed() >= SETTLE).map(|(s, _)| s.clone()).collect();
            pending.retain(|s, _| !due.contains(s));
            due
        };
        for slug in due {
            if let Err(err) = hub.restart_reflex(&slug).await {
                tracing::warn!("reflex for {slug} after lessons: {err:#}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_become_domains() {
        assert_eq!(domain("Light").as_deref(), Some("light"));
        assert_eq!(domain("needs review").as_deref(), Some("needs_review"));
        assert_eq!(domain("ด่วน"), None);
        assert_eq!(domain(""), None);
    }

    #[test]
    fn examples_are_added_once_and_capped() {
        let first = with_example(None, "bug", "- the save button crashes").unwrap();
        assert!(first.starts_with("# Learned: bug") && first.contains("- the save button crashes"));
        assert!(with_example(Some(&first), "bug", "- the save button crashes").is_none());
        let mut doc = first;
        for i in 0..KEEP + 5 {
            doc = with_example(Some(&doc), "bug", &format!("- case {i}")).unwrap();
        }
        assert_eq!(doc.lines().filter(|l| l.starts_with("- ")).count(), KEEP);
        assert!(!doc.contains("save button") && doc.contains(&format!("- case {}", KEEP + 4)));
        assert_eq!(line("a\n  b\tc"), "- a b c");
    }

    #[test]
    fn a_threshold_needs_enough_lessons_and_precision() {
        let good: Vec<(f64, bool)> = (0..20).map(|i| (0.5 + i as f64 * 0.02, true)).chain([(0.1, false), (0.2, false)]).collect();
        let t = threshold_from(&good).unwrap();
        assert!(t > 0.2, "{t}");
        assert_eq!(threshold_from(&good[..10]), None);
        let noisy: Vec<(f64, bool)> = (0..20).map(|i| (0.9, i % 2 == 0)).collect();
        assert_eq!(threshold_from(&noisy), None, "right only half the time above the threshold");
    }

    #[tokio::test]
    async fn a_trusted_answer_replaces_the_placeholder_with_a_lesson() {
        use crate::model::{Lane, ProjectSettings, ProjectSource, Question, SourceKind};
        let dir = tempfile::tempdir().unwrap();
        let text = format!("[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\n", dir.path().display());
        let config = crate::config::Config::parse(&text, std::path::Path::new("/")).unwrap();
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        let source = ProjectSource { kind: SourceKind::Empty, location: Some(dir.path().join("p").display().to_string()), branch: None, remote: None };
        let new = projects::NewProject { name: "Shop".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let (p, _) = projects::create_with(&hub, new, ProjectSettings { source, ..Default::default() }).await.unwrap();
        let q = Question { id: "q".into(), kind: QuestionKind::Choice, prompt: "Bug or feature?".into(), options: Some(vec!["bug".into(), "feature".into()]), criteria: None };
        let d = Decision {
            id: 7, project: p.slug.clone(), at: Utc::now(), state: "The save button\ncrashes the page".into(), question: q,
            answer: Value::Null, lane: Lane::Llm, confidence: Some(0.4), reflex_answer: Value::from("feature"), ms: 0, error: None, feedback: None, laya: None,
        };
        learn(&hub, &p, &d, &Value::from("bug"), "panel").await;
        learn(&hub, &p, &d, &Value::from("bug"), "panel").await;
        let domains: Vec<String> = projects::corpus_domains(&hub, &p.slug).await.unwrap().into_iter().collect();
        assert_eq!(domains, ["bug"], "the placeholder is gone");
        let doc = repo::read(&hub.corpus_repo(&p.slug), "bug/learned.md").await.unwrap();
        assert_eq!(doc.matches("- The save button crashes the page").count(), 1, "{doc}");
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM corpus_lessons WHERE correct = 0").fetch_one(&hub.store.pool).await.unwrap();
        assert_eq!(n, 1, "one lesson, Reflex had said feature");
        assert!(PENDING.lock().unwrap().contains_key(&p.slug));
        let listed = lessons(&hub, &p.slug, 10).await.unwrap();
        assert_eq!((listed.len(), listed[0].example.as_str()), (1, "- The save button crashes the page"));
        let other = Decision { id: 8, state: "Checkout totals are wrong".into(), ..d.clone() };
        learn(&hub, &p, &other, &Value::from("bug"), "owner").await;
        remove(&hub, &p.slug, listed[0].id).await.unwrap().unwrap();
        let doc = repo::read(&hub.corpus_repo(&p.slug), "bug/learned.md").await.unwrap();
        assert!(!doc.contains("save button") && doc.contains("- Checkout totals are wrong"), "{doc}");
        let last = lessons(&hub, &p.slug, 10).await.unwrap()[0].id;
        remove(&hub, &p.slug, last).await.unwrap().unwrap();
        assert!(repo::read(&hub.corpus_repo(&p.slug), "bug/learned.md").await.is_none(), "the last example takes the file");
        assert!(remove(&hub, &p.slug, last).await.unwrap().is_none());
    }
}
