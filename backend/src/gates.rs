//! Reflex gates: decisions the orchestrator repeats for every task, asked of Reflex first and never of an LLM.
//! `intake` comes first, before any LLM sees a task: unclear (ask the owner), plan first, or ready as a light or heavy
//! task, with `shared` (shared files or its own) in the same call. `needs_audit` can skip a whole LLM audit after
//! verification. Whenever Reflex is not running, abstains or is below the gates' threshold, the orchestrator does what
//! it did before.
//!
//! The gates have a Reflex of their own, shared by every project: a choice question only routes between corpus
//! domains named like its options (and a corpus holds at most 8), so the gates' corpus is one domain per answer,
//! seeded from `templates/gates/` into `<data_dir>/gates/corpus.git`. A project's own corpus stays its own. Thai tasks
//! reach Reflex glossed into English with the project's glossary (`decide::for_reflex`).

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;

use crate::decide::{self, Route, fmt_conf};
use crate::hub::Hub;
use crate::model::{CheckLine, Lane, Level, Question, QuestionKind, ReflexGates, ReflexState, Task};

/// The gates Reflex's key in the Reflex manager (never a project slug: those cannot start with `_`).
pub const KEY: &str = "_gates";
pub const INTAKE: &str = "intake";
pub const SHARED: &str = "shared";
pub const NEEDS_AUDIT: &str = "needs_audit";
/// `intake`'s options, each a domain of the gates corpus. `light` and `heavy` are ready tasks and their tier.
pub const INTAKE_OPTIONS: [&str; 4] = ["unclear", "plan_first", "light", "heavy"];
pub const SHARED_OPTIONS: [&str; 2] = ["shared", "own"];
pub const AUDIT_OPTIONS: [&str; 2] = ["audit", "no_audit"];

/// The gates corpus as first seeded: one domain per answer.
pub const SEED: &[(&str, &str)] = &[
    ("unclear/unclear.md", include_str!("../templates/gates/unclear.md")),
    ("plan_first/plan_first.md", include_str!("../templates/gates/plan_first.md")),
    ("light/light.md", include_str!("../templates/gates/light.md")),
    ("heavy/heavy.md", include_str!("../templates/gates/heavy.md")),
    ("shared/shared.md", include_str!("../templates/gates/shared.md")),
    ("own/own.md", include_str!("../templates/gates/own.md")),
    ("audit/audit.md", include_str!("../templates/gates/audit.md")),
    ("no_audit/no_audit.md", include_str!("../templates/gates/no_audit.md")),
];

fn choice(id: &str, prompt: &str, options: &[&str]) -> Question {
    Question { id: id.into(), kind: QuestionKind::Choice, prompt: prompt.into(), options: Some(options.iter().map(|t| t.to_string()).collect()), criteria: None }
}

pub fn intake_question() -> Question {
    choice(INTAKE, "Before an agent starts: is this task too unclear to start, big enough to plan first, or a light or a heavy task?", &INTAKE_OPTIONS)
}

pub fn shared_question() -> Question {
    choice(SHARED, "Does this task change files other tasks share (dependencies, migrations, generated code, central config), or only its own?", &SHARED_OPTIONS)
}

pub fn needs_audit_question() -> Question {
    choice(NEEDS_AUDIT, "Does this change need a careful LLM audit before it is accepted?", &AUDIT_OPTIONS)
}

/// The tier in an `intake` answer: `light` or `heavy`.
pub fn tier_of(answer: &Value) -> Option<&str> {
    answer.as_str().filter(|a| matches!(*a, "light" | "heavy"))
}

/// Whether a question id is one of the gates' (asked of the gates Reflex, not the project's).
pub fn is_gate(id: &str) -> bool {
    matches!(id, INTAKE | SHARED | NEEDS_AUDIT)
}

pub fn names(g: &ReflexGates) -> Vec<&'static str> {
    [(g.intake, "intake"), (g.audit, "audit"), (g.tier, "tier")].into_iter().filter(|(on, _)| *on).map(|(_, n)| n).collect()
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
async fn changes(hub: &Hub, dir: &Path, base: Option<&str>) -> String {
    let timeout = Duration::from_secs(10);
    // A task on its own branch: everything since it left the base, committed or not.
    if let Some(base) = base
        && let Ok(base) = crate::hub::quote(base)
        && let Ok((true, out)) = crate::orchestrator::shell(hub, dir, &format!("git diff --stat \"$(git merge-base {base} HEAD)\""), timeout).await
        && !out.trim().is_empty()
    {
        return format!("git diff --stat against {base}:\n{}", tail(&out, 800));
    }
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
    format!("Task: {}\n\n{}\n\nWorker summary:\n{}\n\n{}", head(&task.title, 200), head(&task.prompt, 600), head(summary, 600), changes(hub, dir, task.git.as_ref().map(|g| g.base.as_str())).await)
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

/// Why the gates Reflex cannot be asked now; `None` when it is running.
pub fn not_running(hub: &Hub) -> Option<Said> {
    let state = hub.reflex.status(KEY).state;
    (state != ReflexState::Running).then(|| Said::Unsure(format!("the gates Reflex is {}, not running", format!("{state:?}").to_lowercase())))
}

/// Asks the gates Reflex one question about a task of `slug`, never an LLM; the answer is logged as a decision of
/// the project.
pub async fn ask(hub: &Hub, slug: &str, state: &str, q: Question) -> Said {
    ask_many(hub, slug, state, vec![q]).await.pop().unwrap_or_else(|| Said::Unsure("Reflex gave no answer".into()))
}

/// Several gate questions about one state in a single Reflex call; one `Said` per question, in order.
pub async fn ask_many(hub: &Hub, slug: &str, state: &str, qs: Vec<Question>) -> Vec<Said> {
    let all = |said: Said| qs.iter().map(|_| said.clone()).collect::<Vec<_>>();
    if let Some(said) = not_running(hub) {
        return all(said);
    }
    let project = match hub.store.project(slug).await {
        Ok(Some(p)) => p,
        Ok(None) => return all(Said::Unsure(format!("unknown project {slug}"))),
        Err(err) => return all(Said::Unsure(format!("{err:#}"))),
    };
    let target = hub.gates_target();
    let resp = match decide::decide_at(hub, &project, target, state, &qs, Route::ReflexGated).await {
        Ok(r) => r,
        Err(err) => return all(Said::Unsure(format!("{err:#}"))),
    };
    qs.iter()
        .map(|q| {
            let Some(a) = resp.answers.iter().find(|a| a.question_id == q.id) else { return Said::Unsure("Reflex gave no answer".into()) };
            match (a.lane, target.threshold) {
                (Lane::Reflex, Some(threshold)) if !a.answer.is_null() => {
                    Said::Answer { answer: a.answer.clone(), confidence: a.confidence.unwrap_or(0.0), threshold, decision_id: a.decision_id }
                }
                _ => {
                    let error = resp.reflex_error.clone().or(a.error.clone()).map(|e| format!(" ({})", head(e.lines().next().unwrap_or(""), 160))).unwrap_or_default();
                    Said::Unsure(format!("{}{error}", a.reason.trim_end_matches('.')))
                }
            }
        })
        .collect()
}

/// After the project's gates change from `before`: a line on the gates Reflex when a gate was just turned on.
pub async fn seed(hub: &Hub, slug: &str, before: &ReflexGates) -> Result<Vec<CheckLine>> {
    let Some(project) = hub.store.project(slug).await? else { return Ok(vec![]) };
    let after = project.settings.reflex_gates;
    if !((after.audit && !before.audit) || (after.tier && !before.tier) || (after.intake && !before.intake)) {
        return Ok(vec![]);
    }
    // Not running yet (no project had a gate on, or the binary was installed since): try once now.
    if hub.reflex.status(KEY).state != ReflexState::Running
        && let Err(err) = hub.restart_gates().await
    {
        tracing::warn!("gates reflex: {err:#}");
    }
    let line = gates_line(hub);
    hub.event(line.level, "project.reflex_gates", None, None, format!("{slug}: {}", line.text)).await;
    Ok(vec![line])
}

/// How the gates Reflex is doing, as a check line.
pub fn gates_line(hub: &Hub) -> CheckLine {
    let s = hub.reflex.status(KEY);
    let t = hub.gates_target();
    match s.state {
        ReflexState::Running => CheckLine {
            level: Level::Success,
            text: match t.threshold {
                Some(th) => format!("the gates Reflex is running · {} domains · threshold {}", s.domains.len(), fmt_conf(th)),
                None => format!("the gates Reflex is running · {} domains · shadow: answers are logged, none is acted on until a threshold is set (Settings → Reflex gates)", s.domains.len()),
            },
        },
        state => CheckLine {
            level: Level::Warning,
            text: format!("the gates Reflex is {}: until it runs, every gate does what it did before ({})", format!("{state:?}").to_lowercase(), s.message.lines().next().unwrap_or("")),
        },
    }
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

    /// Points the gates at a fake Reflex on `port` with `threshold` and reports the gates Reflex as running.
    pub fn attach_gates(hub: &Hub, port: u16, threshold: Option<f64>) {
        hub.set_gates_target(crate::decide::Target { port, threshold });
        hub.reflex.pretend(super::KEY, ReflexState::Running);
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

    #[tokio::test]
    async fn the_gates_corpus_is_one_domain_per_answer() {
        let mut domains: Vec<&str> = SEED.iter().map(|(p, _)| p.split('/').next().unwrap()).collect();
        domains.sort();
        let mut answers: Vec<&str> = INTAKE_OPTIONS.iter().chain(&SHARED_OPTIONS).chain(&AUDIT_OPTIONS).copied().collect();
        answers.sort();
        assert_eq!(domains, answers);
        assert!(domains.len() <= crate::reflex::MAX_DOMAINS);
        let (hub, dir) = hub().await;
        let repo = hub.gates_corpus().await.unwrap();
        assert_eq!(crate::repo::files(&repo).await.unwrap().len(), SEED.len());
        // A project's own corpus gets nothing of the gates, whatever is on.
        let (slug, checks) = project(&hub, dir.path(), "Gated", ReflexGates { intake: true, audit: true, tier: true }).await;
        assert_eq!(crate::projects::corpus_domains(&hub, &slug).await.unwrap().into_iter().collect::<Vec<_>>(), ["example"]);
        let text: Vec<&str> = checks.iter().map(|c| c.text.as_str()).collect();
        assert!(text.iter().any(|t| t.contains("Reflex gates: intake, audit, tier")), "{text:?}");
        assert!(text.iter().any(|t| t.starts_with("the gates Reflex is")), "{text:?}");
        assert!(hub.store.project(&slug).await.unwrap().unwrap().questions.is_empty());
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
            git: None,
            scope: None,
            intake: None,
            workflow: None,
            workflow_state: None,
            created_by: String::new(),
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

        let reflex = fake::start(vec![fake::answer(NEEDS_AUDIT, serde_json::json!({"choice": {"index": 1}}), 0.2)]).await;
        // The project's own Reflex is elsewhere: the gates never ask it.
        fake::attach(&hub, &slug, fake::closed_port(), Some(0.0001)).await;
        fake::attach_gates(&hub, reflex.port, Some(0.5));
        let said = ask(&hub, &slug, "Task: fix a typo", needs_audit_question()).await;
        assert!(matches!(&said, Said::Unsure(r) if r.contains("below the threshold")), "{said:?}");
        fake::attach_gates(&hub, reflex.port, None);
        assert!(matches!(ask(&hub, &slug, "Task: fix a typo", needs_audit_question()).await, Said::Unsure(r) if r.starts_with("Not calibrated")));
        fake::attach_gates(&hub, reflex.port, Some(0.1));
        let said = ask(&hub, &slug, "Task: fix a typo", needs_audit_question()).await;
        let Said::Answer { answer, decision_id, .. } = &said else { panic!("{said:?}") };
        assert_eq!(answer, &Value::String("no_audit".into()));
        assert_eq!(said.detail(), format!("confidence 2.0e-1 ≥ 1.0e-1 · decision #{decision_id}"));
        assert_eq!(reflex.bodies.lock().unwrap()[0]["questions"][0]["options"], serde_json::json!(["audit", "no_audit"]));

        // Thai the glossary knows reaches Reflex in English; Thai it does not know never does.
        let said = ask(&hub, &slug, "แก้คำผิดในหน้าแรก", needs_audit_question()).await;
        assert!(matches!(said, Said::Answer { .. }), "{said:?}");
        assert_eq!(reflex.bodies.lock().unwrap().last().unwrap()["state"], "fix typo in home page");
        assert!(matches!(ask(&hub, &slug, "ฉันชอบกินข้าวมันไก่ที่ร้านป้าแดงตรงหัวมุมถนน", needs_audit_question()).await, Said::Unsure(r) if r.starts_with("Thai input")));
        assert_eq!(reflex.bodies.lock().unwrap().len(), 4);

        let rows = hub.store.decisions(&slug, 10).await.unwrap();
        assert_eq!(rows.len(), 5, "every asked gate question is in the project's decision log");
        assert!(rows.iter().all(|d| d.question.id == NEEDS_AUDIT && d.lane != Lane::Llm));
        assert!(rows.iter().any(|d| d.lane == Lane::Reflex && d.state == "Task: fix a typo"));
    }
}
