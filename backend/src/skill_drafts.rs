//! Skills from finished work (Hermes Agent's learning loop), as proposals. When a hard task is done (Reflex tiered it
//! heavy, or it failed verify, audit or review on the way and came through), MAP drafts a one-task plan: read what the
//! task did and turn the repeatable part into a Claude Code skill (`.claude/skills/<name>/SKILL.md`), or update the
//! skill that already covers it, or change nothing. The owner approves or rejects it on the Plans screen; nothing is
//! written before that, and the draft costs no LLM call.
//!
//! It follows the project's RAG switch (the same "learn from this project" choice), looks at tasks finished in the
//! last day, and keeps at most one draft open per project.

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;

use crate::hub::Hub;
use crate::model::{PlanStatus, PlanSubmit, PlanTaskInput, Task, TaskStatus};

pub const TITLE: &str = "Skill from task #";
/// Event kinds that mark a task as hard won.
const STRUGGLED: [&str; 4] = ["verify.fail", "audit.fail", "review.send_back", "pick.tier_heavy"];
const LOOK_BACK_HOURS: i64 = 24;

/// The drafted task's prompt for finished task `id`.
pub fn prompt(id: i64, title: &str) -> String {
    format!(
        "Task #{id} (\"{title}\") is done. Decide whether it holds a repeatable procedure worth a Claude Code skill, and write it if so.\n\n\
         1. Read what it did: its record in the project's knowledge (`map-ask --doc map://tasks/{id}`, or `curl -s -G -H \"Authorization: Bearer $MAP_PROJECT_TOKEN\" \"$MAP_KNOWLEDGE_URL/doc\" --data-urlencode \"path=map://tasks/{id}\"`) and its commits (`git log --grep '#{id}'` or the branch it merged).\n\
         2. List the skills in `.claude/skills/`. If one already covers this kind of work, improve that skill with what this task taught (a step it missed, a trap, a check) instead of adding a new one.\n\
         3. Otherwise, if the task shows steps another task of the same kind would repeat, write `.claude/skills/<name>/SKILL.md`: frontmatter `name` and a one-line `description` that says when to use it, then the steps, the files involved, the checks that prove it worked, and the traps this task hit. Keep it under 120 lines; point at files and knowledge cards rather than copying them.\n\
         4. If nothing here is repeatable (a one-off fix), change nothing and say so in your result.\n\n\
         Commit only the skill file.",
    )
}

/// Whether `t` was hard enough to learn from, from its decision trail.
async fn struggled(hub: &Hub, t: &Task) -> bool {
    let events = hub.store.decision_events(t.id, 300).await.unwrap_or_default();
    t.attempts > 1 || events.iter().any(|e| STRUGGLED.iter().any(|k| e.kind.ends_with(k)))
}

async fn seen(hub: &Hub, task_id: i64) -> bool {
    sqlx::query("SELECT task_id FROM skill_drafts WHERE task_id = ?").bind(task_id).fetch_optional(&hub.store.pool).await.ok().flatten().is_some()
}

async fn mark(hub: &Hub, t: &Task, slug: &str, plan_id: Option<i64>) {
    let _ = sqlx::query("INSERT OR IGNORE INTO skill_drafts (task_id, project, at, plan_id) VALUES (?, ?, ?, ?)")
        .bind(t.id)
        .bind(slug)
        .bind(Utc::now())
        .bind(plan_id)
        .execute(&hub.store.pool)
        .await;
}

/// One pass: a draft for the newest hard task of each project that has none open.
pub async fn tick(hub: &Hub) {
    let since = Utc::now() - chrono::Duration::hours(LOOK_BACK_HOURS);
    let mut tasks: Vec<Task> = hub.store.tasks().await.unwrap_or_default().into_iter().filter(|t| t.status == TaskStatus::Done && t.updated_at >= since).collect();
    tasks.sort_by_key(|t| std::cmp::Reverse(t.updated_at));
    for t in tasks {
        let Some(slug) = t.project.clone() else { continue };
        if t.title.starts_with(TITLE) || seen(hub, t.id).await {
            continue;
        }
        let Ok(Some(project)) = hub.store.project(&slug).await else { continue };
        if !crate::rag::on(&project) {
            continue;
        }
        let open = hub.store.plans(Some(&slug), 50).await.unwrap_or_default().into_iter().any(|p| p.status == PlanStatus::Draft && p.title.starts_with(TITLE));
        if open {
            continue;
        }
        if !struggled(hub, &t).await {
            mark(hub, &t, &slug, None).await;
            continue;
        }
        let title = format!("{TITLE}{}: {}", t.id, t.title.chars().take(80).collect::<String>());
        let body = PlanSubmit {
            title: title.clone(),
            summary: Some(format!("Task #{} was hard won. This draft asks an agent to turn what it learned into a skill, or improve the skill that covers it. Reject it if the task was a one-off.", t.id)),
            tasks: vec![PlanTaskInput { key: "skill".into(), title, prompt: prompt(t.id, &t.title), depends_on: None, provider: None, model: None }],
            replaces: None,
            session: None,
        };
        match crate::plans::submit(hub, Some(&slug), body).await {
            Ok(plan) => mark(hub, &t, &slug, Some(plan.id)).await,
            Err(err) => tracing::warn!("{slug}: no skill draft for task #{}: {err:?}", t.id),
        }
    }
}

pub async fn run(hub: Arc<Hub>) {
    loop {
        tokio::time::sleep(Duration::from_secs(300)).await;
        tick(&hub).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_points_at_the_task_and_the_existing_skills() {
        let p = prompt(42, "Add the refund report");
        assert!(p.contains("map://tasks/42") && p.contains(".claude/skills/") && p.contains("change nothing"));
    }

    #[tokio::test]
    async fn a_hard_won_task_gets_one_draft_and_an_easy_one_none() {
        use crate::model::{ProjectSettings, ProjectSource, SourceKind};
        use crate::store::NewTask;
        let dir = tempfile::tempdir().unwrap();
        let text = format!("[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\n", dir.path().display());
        let config = crate::config::Config::parse(&text, std::path::Path::new("/")).unwrap();
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        let source = ProjectSource { kind: SourceKind::Empty, location: Some(dir.path().join("p").display().to_string()), branch: None, remote: None };
        let new = crate::projects::NewProject { name: "Shop".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None };
        let (p, _) = crate::projects::create_with(&hub, new, ProjectSettings { source, ..Default::default() }).await.unwrap();
        let task = |title: &str| NewTask { title: title.into(), prompt: "do it".into(), depends_on: vec![], provider: None, model: None, project: Some(p.slug.clone()), workflow: None };
        let mut easy = hub.store.create_task(task("Fix a typo")).await.unwrap();
        easy.status = TaskStatus::Done;
        hub.put_task(&mut easy).await.unwrap();
        tick(&hub).await;
        assert!(hub.store.plans(Some(&p.slug), 10).await.unwrap().is_empty(), "an easy task drafts nothing");
        let mut hard = hub.store.create_task(task("Refund report")).await.unwrap();
        (hard.status, hard.attempts) = (TaskStatus::Done, 2);
        hub.put_task(&mut hard).await.unwrap();
        tick(&hub).await;
        tick(&hub).await;
        let plans = hub.store.plans(Some(&p.slug), 10).await.unwrap();
        assert_eq!(plans.len(), 1, "one draft, once");
        assert!(plans[0].title.starts_with(&format!("{TITLE}{}", hard.id)) && plans[0].status == PlanStatus::Draft);
    }
}
