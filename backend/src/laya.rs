//! Laya, Reflex's model lane, as a project's second stage: Reflex's default lane answers from corpus words alone and
//! is often unsure, laya (a ModernBERT-large port, run locally by the same Reflex process) reads the whole question.
//! So a question the default lane does not settle is asked of laya (`X-Reflex-Lane: laya`), and only what laya does
//! not settle either goes to the fallback: default lane → laya → fallback.
//!
//! Off by default: laya holds about 1.7 GB of memory while on, and its first start downloads the open weights
//! (about 800 MB, to `~/.cache/riir-reflex/laya`). `PUT /api/projects/{slug}/laya` turns it on for one project and
//! restarts that project's Reflex with `RIIR_REFLEX_LAYA=1`; the gates' and the chat's Reflex never load it.
//!
//! Laya has a threshold of its own (`settings.reflex_laya.threshold`). Without one it is in shadow: its answer is
//! logged with each decision (`decisions.laya`) and the fallback answers. The threshold is fitted from the project's
//! lessons (`corpus_learn.rs`), which are held out for laya since it does not read the corpus: the lessons made before
//! laya was on are scored first, then the lowest confidence is taken at which laya was right on at least 90% of the
//! lessons it would have answered, counting one more right and one more wrong answer than there were, so a few lucky
//! answers fit nothing.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::StatusCode;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::json;
use sqlx::Row;
use ts_rs::TS;

use crate::decide::{decode, fmt_conf, for_reflex};
use crate::hub::Hub;
use crate::model::{LayaRead, Level, Project, Question};

const LANE: &str = "laya";
/// The feature `reflex --version` lists when the build has the lane.
const FEATURE: &str = "laya-riir";
/// Scored lessons before a threshold is fitted, and the precision it must reach.
const MIN_LESSONS: usize = 16;
const MIN_PRECISION: f64 = 0.9;
/// How long a start waits for laya to load (the first one downloads the weights).
const LOAD_WAIT: Duration = Duration::from_secs(20 * 60);

/// Asks laya one question (`wire`: as Reflex reads it, Thai glossed). `None` when laya did not answer it: off, still
/// loading, or Reflex not reachable.
pub async fn ask(hub: &Hub, port: u16, state: &str, wire: &Question, q: &Question) -> Option<LayaRead> {
    let reply = hub.reflex.post_lane(port, "/decide", &json!({ "state": state, "questions": [wire] }), Some(LANE)).await.ok()?;
    if reply["routing"]["lane"] != LANE {
        return None;
    }
    let a = reply["answers"].as_array()?.first()?;
    Some(LayaRead { answer: decode(q, &a["outcome"]), confidence: a["confidence"].as_f64()? })
}

/// Laya's answer is used: it gave one, at or above its threshold.
pub fn passes(read: &LayaRead, threshold: Option<f64>) -> bool {
    !read.answer.is_null() && threshold.is_some_and(|t| read.confidence >= t)
}

/// Laya's part of a decision's reason, in one sentence.
pub fn reason(read: &LayaRead, threshold: Option<f64>) -> String {
    let c = fmt_conf(read.confidence);
    match threshold {
        _ if read.answer.is_null() => "Laya abstained too.".into(),
        None => format!("Laya scored {c} in shadow: it has no threshold yet."),
        Some(t) if read.confidence >= t => format!("Laya answered instead, at confidence {c}, at or above its threshold {}: no LLM call.", fmt_conf(t)),
        Some(t) => format!("Laya's confidence {c} is below its threshold {} too.", fmt_conf(t)),
    }
}

/// The threshold the scored lessons support: (laya's confidence, laya was right) of the lessons laya answered.
pub fn threshold_from(scored: &[(f64, bool)]) -> Option<f64> {
    if scored.len() < MIN_LESSONS {
        return None;
    }
    let mut rows = scored.to_vec();
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (mut right, mut best) = (0usize, None);
    for (i, (confidence, correct)) in rows.iter().enumerate() {
        right += usize::from(*correct);
        // Lessons of one confidence pass or fail together.
        let last_of_its_confidence = rows.get(i + 1).is_none_or(|next| next.0 < *confidence);
        if last_of_its_confidence && (right + 1) as f64 / (i + 3) as f64 >= MIN_PRECISION {
            best = Some(*confidence);
        }
    }
    best
}

/// Laya's lane of the project's Reflex: `ready`, `loading`, `off`, or `stopped` when Reflex does not answer.
async fn lane_state(hub: &Hub, project: &Project) -> String {
    hub.reflex.lane_state(project.port, LANE).await.unwrap_or_else(|| "stopped".into())
}

/// (laya's confidence, laya was right) of the project's lessons laya answered, newest first.
async fn scored(hub: &Hub, slug: &str) -> Vec<(f64, bool)> {
    let sql = "SELECT l.domain, d.laya FROM corpus_lessons l JOIN decisions d ON d.id = l.decision_id WHERE l.project = ? AND d.laya IS NOT NULL ORDER BY l.id DESC LIMIT 500";
    let rows = sqlx::query(sql).bind(slug).fetch_all(&hub.store.pool).await.unwrap_or_default();
    rows.iter()
        .filter_map(|r| {
            let read: LayaRead = serde_json::from_str(r.get("laya")).ok()?;
            let said = crate::corpus_learn::domain(read.answer.as_str()?);
            Some((read.confidence, said.as_deref() == Some(r.get::<String, _>("domain").as_str())))
        })
        .collect()
}

/// Asks laya the lessons it has not seen (those learned before it was on), so they count toward its threshold.
async fn score_lessons(hub: &Hub, project: &Project) {
    let sql = "SELECT d.id FROM corpus_lessons l JOIN decisions d ON d.id = l.decision_id WHERE l.project = ? AND d.laya IS NULL ORDER BY l.id DESC LIMIT 200";
    let ids: Vec<i64> = sqlx::query(sql).bind(&project.slug).fetch_all(&hub.store.pool).await.unwrap_or_default().iter().map(|r| r.get("id")).collect();
    for id in ids {
        let Ok(Some(d)) = hub.store.decision(&project.slug, id).await else { continue };
        let read = for_reflex(project, &d.state, std::slice::from_ref(&d.question));
        if read.unreadable().is_some() {
            continue;
        }
        let Some(said) = ask(hub, project.port, &read.state, &read.questions[0], &d.question).await else { return };
        if let Err(err) = hub.store.set_decision_laya(id, &said).await {
            tracing::warn!("{}: laya's answer to decision {id} not recorded: {err:#}", project.slug);
        }
    }
}

/// Fits and saves laya's threshold from the project's lessons, when laya is on without one and they support one.
pub async fn fit(hub: &Hub, slug: &str) {
    let Ok(Some(project)) = hub.store.project(slug).await else { return };
    let laya = project.settings.reflex_laya;
    if !laya.enabled || laya.threshold.is_some() || lane_state(hub, &project).await != "ready" {
        return;
    }
    score_lessons(hub, &project).await;
    let lessons = scored(hub, slug).await;
    let Some(t) = threshold_from(&lessons) else { return };
    // Read again: the owner may have set a threshold, or turned laya off, while the lessons were scored.
    let Ok(Some(mut p)) = hub.store.project(slug).await else { return };
    if !p.settings.reflex_laya.enabled || p.settings.reflex_laya.threshold.is_some() {
        return;
    }
    p.settings.reflex_laya.threshold = Some(t);
    if hub.put_project(&p).await.is_ok() {
        let msg = format!("{slug}: laya's threshold {} fitted from {} lessons; laya now answers above it", fmt_conf(t), lessons.len());
        hub.event(Level::Success, "reflex.laya", None, None, msg).await;
    }
}

/// After a (re)start with laya on: waits for the lane to load, then fits its threshold.
async fn fit_when_ready(hub: Arc<Hub>, slug: String) {
    let started = tokio::time::Instant::now();
    while started.elapsed() < LOAD_WAIT {
        let Ok(Some(project)) = hub.store.project(&slug).await else { return };
        match lane_state(&hub, &project).await.as_str() {
            "ready" => {
                hub.announce_project(&slug).await;
                return fit(&hub, &slug).await;
            }
            "loading" => tokio::time::sleep(Duration::from_secs(3)).await,
            _ => return,
        }
    }
}

/// Laya for one project, as the Reflex screen shows it.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct LayaInfo {
    pub enabled: bool,
    pub threshold: Option<f64>,
    /// The installed Reflex has the lane (`reflex --version` lists `laya-riir`).
    pub available: bool,
    /// `ready`, `loading` (the first start downloads the weights), `off`, or `stopped` when Reflex is not running.
    pub state: String,
    /// Lessons laya answered, and how many of those it got right: what its threshold is fitted from.
    #[ts(type = "number")]
    pub lessons: usize,
    #[ts(type = "number")]
    pub right: usize,
    /// Lessons a threshold needs.
    #[ts(type = "number")]
    pub lessons_needed: usize,
}

async fn info(hub: &Hub, project: &Project) -> LayaInfo {
    let lessons = scored(hub, &project.slug).await;
    LayaInfo {
        enabled: project.settings.reflex_laya.enabled,
        threshold: project.settings.reflex_laya.threshold,
        available: crate::reflex::has_feature(&hub.config.reflex.binary, FEATURE),
        state: lane_state(hub, project).await,
        lessons: lessons.len(),
        right: lessons.iter().filter(|l| l.1).count(),
        lessons_needed: MIN_LESSONS,
    }
}

type ApiResult<T> = std::result::Result<Json<T>, (StatusCode, String)>;

fn failed(err: anyhow::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{err:#}"))
}

async fn load(hub: &Hub, slug: &str) -> std::result::Result<Project, (StatusCode, String)> {
    hub.store.project(slug).await.map_err(failed)?.ok_or((StatusCode::NOT_FOUND, "no such project".into()))
}

/// `GET /api/projects/{slug}/laya`.
pub async fn get_api(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>) -> ApiResult<LayaInfo> {
    Ok(Json(info(&hub, &load(&hub, &slug).await?).await))
}

/// Tells a missing field (keep) from an explicit `null` (clear).
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Deserialize)]
pub struct LayaBody {
    enabled: Option<bool>,
    /// A number sets laya's threshold; `null` clears it (shadow, until the lessons fit one again).
    #[serde(default, deserialize_with = "present")]
    threshold: Option<Option<f64>>,
}

/// `PUT /api/projects/{slug}/laya`: turns laya on or off (the project's Reflex restarts) and sets its threshold.
pub async fn put_api(State(hub): State<Arc<Hub>>, UrlPath(slug): UrlPath<String>, Json(b): Json<LayaBody>) -> ApiResult<LayaInfo> {
    let mut p = load(&hub, &slug).await?;
    let before = p.settings.reflex_laya;
    if let Some(on) = b.enabled {
        if on && !crate::reflex::has_feature(&hub.config.reflex.binary, FEATURE) {
            return Err((StatusCode::BAD_REQUEST, format!("the installed Reflex has no laya lane (`reflex --version` does not list {FEATURE}); update Reflex")));
        }
        p.settings.reflex_laya.enabled = on;
    }
    if let Some(t) = b.threshold {
        if t.is_some_and(|t| !t.is_finite() || !(0.0..=1.0).contains(&t)) {
            return Err((StatusCode::BAD_REQUEST, "threshold is a number from 0 to 1, or null".into()));
        }
        p.settings.reflex_laya.threshold = t;
    }
    let after = p.settings.reflex_laya;
    hub.put_project(&p).await.map_err(failed)?;
    if after.enabled != before.enabled {
        let what = if after.enabled { "on: questions Reflex is unsure of go to laya before the fallback" } else { "off" };
        hub.event(Level::Info, "reflex.laya", None, None, format!("{slug}: laya {what}")).await;
        hub.restart_reflex(&slug).await.map_err(failed)?;
    }
    if after.enabled && (!before.enabled || after.threshold.is_none()) {
        tokio::spawn(fit_when_ready(hub.clone(), slug.clone()));
    }
    Ok(Json(info(&hub, &p).await))
}

/// At startup: laya's threshold for the projects that have laya on without one (their Reflex was just started).
pub async fn run(hub: Arc<Hub>) {
    tokio::time::sleep(Duration::from_secs(30)).await;
    for p in hub.store.projects().await.unwrap_or_default() {
        if p.settings.reflex_laya.enabled && p.settings.reflex_laya.threshold.is_none() {
            tokio::spawn(fit_when_ready(hub.clone(), p.slug));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn laya_passes_only_with_an_answer_at_its_threshold() {
        let said = LayaRead { answer: Value::from("bug"), confidence: 0.5 };
        assert!(passes(&said, Some(0.5)) && !passes(&said, Some(0.6)) && !passes(&said, None));
        assert!(!passes(&LayaRead { answer: Value::Null, confidence: 0.9 }, Some(0.1)));
        assert!(reason(&said, Some(0.4)).contains("answered instead") && reason(&said, None).contains("shadow"));
    }

    async fn hub() -> (Arc<Hub>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!("[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\n", dir.path().display());
        let config = crate::config::Config::parse(&text, std::path::Path::new("/")).unwrap();
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        (hub, dir)
    }

    fn kind() -> Question {
        Question { id: "kind".into(), kind: crate::model::QuestionKind::Choice, prompt: "Kind".into(), options: Some(vec!["bug".into(), "feature".into()]), criteria: None }
    }

    #[tokio::test]
    async fn laya_answers_what_reflex_does_not_settle() {
        use crate::decide::{Route, decide};
        use crate::gates::fake;
        use crate::model::{Lane, ProjectSettings, ProjectSource, SourceKind};
        let (hub, dir) = hub().await;
        let source = ProjectSource { kind: SourceKind::Empty, location: Some(dir.path().join("shop").display().to_string()), branch: None, remote: None };
        let new = crate::projects::NewProject { name: "shop".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let (p, _) = crate::projects::create_with(&hub, new, ProjectSettings { source, ..Default::default() }).await.unwrap();
        let slug = p.slug;
        // Reflex says "bug" at 0.2, laya "feature" at 0.6.
        let reflex = fake::start_lanes(vec![fake::answer("kind", json!({"choice": {"index": 0}}), 0.2)], Some(vec![fake::answer("kind", json!({"choice": {"index": 1}}), 0.6)])).await;
        fake::attach(&hub, &slug, reflex.port, Some(0.5)).await;
        let set = |laya: crate::model::ReflexLaya| {
            let (hub, slug) = (hub.clone(), slug.clone());
            async move {
                let mut p = hub.store.project(&slug).await.unwrap().unwrap();
                p.settings.reflex_laya = laya;
                hub.put_project(&p).await.unwrap();
                p
            }
        };
        let ask = |p: Project, route: Route| {
            let hub = hub.clone();
            async move { decide(&hub, &p, "Task: add a button", &[kind()], route).await.unwrap().answers.remove(0) }
        };

        // Off: laya is not asked.
        let p = set(crate::model::ReflexLaya { enabled: false, threshold: Some(0.5) }).await;
        let a = ask(p, Route::Auto).await;
        assert!(a.laya.is_none() && a.lane == Lane::Abstain, "{a:?}");
        assert_eq!(reflex.bodies.lock().unwrap().len(), 1);

        // On without a threshold: asked, logged with the decision, not used.
        let p = set(crate::model::ReflexLaya { enabled: true, threshold: None }).await;
        let a = ask(p, Route::Auto).await;
        assert_eq!(a.laya.as_ref().map(|l| (l.answer.clone(), l.confidence)), Some((json!("feature"), 0.6)));
        assert!(a.lane == Lane::Abstain && a.reason.contains("shadow"), "{a:?}");
        let logged = hub.store.decision(&slug, a.decision_id).await.unwrap().unwrap();
        assert_eq!(logged.laya.map(|l| l.answer), Some(json!("feature")));

        // Above laya's threshold: laya's answer, no fallback. Below it: not.
        let p = set(crate::model::ReflexLaya { enabled: true, threshold: Some(0.5) }).await;
        let a = ask(p.clone(), Route::Auto).await;
        assert!(a.answer == json!("feature") && a.lane == Lane::Reflex && a.reflex_answer == json!("bug"), "{a:?}");
        assert!(a.reason.contains("Laya answered instead"), "{}", a.reason);
        // The caller asked for Reflex's own answer: laya is left out.
        assert!(ask(p, Route::Reflex).await.laya.is_none());
        let p = set(crate::model::ReflexLaya { enabled: true, threshold: Some(0.7) }).await;
        let a = ask(p.clone(), Route::Auto).await;
        assert!(a.lane == Lane::Abstain && a.reason.contains("below its threshold"), "{a:?}");

        // Reflex itself is sure: laya is not asked.
        fake::attach(&hub, &slug, reflex.port, Some(0.1)).await;
        let p = hub.store.project(&slug).await.unwrap().unwrap();
        let before = reflex.bodies.lock().unwrap().len();
        let a = ask(p, Route::Auto).await;
        assert!(a.laya.is_none() && a.answer == json!("bug"), "{a:?}");
        assert_eq!(reflex.bodies.lock().unwrap().len(), before + 1);
    }

    #[test]
    fn the_threshold_is_the_lowest_confidence_that_keeps_laya_right() {
        // Too few lessons, however right.
        assert_eq!(threshold_from(&[(0.9, true); 15]), None);
        // The 20 lessons of a real project: right on the top 8, then one wrong among every few.
        let wrong_at = [8, 11, 12, 16, 17, 18, 19];
        let lessons: Vec<(f64, bool)> = (0..20).map(|i| (0.9 - i as f64 * 0.04, !wrong_at.contains(&i))).collect();
        assert_eq!(threshold_from(&lessons), Some(lessons[7].0));
        // Never right enough: no threshold.
        let noisy: Vec<(f64, bool)> = (0..20).map(|i| (0.9 - i as f64 * 0.04, i % 2 == 0)).collect();
        assert_eq!(threshold_from(&noisy), None);
        // A handful of right answers above many wrong ones is not enough.
        let few: Vec<(f64, bool)> = (0..20).map(|i| (0.9 - i as f64 * 0.04, i < 5)).collect();
        assert_eq!(threshold_from(&few), None);
        // Equal confidences pass or fail together.
        let tied: Vec<(f64, bool)> = (0..20).map(|i| (if i < 10 { 0.8 } else { 0.2 }, i < 9)).collect();
        assert_eq!(threshold_from(&tied), None);
    }
}
