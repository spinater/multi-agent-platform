//! SQLite persistence for sessions, tasks and the event log. Survives restarts; the watcher reconciles
//! stored sessions against tmux on its first tick.

use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions, SqliteRow};
use sqlx::{Row, SqlitePool};

use crate::model::{CaseScore, Decision, DecisionStats, DecisionSummary, Event, HourBucket, Lane, LaneSummary, Level, Project, ReflexCase, Session, Task, TaskStatus};
use crate::savings;

#[derive(Clone)]
pub struct Store {
    pub(crate) pool: SqlitePool,
}

/// Enums are stored as their serde names (`rate_limited`, `worker`, …), the same strings the API uses.
fn enum_str<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(s)) => s,
        _ => unreachable!("unit enum serializes to a string"),
    }
}

fn parse_enum<T: DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_value(serde_json::Value::String(s.to_string())).with_context(|| format!("unknown value `{s}`"))
}

#[derive(Debug, Clone)]
pub struct NewTask {
    pub title: String,
    pub prompt: String,
    pub depends_on: Vec<i64>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub project: Option<String>,
}

/// What a decision cost, or would have cost, in fallback tokens (the savings report, savings.rs).
/// The default is a row as old as before migration 0008: nothing known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DecisionCost {
    /// Length in chars of the fallback prompt, also when the fallback was not asked.
    pub prompt_chars: Option<i64>,
    /// Tokens the fallback used; `None` when it was not asked or failed.
    pub tokens_in: Option<i64>,
    pub tokens_out: Option<i64>,
    /// The provider reported the tokens (else they are estimated).
    pub measured: bool,
    /// The project's fallback at the time.
    pub fallback: Option<String>,
    /// Thai input: Reflex was not asked.
    pub skipped_thai: bool,
}

impl Store {
    pub async fn open(path: &Path) -> Result<Self> {
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new().max_connections(4).connect_with(options).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    #[cfg(test)]
    pub async fn memory() -> Result<Self> {
        let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    // ---- sessions

    pub async fn save_session(&self, s: &Session) -> Result<()> {
        sqlx::query(
            "INSERT INTO sessions (name, provider, model, role, task_id, state, detail, rate_limit_reset_at, wakeups, last_line, attached, created_at, last_activity_at, project)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(name) DO UPDATE SET provider = excluded.provider, model = excluded.model, role = excluded.role,
               task_id = excluded.task_id, project = excluded.project, state = excluded.state, detail = excluded.detail,
               rate_limit_reset_at = excluded.rate_limit_reset_at, wakeups = excluded.wakeups, last_line = excluded.last_line,
               attached = excluded.attached, last_activity_at = excluded.last_activity_at",
        )
        .bind(&s.name)
        .bind(&s.provider)
        .bind(&s.model)
        .bind(enum_str(&s.role))
        .bind(s.task_id)
        .bind(enum_str(&s.state))
        .bind(&s.detail)
        .bind(s.rate_limit_reset_at)
        .bind(s.wakeups)
        .bind(&s.last_line)
        .bind(s.attached)
        .bind(s.created_at)
        .bind(s.last_activity_at)
        .bind(&s.project)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    fn session_row(row: &SqliteRow) -> Result<Session> {
        Ok(Session {
            name: row.get("name"),
            provider: row.get("provider"),
            model: row.get("model"),
            role: parse_enum(row.get("role"))?,
            task_id: row.get("task_id"),
            project: row.get("project"),
            state: parse_enum(row.get("state"))?,
            detail: row.get("detail"),
            rate_limit_reset_at: row.get("rate_limit_reset_at"),
            wakeups: row.get("wakeups"),
            last_line: row.get("last_line"),
            attached: row.get("attached"),
            created_at: row.get("created_at"),
            last_activity_at: row.get("last_activity_at"),
        })
    }

    pub async fn session(&self, name: &str) -> Result<Option<Session>> {
        let row = sqlx::query("SELECT * FROM sessions WHERE name = ?").bind(name).fetch_optional(&self.pool).await?;
        row.as_ref().map(Self::session_row).transpose()
    }

    /// Live sessions first, newest first; exited sessions stay listed until deleted so their history is visible.
    pub async fn sessions(&self) -> Result<Vec<Session>> {
        let rows = sqlx::query("SELECT * FROM sessions ORDER BY state = 'exited', created_at DESC").fetch_all(&self.pool).await?;
        rows.iter().map(Self::session_row).collect()
    }

    pub async fn delete_session(&self, name: &str) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE name = ?").bind(name).execute(&self.pool).await?;
        Ok(())
    }

    // ---- tasks

    pub async fn create_task(&self, t: NewTask) -> Result<Task> {
        let now = Utc::now();
        let id: i64 = sqlx::query(
            "INSERT INTO tasks (title, prompt, depends_on, provider, model, project, status, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&t.title)
        .bind(&t.prompt)
        .bind(serde_json::to_string(&t.depends_on)?)
        .bind(&t.provider)
        .bind(&t.model)
        .bind(&t.project)
        .bind(TaskStatus::Pending.as_str())
        .bind(now)
        .bind(now)
        .fetch_one(&self.pool)
        .await?
        .get("id");
        self.task(id).await?.context("task vanished after insert")
    }

    pub async fn save_task(&self, t: &Task) -> Result<()> {
        sqlx::query(
            "UPDATE tasks SET title = ?, prompt = ?, depends_on = ?, provider = ?, model = ?, status = ?, session = ?, attempts = ?, result = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&t.title)
        .bind(&t.prompt)
        .bind(serde_json::to_string(&t.depends_on)?)
        .bind(&t.provider)
        .bind(&t.model)
        .bind(t.status.as_str())
        .bind(&t.session)
        .bind(t.attempts)
        .bind(&t.result)
        .bind(t.updated_at)
        .bind(t.id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    fn task_row(row: &SqliteRow) -> Result<Task> {
        Ok(Task {
            id: row.get("id"),
            title: row.get("title"),
            prompt: row.get("prompt"),
            depends_on: serde_json::from_str(row.get("depends_on"))?,
            provider: row.get("provider"),
            model: row.get("model"),
            project: row.get("project"),
            status: parse_enum(row.get("status"))?,
            session: row.get("session"),
            attempts: row.get("attempts"),
            result: row.get("result"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        })
    }

    pub async fn task(&self, id: i64) -> Result<Option<Task>> {
        let row = sqlx::query("SELECT * FROM tasks WHERE id = ?").bind(id).fetch_optional(&self.pool).await?;
        row.as_ref().map(Self::task_row).transpose()
    }

    pub async fn tasks(&self) -> Result<Vec<Task>> {
        let rows = sqlx::query("SELECT * FROM tasks ORDER BY id").fetch_all(&self.pool).await?;
        rows.iter().map(Self::task_row).collect()
    }

    pub async fn delete_task(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM tasks WHERE id = ?").bind(id).execute(&self.pool).await?;
        Ok(())
    }

    // ---- projects

    pub async fn save_project(&self, p: &Project) -> Result<()> {
        sqlx::query(
            "INSERT INTO projects (slug, name, description, port, threshold, fallback, model, questions, created_at, settings) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(slug) DO UPDATE SET name = excluded.name, description = excluded.description, threshold = excluded.threshold,
               fallback = excluded.fallback, model = excluded.model, questions = excluded.questions, settings = excluded.settings",
        )
        .bind(&p.slug)
        .bind(&p.name)
        .bind(&p.description)
        .bind(p.port)
        .bind(p.threshold)
        .bind(&p.fallback)
        .bind(&p.model)
        .bind(serde_json::to_string(&p.questions)?)
        .bind(p.created_at)
        .bind(serde_json::to_string(&p.settings)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Tests: move a project's Reflex to `port` (a fake Reflex); `save_project` never changes a port.
    #[cfg(test)]
    pub async fn set_project_port(&self, slug: &str, port: u16) -> Result<()> {
        sqlx::query("UPDATE projects SET port = ? WHERE slug = ?").bind(port).bind(slug).execute(&self.pool).await?;
        Ok(())
    }

    fn project_row(row: &SqliteRow) -> Result<Project> {
        Ok(Project {
            slug: row.get("slug"),
            name: row.get("name"),
            description: row.get("description"),
            port: row.get("port"),
            threshold: row.get("threshold"),
            fallback: row.get("fallback"),
            model: row.get("model"),
            questions: serde_json::from_str(row.get("questions"))?,
            created_at: row.get("created_at"),
            settings: serde_json::from_str(row.get("settings"))?,
        })
    }

    pub async fn project(&self, slug: &str) -> Result<Option<Project>> {
        let row = sqlx::query("SELECT * FROM projects WHERE slug = ?").bind(slug).fetch_optional(&self.pool).await?;
        row.as_ref().map(Self::project_row).transpose()
    }

    pub async fn projects(&self) -> Result<Vec<Project>> {
        let rows = sqlx::query("SELECT * FROM projects ORDER BY created_at").fetch_all(&self.pool).await?;
        rows.iter().map(Self::project_row).collect()
    }

    /// The lowest port at or above `base` that no project uses.
    pub async fn free_port(&self, base: u16) -> Result<u16> {
        let used: Vec<u16> = sqlx::query("SELECT port FROM projects").fetch_all(&self.pool).await?.iter().map(|r| r.get("port")).collect();
        (base..=u16::MAX).find(|p| !used.contains(p)).context("no free port for Reflex")
    }

    pub async fn delete_project(&self, slug: &str) -> Result<()> {
        sqlx::query("DELETE FROM decisions WHERE project = ?").bind(slug).execute(&self.pool).await?;
        sqlx::query("DELETE FROM reflex_cases WHERE project = ?").bind(slug).execute(&self.pool).await?;
        sqlx::query("DELETE FROM projects WHERE slug = ?").bind(slug).execute(&self.pool).await?;
        Ok(())
    }

    // ---- decisions

    pub async fn insert_decision(&self, d: &Decision, cost: &DecisionCost) -> Result<i64> {
        let id = sqlx::query(
            "INSERT INTO decisions (project, at, state, question, answer, lane, confidence, reflex_answer, ms, error,
               prompt_chars, tokens_in, tokens_out, tokens_measured, fallback, skipped_thai)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(&d.project)
        .bind(d.at)
        .bind(&d.state)
        .bind(serde_json::to_string(&d.question)?)
        .bind(d.answer.to_string())
        .bind(enum_str(&d.lane))
        .bind(d.confidence)
        .bind(d.reflex_answer.to_string())
        .bind(d.ms)
        .bind(&d.error)
        .bind(cost.prompt_chars)
        .bind(cost.tokens_in)
        .bind(cost.tokens_out)
        .bind(cost.measured)
        .bind(&cost.fallback)
        .bind(cost.skipped_thai)
        .fetch_one(&self.pool)
        .await?
        .get("id");
        Ok(id)
    }

    fn decision_row(row: &SqliteRow) -> Result<Decision> {
        Ok(Decision {
            id: row.get("id"),
            project: row.get("project"),
            at: row.get("at"),
            state: row.get("state"),
            question: serde_json::from_str(row.get("question"))?,
            answer: serde_json::from_str(row.get("answer"))?,
            lane: parse_enum(row.get("lane"))?,
            confidence: row.get("confidence"),
            reflex_answer: serde_json::from_str(row.get("reflex_answer"))?,
            ms: row.get("ms"),
            error: row.get("error"),
            feedback: row.get("feedback"),
        })
    }

    /// Newest first.
    pub async fn decisions(&self, project: &str, limit: i64) -> Result<Vec<Decision>> {
        let rows = sqlx::query("SELECT * FROM decisions WHERE project = ? ORDER BY id DESC LIMIT ?").bind(project).bind(limit).fetch_all(&self.pool).await?;
        rows.iter().map(Self::decision_row).collect()
    }

    pub async fn decision_stats(&self, project: &str) -> Result<DecisionStats> {
        let rows = sqlx::query("SELECT lane, COUNT(*) AS n FROM decisions WHERE project = ? GROUP BY lane").bind(project).fetch_all(&self.pool).await?;
        let mut stats = DecisionStats::default();
        for row in rows {
            let n: i64 = row.get("n");
            stats.total += n;
            match parse_enum::<Lane>(row.get("lane"))? {
                Lane::Reflex => stats.reflex = n,
                Lane::Llm => stats.llm = n,
                Lane::Abstain => stats.abstain = n,
            }
        }
        Ok(stats)
    }

    // ---- events

    pub async fn insert_event(&self, level: Level, kind: &str, session: Option<&str>, task_id: Option<i64>, message: &str) -> Result<Event> {
        let at: DateTime<Utc> = Utc::now();
        let id: i64 = sqlx::query("INSERT INTO events (at, level, kind, session, task_id, message) VALUES (?, ?, ?, ?, ?, ?) RETURNING id")
            .bind(at)
            .bind(enum_str(&level))
            .bind(kind)
            .bind(session)
            .bind(task_id)
            .bind(message)
            .fetch_one(&self.pool)
            .await?
            .get("id");
        Ok(Event { id, at, level, kind: kind.into(), session: session.map(String::from), task_id, message: message.into() })
    }

    /// Newest first.
    pub async fn events(&self, limit: i64, session: Option<&str>) -> Result<Vec<Event>> {
        let rows = sqlx::query("SELECT * FROM events WHERE (?1 IS NULL OR session = ?1) ORDER BY id DESC LIMIT ?2")
            .bind(session)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| {
                Ok(Event {
                    id: row.get("id"),
                    at: row.get("at"),
                    level: parse_enum(row.get("level"))?,
                    kind: row.get("kind"),
                    session: row.get("session"),
                    task_id: row.get("task_id"),
                    message: row.get("message"),
                })
            })
            .collect()
    }

    // ---- orchestrator decisions (`decision.*` events, see decisions.rs)

    fn decision_event_row(row: &SqliteRow) -> Result<Event> {
        Ok(Event {
            id: row.get("id"),
            at: row.get("at"),
            level: parse_enum(row.get("level"))?,
            kind: row.get("kind"),
            session: row.get("session"),
            task_id: row.get("task_id"),
            message: row.get("message"),
        })
    }

    /// A task's decision trace, newest first.
    pub async fn decision_events(&self, task_id: i64, limit: i64) -> Result<Vec<Event>> {
        let rows = sqlx::query("SELECT * FROM events WHERE task_id = ? AND kind LIKE 'decision.%' ORDER BY id DESC LIMIT ?")
            .bind(task_id)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?;
        rows.iter().map(Self::decision_event_row).collect()
    }

    /// Decision events with an id above `after`, oldest first.
    pub async fn decision_events_after(&self, after: i64) -> Result<Vec<Event>> {
        let rows = sqlx::query("SELECT * FROM events WHERE id > ? AND kind LIKE 'decision.%' ORDER BY id").bind(after).fetch_all(&self.pool).await?;
        rows.iter().map(Self::decision_event_row).collect()
    }

    pub async fn last_event_id(&self) -> Result<i64> {
        Ok(sqlx::query("SELECT COALESCE(MAX(id), 0) AS id FROM events").fetch_one(&self.pool).await?.get("id"))
    }

    /// The newest decision event of every task (in `project`: `None` all, `Some("")` the default workspace).
    pub async fn latest_decisions(&self, project: Option<&str>) -> Result<Vec<Event>> {
        let rows = sqlx::query(
            "SELECT e.* FROM events e JOIN tasks t ON t.id = e.task_id
             WHERE e.id IN (SELECT MAX(id) FROM events WHERE kind LIKE 'decision.%' AND task_id IS NOT NULL GROUP BY task_id)
               AND (?1 IS NULL OR (?1 = '' AND t.project IS NULL) OR t.project = ?1)
             ORDER BY e.id DESC",
        )
        .bind(project)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(Self::decision_event_row).collect()
    }

    /// How many decision events of each kind happened since `since` (project filter as in `latest_decisions`).
    pub async fn decision_kind_counts(&self, since: DateTime<Utc>, project: Option<&str>) -> Result<Vec<(String, i64)>> {
        let rows = sqlx::query(
            "SELECT e.kind, COUNT(*) AS n FROM events e JOIN tasks t ON t.id = e.task_id
             WHERE e.kind LIKE 'decision.%' AND julianday(e.at) >= julianday(?1)
               AND (?2 IS NULL OR (?2 = '' AND t.project IS NULL) OR t.project = ?2)
             GROUP BY e.kind",
        )
        .bind(since)
        .bind(project)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| (r.get("kind"), r.get("n"))).collect())
    }

    // ---- reflex screen: decision feedback, calibration cases, summary

    pub async fn decision(&self, project: &str, id: i64) -> Result<Option<Decision>> {
        let row = sqlx::query("SELECT * FROM decisions WHERE project = ? AND id = ?").bind(project).bind(id).fetch_optional(&self.pool).await?;
        row.as_ref().map(Self::decision_row).transpose()
    }

    pub async fn set_decision_feedback(&self, project: &str, id: i64, feedback: Option<bool>) -> Result<()> {
        sqlx::query("UPDATE decisions SET feedback = ? WHERE project = ? AND id = ?").bind(feedback).bind(project).bind(id).execute(&self.pool).await?;
        Ok(())
    }

    fn case_row(row: &SqliteRow) -> Result<ReflexCase> {
        let answer: Option<String> = row.get("answer");
        let confidence: Option<f64> = row.get("confidence");
        let correct: Option<bool> = row.get("correct");
        let at: Option<DateTime<Utc>> = row.get("scored_at");
        let score = match (answer, confidence, correct, at) {
            (Some(answer), Some(confidence), Some(correct), Some(at)) => Some(CaseScore { answer: serde_json::from_str(&answer)?, confidence, correct, at }),
            _ => None,
        };
        Ok(ReflexCase {
            id: row.get("id"),
            question_id: row.get("question_id"),
            text: row.get("text"),
            expected: serde_json::from_str(row.get("expected"))?,
            created_at: row.get("created_at"),
            score,
        })
    }

    /// Inserts the cases in one transaction and returns them with their ids.
    pub async fn add_cases(&self, project: &str, cases: &[(String, String, serde_json::Value)]) -> Result<Vec<ReflexCase>> {
        let now = Utc::now();
        let mut tx = self.pool.begin().await?;
        let mut ids = vec![];
        for (question_id, text, expected) in cases {
            let id: i64 = sqlx::query("INSERT INTO reflex_cases (project, question_id, text, expected, created_at) VALUES (?, ?, ?, ?, ?) RETURNING id")
                .bind(project)
                .bind(question_id)
                .bind(text)
                .bind(expected.to_string())
                .bind(now)
                .fetch_one(&mut *tx)
                .await?
                .get("id");
            ids.push(id);
        }
        tx.commit().await?;
        let mut out = vec![];
        for id in ids {
            out.extend(self.case(project, id).await?);
        }
        Ok(out)
    }

    pub async fn case(&self, project: &str, id: i64) -> Result<Option<ReflexCase>> {
        let row = sqlx::query("SELECT * FROM reflex_cases WHERE project = ? AND id = ?").bind(project).bind(id).fetch_optional(&self.pool).await?;
        row.as_ref().map(Self::case_row).transpose()
    }

    /// Oldest first.
    pub async fn cases(&self, project: &str) -> Result<Vec<ReflexCase>> {
        let rows = sqlx::query("SELECT * FROM reflex_cases WHERE project = ? ORDER BY id").bind(project).fetch_all(&self.pool).await?;
        rows.iter().map(Self::case_row).collect()
    }

    /// Returns whether the case existed.
    pub async fn delete_case(&self, project: &str, id: i64) -> Result<bool> {
        let done = sqlx::query("DELETE FROM reflex_cases WHERE project = ? AND id = ?").bind(project).bind(id).execute(&self.pool).await?;
        Ok(done.rows_affected() > 0)
    }

    /// `None` clears the score (the case could not be run).
    pub async fn save_case_score(&self, id: i64, score: Option<&CaseScore>) -> Result<()> {
        sqlx::query("UPDATE reflex_cases SET answer = ?, confidence = ?, correct = ?, scored_at = ? WHERE id = ?")
            .bind(score.map(|s| s.answer.to_string()))
            .bind(score.map(|s| s.confidence))
            .bind(score.map(|s| s.correct))
            .bind(score.map(|s| s.at))
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Counts and average time per lane, feedback, and hourly buckets over the `hours` up to `now`.
    pub async fn decision_summary(&self, project: &str, hours: i64, now: DateTime<Utc>) -> Result<DecisionSummary> {
        use chrono::{Duration, DurationRound};
        let hours = hours.clamp(1, 24 * 30);
        // Buckets are whole clock hours; the last one is the current, partial hour.
        let first = now.duration_trunc(Duration::hours(1))? - Duration::hours(hours - 1);
        let rows = sqlx::query("SELECT at, lane, ms, feedback FROM decisions WHERE project = ? AND at >= ?").bind(project).bind(first).fetch_all(&self.pool).await?;
        let mut buckets: Vec<HourBucket> = (0..hours).map(|i| HourBucket { start: first + Duration::hours(i), reflex: 0, llm: 0, abstain: 0 }).collect();
        let (mut reflex, mut llm, mut abstain) = (LaneSummary::default(), LaneSummary::default(), LaneSummary::default());
        let (mut right, mut wrong) = (0, 0);
        for row in &rows {
            let at: DateTime<Utc> = row.get("at");
            if at < first {
                continue;
            }
            let lane: Lane = parse_enum(row.get("lane"))?;
            let ms: i64 = row.get("ms");
            match row.get::<Option<bool>, _>("feedback") {
                Some(true) => right += 1,
                Some(false) => wrong += 1,
                None => {}
            }
            let i = (at - first).num_hours();
            let bucket = usize::try_from(i).ok().and_then(|i| buckets.get_mut(i));
            let (summary, slot) = match lane {
                Lane::Reflex => (&mut reflex, bucket.map(|b| &mut b.reflex)),
                Lane::Llm => (&mut llm, bucket.map(|b| &mut b.llm)),
                Lane::Abstain => (&mut abstain, bucket.map(|b| &mut b.abstain)),
            };
            summary.count += 1;
            summary.avg_ms += ms as f64;
            if let Some(slot) = slot {
                *slot += 1;
            }
        }
        for s in [&mut reflex, &mut llm, &mut abstain] {
            if s.count > 0 {
                s.avg_ms /= s.count as f64;
            }
        }
        Ok(DecisionSummary {
            hours,
            since: first,
            total: reflex.count + llm.count + abstain.count,
            reflex,
            llm,
            abstain,
            feedback_right: right,
            feedback_wrong: wrong,
            buckets,
        })
    }

    // ---- reflex savings (savings.rs)

    /// Every decision since `since` (of `project`, or of all projects), as the savings report reads it. Rows from
    /// before migration 0008 are estimated from their state and question.
    pub async fn savings_samples(&self, project: Option<&str>, since: DateTime<Utc>) -> Result<Vec<savings::Sample>> {
        let rows = sqlx::query(
            "SELECT project, at, lane, ms, prompt_chars, tokens_in, tokens_out, tokens_measured, fallback, skipped_thai,
               CASE WHEN prompt_chars IS NULL THEN state END AS state,
               CASE WHEN prompt_chars IS NULL THEN question END AS question,
               CASE WHEN prompt_chars IS NULL THEN answer END AS answer
             FROM decisions WHERE at >= ?1 AND (?2 IS NULL OR project = ?2) ORDER BY at",
        )
        .bind(since)
        .bind(project)
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                let (project, at, lane, ms) = (row.get("project"), row.get("at"), parse_enum(row.get("lane"))?, row.get("ms"));
                let Some(prompt_chars) = row.get::<Option<i64>, _>("prompt_chars") else {
                    let question = serde_json::from_str(row.get::<Option<&str>, _>("question").unwrap_or("null"))?;
                    let answer = serde_json::from_str(row.get::<Option<&str>, _>("answer").unwrap_or("null"))?;
                    let state: Option<String> = row.get("state");
                    return Ok(savings::Sample::legacy(project, at, lane, ms, state.as_deref().unwrap_or_default(), &question, &answer));
                };
                let cost = DecisionCost {
                    prompt_chars: Some(prompt_chars),
                    tokens_in: row.get("tokens_in"),
                    tokens_out: row.get("tokens_out"),
                    measured: row.get("tokens_measured"),
                    fallback: row.get("fallback"),
                    skipped_thai: row.get("skipped_thai"),
                };
                Ok(savings::Sample::recorded(project, at, lane, ms, &cost))
            })
            .collect()
    }

    /// Per project: the llm-lane decisions since `since` whose tokens the provider reported, as
    /// (count, sum of input + output tokens, sum of prompt chars).
    pub async fn measured_calls(&self, since: DateTime<Utc>) -> Result<Vec<(String, i64, i64, i64)>> {
        let rows = sqlx::query(
            "SELECT project, COUNT(*) AS n, SUM(tokens_in + tokens_out) AS tokens, SUM(prompt_chars) AS chars FROM decisions
             WHERE lane = 'llm' AND tokens_measured = 1 AND tokens_in IS NOT NULL AND tokens_out IS NOT NULL AND prompt_chars IS NOT NULL
               AND at >= ? GROUP BY project",
        )
        .bind(since)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.iter().map(|r| (r.get("project"), r.get("n"), r.get("tokens"), r.get("chars"))).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Role, SessionState};

    #[tokio::test]
    async fn round_trips_sessions_tasks_and_events() {
        let store = Store::memory().await.unwrap();
        let now = Utc::now();
        let mut s = Session {
            name: "map-a".into(),
            provider: "claude".into(),
            model: "sonnet".into(),
            role: Role::Manual,
            task_id: None,
            project: None,
            state: SessionState::Starting,
            detail: None,
            rate_limit_reset_at: None,
            wakeups: 0,
            last_line: None,
            attached: false,
            created_at: now,
            last_activity_at: now,
        };
        store.save_session(&s).await.unwrap();
        s.state = SessionState::RateLimited;
        s.rate_limit_reset_at = Some(now);
        store.save_session(&s).await.unwrap();
        let got = store.session("map-a").await.unwrap().unwrap();
        assert_eq!(got.state, SessionState::RateLimited);
        assert!(got.rate_limit_reset_at.is_some());

        let t = store
            .create_task(NewTask { title: "T".into(), prompt: "P".into(), depends_on: vec![3, 4], provider: None, model: None, project: None })
            .await
            .unwrap();
        assert_eq!(t.depends_on, vec![3, 4]);
        assert_eq!(t.status, TaskStatus::Pending);

        store.insert_event(Level::Warning, "x", Some("map-a"), None, "one").await.unwrap();
        store.insert_event(Level::Info, "y", None, Some(t.id), "two").await.unwrap();
        assert_eq!(store.events(10, None).await.unwrap()[0].message, "two");
        assert_eq!(store.events(10, Some("map-a")).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn round_trips_projects_and_decisions() {
        use crate::model::{Question, QuestionKind};
        let store = Store::memory().await.unwrap();
        let mut p = Project {
            slug: "support".into(),
            name: "Support".into(),
            description: String::new(),
            port: store.free_port(7400).await.unwrap(),
            threshold: None,
            fallback: "api".into(),
            model: None,
            questions: vec![Question { id: "q".into(), kind: QuestionKind::Noul, prompt: "Urgent?".into(), options: None, criteria: None }],
            created_at: Utc::now(),
            settings: Default::default(),
        };
        store.save_project(&p).await.unwrap();
        assert_eq!(store.free_port(7400).await.unwrap(), 7401);
        p.threshold = Some(1e-5);
        p.settings.paused = true;
        p.settings.verify = Some(vec!["cargo test".into()]);
        store.save_project(&p).await.unwrap();
        let back = store.project("support").await.unwrap().unwrap();
        assert_eq!(back.threshold, Some(1e-5));
        assert_eq!(back.settings, p.settings);

        let d = Decision {
            id: 0,
            project: "support".into(),
            at: Utc::now(),
            state: "s".into(),
            question: p.questions[0].clone(),
            answer: serde_json::Value::Bool(true),
            lane: Lane::Reflex,
            confidence: Some(0.5),
            reflex_answer: serde_json::Value::Bool(true),
            ms: 1,
            error: None,
            feedback: None,
        };
        store.insert_decision(&d, &DecisionCost::default()).await.unwrap();
        store.insert_decision(&Decision { lane: Lane::Llm, ..d.clone() }, &DecisionCost::default()).await.unwrap();
        let stats = store.decision_stats("support").await.unwrap();
        assert_eq!((stats.total, stats.reflex, stats.llm), (2, 1, 1));
        assert_eq!(store.decisions("support", 10).await.unwrap()[0].lane, Lane::Llm);

        store.delete_project("support").await.unwrap();
        assert!(store.project("support").await.unwrap().is_none());
        assert_eq!(store.decision_stats("support").await.unwrap().total, 0);
    }

    async fn reflex_fixture() -> (Store, Decision) {
        use crate::model::{Question, QuestionKind};
        let store = Store::memory().await.unwrap();
        let q = Question { id: "route".into(), kind: QuestionKind::Choice, prompt: "Team?".into(), options: Some(vec!["billing".into(), "shipping".into()]), criteria: None };
        let p = Project {
            slug: "support".into(),
            name: "Support".into(),
            description: String::new(),
            port: 7400,
            threshold: None,
            fallback: "none".into(),
            model: None,
            questions: vec![q.clone()],
            settings: Default::default(),
            created_at: Utc::now(),
        };
        store.save_project(&p).await.unwrap();
        let d = Decision {
            id: 0,
            project: "support".into(),
            at: Utc::now(),
            state: "charged twice".into(),
            question: q,
            answer: serde_json::json!("billing"),
            lane: Lane::Reflex,
            confidence: Some(2e-4),
            reflex_answer: serde_json::json!("billing"),
            ms: 4,
            error: None,
            feedback: None,
        };
        (store, d)
    }

    #[tokio::test]
    async fn decision_feedback_round_trips() {
        let (store, d) = reflex_fixture().await;
        let id = store.insert_decision(&d, &DecisionCost::default()).await.unwrap();
        assert_eq!(store.decision("support", id).await.unwrap().unwrap().feedback, None);
        store.set_decision_feedback("support", id, Some(false)).await.unwrap();
        assert_eq!(store.decision("support", id).await.unwrap().unwrap().feedback, Some(false));
        assert!(store.decision("other", id).await.unwrap().is_none());
        assert_eq!(store.decisions("support", 5).await.unwrap()[0].feedback, Some(false));
    }

    #[tokio::test]
    async fn calibration_cases_are_added_scored_and_deleted() {
        let (store, _) = reflex_fixture().await;
        let added = store
            .add_cases("support", &[("route".into(), "charged twice".into(), serde_json::json!("billing")), ("route".into(), "parcel lost".into(), serde_json::json!("shipping"))])
            .await
            .unwrap();
        assert_eq!(added.len(), 2);
        assert!(added.iter().all(|c| c.score.is_none()));
        let score = CaseScore { answer: serde_json::json!("billing"), confidence: 3.2e-4, correct: false, at: Utc::now() };
        store.save_case_score(added[1].id, Some(&score)).await.unwrap();
        let cases = store.cases("support").await.unwrap();
        assert_eq!(cases.iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["charged twice", "parcel lost"]);
        let got = cases[1].score.as_ref().unwrap();
        assert_eq!((got.confidence, got.correct, got.answer.clone()), (3.2e-4, false, serde_json::json!("billing")));
        assert_eq!(cases[1].expected, serde_json::json!("shipping"));
        store.save_case_score(added[1].id, None).await.unwrap();
        assert!(store.case("support", added[1].id).await.unwrap().unwrap().score.is_none());

        assert!(store.delete_case("support", added[0].id).await.unwrap());
        assert!(!store.delete_case("support", added[0].id).await.unwrap());
        assert!(!store.delete_case("other", added[1].id).await.unwrap());
        store.delete_project("support").await.unwrap();
        assert!(store.cases("support").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn summary_buckets_lanes_by_hour() {
        let (store, d) = reflex_fixture().await;
        let now = Utc::now();
        let hour = chrono::Duration::hours(1);
        store.insert_decision(&Decision { at: now, ms: 2, ..d.clone() }, &DecisionCost::default()).await.unwrap();
        store.insert_decision(&Decision { at: now, ms: 6, ..d.clone() }, &DecisionCost::default()).await.unwrap();
        let llm = store.insert_decision(&Decision { at: now - hour * 3, lane: Lane::Llm, ms: 1500, ..d.clone() }, &DecisionCost::default()).await.unwrap();
        store.insert_decision(&Decision { at: now - hour * 5, lane: Lane::Abstain, ms: 10, ..d.clone() }, &DecisionCost::default()).await.unwrap();
        store.insert_decision(&Decision { at: now - hour * 30, lane: Lane::Llm, ms: 900, ..d.clone() }, &DecisionCost::default()).await.unwrap();
        store.set_decision_feedback("support", llm, Some(true)).await.unwrap();

        let s = store.decision_summary("support", 24, now).await.unwrap();
        assert_eq!(s.buckets.len(), 24);
        assert_eq!((s.total, s.reflex.count, s.llm.count, s.abstain.count), (4, 2, 1, 1));
        assert_eq!((s.reflex.avg_ms, s.llm.avg_ms), (4.0, 1500.0));
        assert_eq!((s.feedback_right, s.feedback_wrong), (1, 0));
        let last = s.buckets.last().unwrap();
        assert_eq!((last.reflex, last.llm), (2, 0));
        assert_eq!(s.buckets[23 - 3].llm, 1);
        assert_eq!(s.buckets[23 - 5].abstain, 1);
        assert!(s.buckets.windows(2).all(|w| w[1].start - w[0].start == hour));
    }

    #[tokio::test]
    async fn savings_samples_read_costs_and_old_rows() {
        let (store, d) = reflex_fixture().await;
        let now = Utc::now();
        let day = chrono::Duration::days(1);
        let api = DecisionCost { prompt_chars: Some(200), tokens_in: Some(50), tokens_out: Some(2), measured: true, fallback: Some("api".into()), skipped_thai: false };
        for _ in 0..3 {
            store.insert_decision(&Decision { lane: Lane::Llm, ..d.clone() }, &api).await.unwrap();
        }
        // Estimated (CLI) and old rows are not measured calls; neither is one from 40 days ago.
        let cli = DecisionCost { measured: false, fallback: Some("claude".into()), ..api.clone() };
        store.insert_decision(&Decision { lane: Lane::Llm, ..d.clone() }, &cli).await.unwrap();
        store.insert_decision(&Decision { lane: Lane::Llm, at: now - day * 40, ..d.clone() }, &api).await.unwrap();
        let thai = DecisionCost { prompt_chars: Some(90), fallback: Some("none".into()), skipped_thai: true, ..DecisionCost::default() };
        store.insert_decision(&Decision { lane: Lane::Abstain, state: "ด่วน".into(), ..d.clone() }, &thai).await.unwrap();
        store.insert_decision(&d, &DecisionCost::default()).await.unwrap();

        let measured = store.measured_calls(now - day * 30).await.unwrap();
        assert_eq!(measured, vec![("support".to_string(), 3, 156, 600)]);

        let samples = store.savings_samples(Some("support"), now - day * 30).await.unwrap();
        assert_eq!(samples.len(), 6);
        assert_eq!(samples[0].spent, Some((52, true)));
        assert_eq!((samples[3].spent, samples[3].fallback.as_deref()), (Some((52, false)), Some("claude")));
        assert!(samples[4].skipped_thai && samples[4].spent.is_none());
        // The old row: its prompt rebuilt from the stored state and question.
        let old = &samples[5];
        assert_eq!(old.prompt_chars, crate::decide::prompt("charged twice", &d.question).chars().count() as i64);
        assert_eq!((old.fallback.clone(), old.lane), (None, Lane::Reflex));
        assert!(store.savings_samples(Some("other"), now - day * 30).await.unwrap().is_empty());
        assert_eq!(store.savings_samples(None, now - day * 50).await.unwrap().len(), 7);
    }
}
