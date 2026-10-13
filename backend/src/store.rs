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

use crate::model::{Decision, DecisionStats, Event, Lane, Level, Project, Session, Task, TaskStatus};

#[derive(Clone)]
pub struct Store {
    pool: SqlitePool,
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
        sqlx::query("DELETE FROM projects WHERE slug = ?").bind(slug).execute(&self.pool).await?;
        Ok(())
    }

    // ---- decisions

    pub async fn insert_decision(&self, d: &Decision) -> Result<i64> {
        let id = sqlx::query(
            "INSERT INTO decisions (project, at, state, question, answer, lane, confidence, reflex_answer, ms, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
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
        .fetch_one(&self.pool)
        .await?
        .get("id");
        Ok(id)
    }

    /// Newest first.
    pub async fn decisions(&self, project: &str, limit: i64) -> Result<Vec<Decision>> {
        let rows = sqlx::query("SELECT * FROM decisions WHERE project = ? ORDER BY id DESC LIMIT ?").bind(project).bind(limit).fetch_all(&self.pool).await?;
        rows.iter()
            .map(|row| {
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
                })
            })
            .collect()
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
        };
        store.insert_decision(&d).await.unwrap();
        store.insert_decision(&Decision { lane: Lane::Llm, ..d.clone() }).await.unwrap();
        let stats = store.decision_stats("support").await.unwrap();
        assert_eq!((stats.total, stats.reflex, stats.llm), (2, 1, 1));
        assert_eq!(store.decisions("support", 10).await.unwrap()[0].lane, Lane::Llm);

        store.delete_project("support").await.unwrap();
        assert!(store.project("support").await.unwrap().is_none());
        assert_eq!(store.decision_stats("support").await.unwrap().total, 0);
    }
}
