//! Reflex gates: two decisions the orchestrator repeats for every task, asked of the project's Reflex first and
//! never of an LLM. `needs_audit` can skip a whole LLM audit; `tier` picks a lighter or heavier model. Whenever
//! Reflex is not running, abstains or is below the project's threshold, the orchestrator does what it did before.
//! The answers come from the corpus's `orchestration` domain, seeded with `SEED` when a gate is turned on.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::decide::{self, Route, fmt_conf};
use crate::hub::Hub;
use crate::model::{CheckLine, Lane, Level, Question, QuestionKind, ReflexGates, ReflexState, Task};

/// The corpus domain the gates' answers come from.
pub const DOMAIN: &str = "orchestration";
pub const SEED_PATH: &str = "orchestration/gates.md";
pub const NEEDS_AUDIT: &str = "needs_audit";
pub const TIER: &str = "tier";
pub const TIERS: [&str; 3] = ["light", "standard", "heavy"];

/// The seed for the `orchestration` domain: runbook prose the project edits to its own rules.
pub const SEED: &str = include_str!("../templates/orchestration-gates.md");

pub fn needs_audit_question() -> Question {
    Question {
        id: NEEDS_AUDIT.into(),
        kind: QuestionKind::Noul,
        prompt: "Does this change need a careful LLM audit before it is accepted?".into(),
        options: None,
        criteria: None,
    }
}

pub fn tier_question() -> Question {
    Question {
        id: TIER.into(),
        kind: QuestionKind::Choice,
        prompt: "How heavy is this task: light, standard or heavy?".into(),
        options: Some(TIERS.iter().map(|t| t.to_string()).collect()),
        criteria: None,
    }
}

/// The questions of the gates that are on, for the project's saved questions (the Reflex screen calibrates them).
pub fn questions(g: &ReflexGates) -> Vec<Question> {
    let mut out = vec![];
    if g.audit {
        out.push(needs_audit_question());
    }
    if g.tier {
        out.push(tier_question());
    }
    out
}

pub fn names(g: &ReflexGates) -> Vec<&'static str> {
    [(g.audit, "audit"), (g.tier, "tier")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect()
}

/// Without surrounding blank lines; leading spaces stay (`git status --short` columns).
fn trimmed(text: &str) -> &str {
    text.trim_end().trim_start_matches(['\n', '\r'])
}

fn head(text: &str, max: usize) -> String {
    let text = trimmed(text);
    let short: String = text.chars().take(max).collect();
    if short.len() < text.len() { format!("{short}…") } else { short }
}

fn tail(text: &str, max: usize) -> String {
    let text = trimmed(text);
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max { text.to_string() } else { format!("…{}", chars[chars.len() - max..].iter().collect::<String>()) }
}

/// What the tier gate reads: the task's title and prompt.
pub fn task_state(task: &Task) -> String {
    format!("Task: {}\n\n{}", head(&task.title, 200), head(&task.prompt, 800))
}

/// The uncommitted changes in `dir`: `git diff --stat HEAD`, else `git status --short` (no commit yet, untracked files).
async fn changes(hub: &Hub, dir: &Path) -> String {
    let timeout = Duration::from_secs(10);
    if let Ok((true, out)) = crate::orchestrator::shell(hub, dir, "git diff --stat HEAD", timeout).await
        && !out.trim().is_empty()
    {
        return format!("git diff --stat HEAD:\n{}", tail(&out, 800));
    }
    match crate::orchestrator::shell(hub, dir, "git status --short", timeout).await {
        Ok((true, out)) if out.trim().is_empty() => "No uncommitted changes.".into(),
        Ok((true, out)) => format!("git status --short:\n{}", head(&out, 800)),
        _ => "Not a git repository: no diff.".into(),
    }
}

/// What the audit gate reads: the task, the worker's summary and the change.
pub async fn audit_state(hub: &Hub, task: &Task, dir: &Path) -> String {
    let summary = task.result.as_deref().unwrap_or("(none)");
    format!("Task: {}\n\n{}\n\nWorker summary:\n{}\n\n{}", head(&task.title, 200), head(&task.prompt, 600), head(summary, 600), changes(hub, dir).await)
}

/// Reflex's part of one gate.
#[derive(Debug, Clone, PartialEq)]
pub enum Said {
    /// Reflex answered at or above the project's threshold.
    Answer { answer: Value, confidence: f64, threshold: f64, decision_id: i64 },
    /// Nothing usable: Reflex is not running, abstained, was below the threshold or failed. The reason is one line.
    Unsure(String),
}

impl Said {
    /// "confidence 9.0e-1 ≥ 5.0e-1 · decision #3" for an answer.
    pub fn detail(&self) -> String {
        match self {
            Said::Answer { confidence, threshold, decision_id, .. } => format!("confidence {} ≥ {} · decision #{decision_id}", fmt_conf(*confidence), fmt_conf(*threshold)),
            Said::Unsure(reason) => reason.clone(),
        }
    }
}

/// Why the project's Reflex cannot be asked now; `None` when it is running.
pub fn not_running(hub: &Hub, slug: &str) -> Option<Said> {
    let state = hub.reflex.status(slug).state;
    (state != ReflexState::Running).then(|| Said::Unsure(format!("Reflex is {}, not running", format!("{state:?}").to_lowercase())))
}

/// Asks the project's Reflex one gate question, never an LLM; the answer is logged as a project decision.
pub async fn ask(hub: &Hub, slug: &str, state: &str, q: Question) -> Said {
    if let Some(said) = not_running(hub, slug) {
        return said;
    }
    let project = match hub.store.project(slug).await {
        Ok(Some(p)) => p,
        Ok(None) => return Said::Unsure(format!("unknown project {slug}")),
        Err(err) => return Said::Unsure(format!("{err:#}")),
    };
    let resp = match decide::decide(hub, &project, state, std::slice::from_ref(&q), Route::ReflexGated).await {
        Ok(r) => r,
        Err(err) => return Said::Unsure(format!("{err:#}")),
    };
    let Some(a) = resp.answers.into_iter().next() else { return Said::Unsure("Reflex gave no answer".into()) };
    match (a.lane, project.threshold) {
        (Lane::Reflex, Some(threshold)) if !a.answer.is_null() => {
            Said::Answer { answer: a.answer, confidence: a.confidence.unwrap_or(0.0), threshold, decision_id: a.decision_id }
        }
        _ => {
            let error = resp.reflex_error.or(a.error).map(|e| format!(" ({})", head(e.lines().next().unwrap_or(""), 160))).unwrap_or_default();
            Said::Unsure(format!("{}{error}", a.reason.trim_end_matches('.')))
        }
    }
}

/// After the project's gates change from `before` to its current ones: when a gate was just turned on, saves the
/// gates' questions on the project and seeds `orchestration/gates.md` unless the corpus has that domain or is full.
pub async fn seed(hub: &Hub, slug: &str, before: &ReflexGates) -> Result<Vec<CheckLine>> {
    let Some(mut project) = hub.store.project(slug).await? else { return Ok(vec![]) };
    let after = project.settings.reflex_gates;
    if !((after.audit && !before.audit) || (after.tier && !before.tier)) {
        return Ok(vec![]);
    }
    let mut checks = vec![];
    let missing: Vec<Question> = questions(&after).into_iter().filter(|q| !project.questions.iter().any(|x| x.id == q.id)).collect();
    if !missing.is_empty() {
        let ids: Vec<String> = missing.iter().map(|q| q.id.clone()).collect();
        project.questions.extend(missing);
        hub.put_project(&project).await?;
        checks.push(CheckLine { level: Level::Info, text: format!("saved question{} {} for calibration on the Reflex screen", if ids.len() == 1 { "" } else { "s" }, ids.join(", ")) });
    }
    let domains = crate::projects::corpus_domains(hub, slug).await?;
    let line = if domains.contains(DOMAIN) {
        CheckLine { level: Level::Info, text: format!("Reflex gates read the corpus's existing {DOMAIN}/ domain") }
    } else if domains.len() >= crate::reflex::MAX_DOMAINS {
        CheckLine {
            level: Level::Warning,
            text: format!(
                "corpus has {} domains, the most Reflex takes: {SEED_PATH} was not added; archive a domain and add {DOMAIN}/ yourself, or the gates keep abstaining",
                domains.len()
            ),
        }
    } else {
        crate::projects::write_file(hub, slug, "corpus", SEED_PATH, Some(SEED.into()), "Seed the orchestration domain for the Reflex gates").await?;
        let state = format!("{:?}", hub.reflex.status(slug).state).to_lowercase();
        CheckLine { level: Level::Success, text: format!("corpus: added {SEED_PATH} for the Reflex gates (edit it to your rules) · Reflex {state}") }
    };
    checks.push(line);
    for c in &checks {
        hub.event(c.level, "project.reflex_gates", None, None, format!("{slug}: {}", c.text)).await;
    }
    Ok(checks)
}

/// A stand-in for a project's Reflex: a local HTTP server that answers every `POST /decide` with fixed answers.
#[cfg(test)]
pub mod fake {
    use std::sync::{Arc, Mutex};

    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::hub::Hub;
    use crate::model::ReflexState;

    pub struct FakeReflex {
        pub port: u16,
        /// Each request's JSON body.
        pub bodies: Arc<Mutex<Vec<Value>>>,
    }

    /// One answer in Reflex's wire format; `outcome` `null` abstains.
    pub fn answer(id: &str, outcome: Value, confidence: f64) -> Value {
        json!({ "question_id": id, "outcome": outcome, "confidence": confidence })
    }

    pub async fn start(answers: Vec<Value>) -> FakeReflex {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let bodies = Arc::new(Mutex::new(vec![]));
        let seen = bodies.clone();
        let reply = json!({ "answers": answers, "routing": { "reason": "fake" } }).to_string();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return };
                let mut buf = vec![];
                let mut chunk = [0u8; 4096];
                // Read the head, then the body to its Content-Length.
                let body = loop {
                    let n = s.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    let Some(end) = text.find("\r\n\r\n") else { continue };
                    let len = text[..end]
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                        .unwrap_or(0);
                    if buf.len() >= end + 4 + len {
                        break Some(buf[end + 4..end + 4 + len].to_vec());
                    }
                };
                if let Some(body) = body {
                    seen.lock().unwrap().push(serde_json::from_slice(&body).unwrap_or(Value::Null));
                }
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len());
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        FakeReflex { port, bodies }
    }

    /// Points the project at `port` with `threshold` and reports its Reflex as running.
    pub async fn attach(hub: &Hub, slug: &str, port: u16, threshold: Option<f64>) {
        hub.store.set_project_port(slug, port).await.unwrap();
        let mut p = hub.store.project(slug).await.unwrap().unwrap();
        p.threshold = threshold;
        hub.put_project(&p).await.unwrap();
        hub.reflex.pretend(slug, ReflexState::Running);
    }

    /// A loopback port nothing listens on.
    pub fn closed_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use crate::config::Config;
    use crate::model::{ProjectSettings, ProjectSource, SourceKind};
    use crate::projects::{NewProject, create_with};
    use crate::store::Store;
    use crate::terminal::fake::FakeTerminal;

    async fn hub() -> (Arc<Hub>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!("[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\n", dir.path().display());
        let config = Config::parse(&text, Path::new("/")).unwrap();
        (Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default())), dir)
    }

    async fn project(hub: &Hub, dir: &Path, name: &str, gates: ReflexGates) -> (String, Vec<CheckLine>) {
        let source = ProjectSource { kind: SourceKind::Empty, location: Some(dir.join(name).display().to_string()), branch: None, remote: None };
        let new = NewProject { name: name.into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let (p, checks) = create_with(hub, new, ProjectSettings { source, reflex_gates: gates, ..Default::default() }).await.unwrap();
        (p.slug, checks)
    }

    async fn turn_on(hub: &Hub, slug: &str, gates: ReflexGates) -> Vec<CheckLine> {
        let mut p = hub.store.project(slug).await.unwrap().unwrap();
        let before = p.settings.reflex_gates;
        p.settings.reflex_gates = gates;
        hub.put_project(&p).await.unwrap();
        seed(hub, slug, &before).await.unwrap()
    }

    async fn seeded(hub: &Hub, slug: &str) -> Option<String> {
        crate::repo::read(&hub.corpus_repo(slug), SEED_PATH).await
    }

    #[tokio::test]
    async fn turning_a_gate_on_seeds_the_orchestration_domain_once() {
        let (hub, dir) = hub().await;
        let (slug, _) = project(&hub, dir.path(), "Api", ReflexGates::default()).await;
        assert!(seeded(&hub, &slug).await.is_none());
        assert!(turn_on(&hub, &slug, ReflexGates::default()).await.is_empty(), "nothing turned on, nothing done");

        let checks = turn_on(&hub, &slug, ReflexGates { audit: true, tier: false }).await;
        let text: Vec<&str> = checks.iter().map(|c| c.text.as_str()).collect();
        assert!(text.iter().any(|t| t.starts_with("corpus: added orchestration/gates.md")), "{text:?}");
        assert!(seeded(&hub, &slug).await.unwrap().contains("Changes that need a careful audit"));
        let ids: Vec<String> = hub.store.project(&slug).await.unwrap().unwrap().questions.into_iter().map(|q| q.id).collect();
        assert_eq!(ids, ["needs_audit"]);
        let commits = crate::repo::log(&hub.corpus_repo(&slug), 10).await.unwrap().len();

        // Off and on again, and the second gate: the domain is there, so nothing is written.
        turn_on(&hub, &slug, ReflexGates::default()).await;
        let checks = turn_on(&hub, &slug, ReflexGates { audit: true, tier: true }).await;
        assert!(checks.iter().any(|c| c.text.contains("existing orchestration/ domain")), "{checks:?}");
        assert_eq!(crate::repo::log(&hub.corpus_repo(&slug), 10).await.unwrap().len(), commits);
        let ids: Vec<String> = hub.store.project(&slug).await.unwrap().unwrap().questions.into_iter().map(|q| q.id).collect();
        assert_eq!(ids, ["needs_audit", "tier"]);
    }

    #[tokio::test]
    async fn a_full_corpus_is_not_seeded() {
        let (hub, dir) = hub().await;
        let (slug, _) = project(&hub, dir.path(), "Full", ReflexGates::default()).await;
        let more: Vec<(String, Option<String>)> = (1..crate::reflex::MAX_DOMAINS).map(|i| (format!("d{i}/a.md"), Some("prose".into()))).collect();
        crate::repo::commit(&hub.corpus_repo(&slug), &more, "more domains").await.unwrap();
        assert_eq!(crate::projects::corpus_domains(&hub, &slug).await.unwrap().len(), crate::reflex::MAX_DOMAINS);
        let checks = turn_on(&hub, &slug, ReflexGates { audit: false, tier: true }).await;
        let warn = checks.iter().find(|c| c.level == Level::Warning).expect("a warning");
        assert!(warn.text.contains("8 domains") && warn.text.contains("was not added"), "{}", warn.text);
        assert!(seeded(&hub, &slug).await.is_none());
    }

    #[tokio::test]
    async fn a_project_created_with_a_gate_on_starts_with_the_domain() {
        let (hub, dir) = hub().await;
        let (slug, checks) = project(&hub, dir.path(), "Gated", ReflexGates { audit: true, tier: true }).await;
        assert!(seeded(&hub, &slug).await.is_some());
        let domains = crate::projects::corpus_domains(&hub, &slug).await.unwrap();
        assert!(domains.contains("orchestration") && domains.contains("example"), "{domains:?}");
        let text: Vec<&str> = checks.iter().map(|c| c.text.as_str()).collect();
        assert!(text.iter().any(|t| t.contains("orchestration/gates.md for the Reflex gates")), "{text:?}");
        assert!(text.iter().any(|t| t.contains("Reflex gates: audit, tier")), "{text:?}");
        assert_eq!(hub.store.project(&slug).await.unwrap().unwrap().questions.len(), 2);
    }

    #[tokio::test]
    async fn the_audit_state_carries_the_task_summary_and_change() {
        let (hub, dir) = hub().await;
        let folder = dir.path().join("code");
        std::fs::create_dir_all(&folder).unwrap();
        let task = |prompt: String| Task {
            id: 1,
            title: "Fix typo".into(),
            prompt,
            depends_on: vec![],
            provider: None,
            model: None,
            status: crate::model::TaskStatus::Auditing,
            attempts: 1,
            session: None,
            result: Some("fixed it".into()),
            project: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let state = audit_state(&hub, &task("long ".repeat(500)), &folder).await;
        assert!(state.starts_with("Task: Fix typo\n\nlong long") && state.contains("…\n\nWorker summary:\nfixed it"), "{state}");
        assert!(state.ends_with("Not a git repository: no diff."), "{state}");

        let sh = |cmd: &str| std::process::Command::new("sh").arg("-c").arg(cmd).current_dir(&folder).output().unwrap();
        sh("git init -q && echo a > README.md");
        assert!(audit_state(&hub, &task("p".into()), &folder).await.contains("git status --short:\n?? README.md"));
        sh("git add -A && git -c user.name=t -c user.email=t@t commit -qm init && echo b >> README.md");
        let state = audit_state(&hub, &task("p".into()), &folder).await;
        assert!(state.contains("git diff --stat HEAD:\n README.md | 1 +") && state.contains("1 file changed"), "{state}");
        sh("git -c user.name=t -c user.email=t@t commit -qam two");
        assert!(audit_state(&hub, &task("p".into()), &folder).await.ends_with("No uncommitted changes."));
    }

    #[tokio::test]
    async fn ask_logs_a_decision_and_applies_the_threshold() {
        let (hub, dir) = hub().await;
        let (slug, _) = project(&hub, dir.path(), "Ask", ReflexGates::default()).await;
        // Not running: Reflex is not asked and nothing is logged.
        assert!(matches!(ask(&hub, &slug, "state", needs_audit_question()).await, Said::Unsure(r) if r.contains("not running")));
        assert!(hub.store.decisions(&slug, 10).await.unwrap().is_empty());

        let reflex = fake::start(vec![fake::answer(NEEDS_AUDIT, serde_json::json!({"noul": {"yes": false}}), 0.2)]).await;
        fake::attach(&hub, &slug, reflex.port, Some(0.5)).await;
        let said = ask(&hub, &slug, "Task: fix a typo", needs_audit_question()).await;
        assert!(matches!(&said, Said::Unsure(r) if r.contains("below the threshold")), "{said:?}");
        fake::attach(&hub, &slug, reflex.port, None).await;
        assert!(matches!(ask(&hub, &slug, "Task: fix a typo", needs_audit_question()).await, Said::Unsure(r) if r.starts_with("Not calibrated")));
        fake::attach(&hub, &slug, reflex.port, Some(0.1)).await;
        let said = ask(&hub, &slug, "Task: fix a typo", needs_audit_question()).await;
        let Said::Answer { answer, decision_id, .. } = &said else { panic!("{said:?}") };
        assert_eq!(answer, &Value::Bool(false));
        assert_eq!(said.detail(), format!("confidence 2.0e-1 ≥ 1.0e-1 · decision #{decision_id}"));

        // Thai text never reaches Reflex.
        assert!(matches!(ask(&hub, &slug, "แก้คำผิด", needs_audit_question()).await, Said::Unsure(r) if r.starts_with("Thai input")));
        assert_eq!(reflex.bodies.lock().unwrap().len(), 3);

        let rows = hub.store.decisions(&slug, 10).await.unwrap();
        assert_eq!(rows.len(), 4, "every asked gate question is in the decision log");
        assert!(rows.iter().all(|d| d.question.id == NEEDS_AUDIT && d.lane != Lane::Llm));
        assert!(rows.iter().any(|d| d.lane == Lane::Reflex && d.state == "Task: fix a typo"));
    }
}
