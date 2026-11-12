//! Reflex reads every message typed into a lane (the dashboard's Ask, the manager panel, Telegram's `/ask`,
//! `map-tell --tell ask`) before the agent does: what the sender wants, as one of [`OPTIONS`]. A confident read goes in
//! front of the message as a short tag the agent can use or ignore; an abstain, a Reflex that is not running or a lane
//! of no project leaves the message as it was. Every read is logged as a decision of the lane's project, so its
//! Reflex screen shows them and confirming one teaches the calibrator.
//!
//! Like the gates (`gates.rs`), the reads have a Reflex of their own, shared by every project, one corpus domain per
//! answer, seeded from `templates/chat/` into `<data_dir>/chat/corpus.git`. Thai goes through the project's glossary.
//! A message is a few words, so each phrase of a template is a file of its own: against whole paragraphs Reflex
//! abstains on nearly every message. Reflex's confidence does not tell its right reads from its wrong ones here, the
//! gap between its two likeliest answers does: a read is used from [`MIN_MARGIN`].

use std::path::PathBuf;

use anyhow::Result;
use chrono::Utc;

use crate::config::LaneConfig;
use crate::decide::{self, Route, Target};
use crate::hub::Hub;
use crate::model::{Lane, Level, Question, QuestionKind, ReflexState, ReflexStatus, TellVerb};

/// The chat Reflex's key in the Reflex manager (never a project slug: those cannot start with `_`).
pub const KEY: &str = "_chat";
pub const INTENT: &str = "intent";
pub const OPTIONS: [&str; 4] = ["status", "task", "reply", "chat"];

/// Each answer's template: a title, one sentence on what it means, then its phrases, comma-separated.
const TEMPLATES: [(&str, &str); 4] = [
    ("status", include_str!("../templates/chat/status.md")),
    ("task", include_str!("../templates/chat/task.md")),
    ("reply", include_str!("../templates/chat/reply.md")),
    ("chat", include_str!("../templates/chat/chat.md")),
];

/// The probability gap between Reflex's two likeliest answers from which its read is used (wrong reads of a test set
/// all fell below 0.016).
pub const MIN_MARGIN: f64 = 0.02;

/// The seeded corpus: `<answer>/<n>.md`, the sentence first, then one phrase per file.
pub fn seed() -> Vec<(String, Option<String>)> {
    let mut files = vec![];
    for (answer, text) in TEMPLATES {
        let mut parts = text.splitn(3, "\n\n").skip(1);
        let (sentence, phrases) = (parts.next().unwrap_or("").trim(), parts.next().unwrap_or(""));
        let phrases = phrases.split([',', '.']).map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|p| !p.is_empty());
        for (i, t) in std::iter::once(sentence.to_string()).chain(phrases).enumerate() {
            files.push((format!("{answer}/{i:03}.md"), Some(format!("{t}\n"))));
        }
    }
    files
}

/// The tag's words for each answer: a reading, never an order.
fn reads_as(answer: &str) -> Option<&'static str> {
    Some(match answer {
        "status" => "a status question",
        "task" => "a work request",
        "reply" => "a reply to your last question",
        "chat" => "small talk",
        _ => return None,
    })
}

pub fn question() -> Question {
    Question {
        id: INTENT.into(),
        kind: QuestionKind::Choice,
        prompt: "What does the owner want from this message: an answer about how things stand, a change made, a reply to the agent's question, or small talk?".into(),
        options: Some(OPTIONS.iter().map(|o| o.to_string()).collect()),
        criteria: None,
    }
}

/// Two below the projects' first port (the gates take the one just below).
pub fn port(hub: &Hub) -> u16 {
    hub.config.reflex.port_base.saturating_sub(2)
}

fn dir(hub: &Hub) -> PathBuf {
    hub.config.server.data_dir.join("chat")
}

/// Seeds the corpus on first use, then (re)starts the chat Reflex.
pub async fn restart(hub: &Hub) -> Result<ReflexStatus> {
    let dir = dir(hub);
    let repo_path = dir.join("corpus.git");
    if !repo_path.exists() {
        std::fs::create_dir_all(&dir)?;
        crate::repo::init_bare(&repo_path).await?;
        crate::repo::commit(&repo_path, &seed(), "Seed the chat corpus").await?;
    }
    let (live, log) = (dir.join("corpus-live"), dir.join("reflex.log"));
    let paths = crate::reflex::Paths { corpus_repo: &repo_path, corpus_live: &live, log: &log };
    let status = hub.reflex.restart(&hub.config.reflex.binary, KEY, port(hub), paths, false).await;
    let level = if status.state == ReflexState::Error { Level::Warning } else { Level::Info };
    hub.event(level, "reflex.chat", None, None, format!("chat Reflex {:?} {}", status.state, status.message.lines().next().unwrap_or(""))).await;
    Ok(status)
}

/// The note a tell types: one line, at most 4000 chars, with Reflex's reading in front when it has a confident one.
pub async fn hinted(hub: &Hub, cfg: &LaneConfig, verb: TellVerb, text: Option<&str>) -> String {
    let line: String = crate::lanes::one_line(text.unwrap_or("")).chars().take(4000).collect();
    if verb != TellVerb::Ask || line.is_empty() {
        return line;
    }
    match read(hub, cfg, &line).await {
        Some(tag) => format!("[MAP Reflex reads this as {tag}] {line}").chars().take(4000).collect(),
        None => line,
    }
}

/// Reflex's reading of `text`, or `None` (not running, no project, abstained).
async fn read(hub: &Hub, cfg: &LaneConfig, text: &str) -> Option<&'static str> {
    if hub.reflex.status(KEY).state != ReflexState::Running {
        return None;
    }
    let slug = hub.lanes.project_of(cfg)?;
    let project = hub.store.project(&slug).await.ok().flatten()?;
    let target = Target { port: port(hub), threshold: None };
    let started = Utc::now();
    let resp = match decide::decide_at(hub, &project, target, text, &[question()], Route::Reflex).await {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!("chat reflex for {}: {err:#}", cfg.name);
            return None;
        }
    };
    let a = resp.answers.into_iter().next()?;
    tracing::debug!("chat reflex for {} in {} ms: {:?}", cfg.name, (Utc::now() - started).num_milliseconds(), a.answer);
    if a.lane != Lane::Reflex || margin(a.probabilities.as_deref().unwrap_or_default()) < MIN_MARGIN {
        return None;
    }
    a.answer.as_str().and_then(reads_as)
}

/// The gap between the two highest probabilities (0 with fewer than two).
fn margin(p: &[f64]) -> f64 {
    let mut p = p.to_vec();
    p.sort_by(|a, b| b.total_cmp(a));
    if p.len() < 2 { 0.0 } else { p[0] - p[1] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Arc;

    use serde_json::{Value, json};

    use crate::config::Config;
    use crate::gates::fake;
    use crate::model::{ProjectSettings, ProjectSource, SourceKind};
    use crate::projects::{NewProject, create_with};
    use crate::store::Store;
    use crate::terminal::fake::FakeTerminal;

    #[test]
    fn every_option_is_a_seeded_domain_of_one_phrase_per_file() {
        let files = seed();
        for o in OPTIONS {
            let mine: Vec<&str> = files.iter().filter(|(p, _)| p.starts_with(&format!("{o}/"))).filter_map(|(_, t)| t.as_deref()).collect();
            assert!(mine.len() > 20, "{o}: {}", mine.len());
            assert!(mine[0].split_whitespace().count() > 5, "{o} starts with its sentence: {}", mine[0]);
            assert!(mine[1..].iter().all(|t| !t.contains(',') && !t.starts_with('#') && t.split_whitespace().count() <= 8), "{o}: {mine:?}");
            assert!(reads_as(o).is_some(), "{o}");
        }
        assert!(files.iter().any(|(p, t)| p.starts_with("status/") && t.as_deref() == Some("why did the task fail\n")));
        assert_eq!(TEMPLATES.map(|(o, _)| o), OPTIONS);
        assert!(OPTIONS.len() <= crate::reflex::MAX_DOMAINS);
        assert_eq!(margin(&[0.2, 0.5, 0.3]), 0.2);
        assert_eq!(margin(&[1.0]), 0.0);
    }

    /// A hub whose chat Reflex is a fake answering `outcome`, a project `support` and its lane `dev`.
    async fn hub_with(outcome: Value, probabilities: [f64; 4]) -> (Arc<Hub>, tempfile::TempDir, fake::FakeReflex) {
        let mut a = fake::answer(INTENT, outcome, 0.02);
        a["probabilities"] = json!(probabilities);
        let rx = fake::start(vec![a]).await;
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\nport_base = {1}\n\
             [[lanes]]\nname = \"dev\"\nproject = \"support\"\nsession_match = [\"dev\"]\n[[lanes]]\nname = \"loose\"\nsession_match = [\"loose\"]\n",
            dir.path().display(),
            rx.port + 2
        );
        let hub = Hub::new(Config::parse(&text, Path::new("/")).unwrap(), Store::memory().await.unwrap(), Arc::new(FakeTerminal::default()));
        let source = ProjectSource { kind: SourceKind::Empty, location: Some(dir.path().join("support").display().to_string()), branch: None, remote: None };
        let new = NewProject { name: "support".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        create_with(&hub, new, ProjectSettings { source, ..Default::default() }).await.unwrap();
        hub.reflex.pretend(KEY, ReflexState::Running);
        (hub, dir, rx)
    }

    #[tokio::test]
    async fn a_confident_read_is_tagged_and_logged() {
        let (hub, _d, rx) = hub_with(json!({"choice": {"index": 0}}), [0.4, 0.3, 0.2, 0.1]).await;
        let cfg = hub.lanes.config("dev").unwrap();
        let note = hinted(&hub, &cfg, TellVerb::Ask, Some("why did\n task 12 fail?")).await;
        assert_eq!(note, "[MAP Reflex reads this as a status question] why did task 12 fail?");
        assert_eq!(rx.bodies.lock().unwrap().len(), 1);
        let logged = hub.store.decisions("support", 10).await.unwrap();
        assert_eq!(logged.len(), 1);
        assert_eq!(logged[0].question.id, INTENT);
    }

    #[tokio::test]
    async fn a_read_too_close_to_the_runner_up_is_left_out() {
        let (hub, _d, rx) = hub_with(json!({"choice": {"index": 0}}), [0.30, 0.29, 0.21, 0.20]).await;
        let cfg = hub.lanes.config("dev").unwrap();
        assert_eq!(hinted(&hub, &cfg, TellVerb::Ask, Some("is the build passing")).await, "is the build passing");
        assert_eq!(rx.bodies.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_abstain_a_clear_or_an_empty_note_types_the_text_as_it_is() {
        let (hub, _d, rx) = hub_with(Value::Null, [0.4, 0.3, 0.2, 0.1]).await;
        let cfg = hub.lanes.config("dev").unwrap();
        assert_eq!(hinted(&hub, &cfg, TellVerb::Ask, Some("hmm")).await, "hmm");
        assert_eq!(hinted(&hub, &cfg, TellVerb::Clear, Some("card done")).await, "card done");
        assert_eq!(hinted(&hub, &cfg, TellVerb::Ask, None).await, "");
        assert_eq!(rx.bodies.lock().unwrap().len(), 1, "only the Ask with text is read");
        assert_eq!(hinted(&hub, &hub.lanes.config("loose").unwrap(), TellVerb::Ask, Some("why?")).await, "why?");
        hub.reflex.pretend(KEY, ReflexState::Stopped);
        assert_eq!(hinted(&hub, &cfg, TellVerb::Ask, Some("why?")).await, "why?");
        assert_eq!(rx.bodies.lock().unwrap().len(), 1);
    }
}
