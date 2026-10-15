//! The intake gate: what the orchestrator makes of a pending task before any LLM sees it. First a local look for a
//! done task of the project it nearly repeats (character trigrams, so Thai and English alike); then one call to the
//! gates Reflex (Thai through the project's glossary) with `intake` (unclear, plan_first, or ready as light or heavy,
//! which is the tier too), and `shared` (shared files or its own) when the task would share a folder with the parallel
//! analyst on. Only a confident Reflex holds a task; unsure, below the threshold or not running, it goes on as before.

use chrono::Utc;

use crate::decisions::{Kind, record};
use crate::gates::{self, Said};
use crate::hub::Hub;
use crate::model::{BranchMode, IntakeVerdict, ProjectSettings, Task, TaskIntake, TaskScope, TaskStatus};

/// Share of the same character trigrams from which a task repeats a done one.
pub const DUPLICATE_AT: f64 = 0.7;

fn text(t: &Task) -> String {
    format!("{}\n{}", t.title, t.prompt)
}

/// The done task of the same project `task` nearly repeats, with the similarity; a task it depends on is a step
/// before it, not a repeat.
pub fn duplicate_of(task: &Task, tasks: &[Task]) -> Option<(i64, f64)> {
    let mine = text(task);
    tasks
        .iter()
        .filter(|t| t.id != task.id && t.project == task.project && t.status == TaskStatus::Done && !task.depends_on.contains(&t.id))
        .map(|t| (t.id, crate::thai::similarity(&mine, &text(t))))
        .filter(|(_, s)| *s >= DUPLICATE_AT)
        .max_by(|a, b| a.1.total_cmp(&b.1))
}

/// Runs the gate for `task` and saves the verdict on it (with a scope when Reflex is sure it touches shared files).
pub async fn run(hub: &Hub, mut task: Task, settings: &ProjectSettings, tasks: &[Task]) -> anyhow::Result<Task> {
    let Some(slug) = task.project.clone() else { return Ok(task) };
    let mut intake = TaskIntake { at: Some(Utc::now()), ..TaskIntake::default() };
    if let Some((id, s)) = duplicate_of(&task, tasks) {
        intake.verdict = IntakeVerdict::Duplicate;
        intake.similar = Some(id);
        intake.reason = format!("{:.0}% the same text as #{id}, already done", s * 100.0);
        record(hub, Kind::INTAKE_DUPLICATE, &task, &intake.reason).await;
        task.intake = Some(intake);
        hub.put_task(&mut task).await?;
        return Ok(task);
    }

    // `intake` answers the tier too (light, heavy); the tier gate uses it without asking again.
    let mut qs = vec![gates::intake_question()];
    intake.tier_asked = settings.reflex_gates.tier && task.model.is_none();
    let ask_shared = settings.branches.mode == BranchMode::Shared && settings.parallel.analyst && task.scope.is_none();
    if ask_shared {
        qs.push(gates::shared_question());
    }
    let said = gates::ask_many(hub, &slug, &gates::task_state(&task), qs.clone()).await;
    let of = |id: &str| qs.iter().position(|q| q.id == id).and_then(|i| said.get(i));

    let main = of(gates::INTAKE).cloned().unwrap_or_else(|| Said::Unsure("Reflex gave no answer".into()));
    let (verdict, kind) = match &main {
        Said::Answer { answer, .. } => match answer.as_str() {
            Some("unclear") => (IntakeVerdict::Unclear, Kind::INTAKE_UNCLEAR),
            Some("plan_first") => (IntakeVerdict::PlanFirst, Kind::INTAKE_PLAN_FIRST),
            _ => (IntakeVerdict::Ready, Kind::INTAKE_READY),
        },
        Said::Unsure(_) => (IntakeVerdict::Unsure, Kind::INTAKE_UNSURE),
    };
    intake.verdict = verdict;
    intake.reason = match &main {
        Said::Answer { answer, .. } => format!("intake: {} · {}", answer.as_str().unwrap_or("?"), main.detail()),
        Said::Unsure(why) => format!("intake: {why}"),
    };
    if let Said::Answer { answer, .. } = &main {
        intake.tier = gates::tier_of(answer).map(String::from);
    }
    if let Some(s @ Said::Answer { answer, .. }) = of(gates::SHARED)
        && answer.as_str() == Some("shared")
    {
        intake.shared_files = true;
        let why = format!("Reflex: changes files other tasks share · {}", s.detail());
        task.scope = Some(TaskScope { paths: vec!["*".into()], independent: false, reason: why.clone(), by: "reflex".into(), at: Utc::now() });
        record(hub, Kind::INTAKE_SHARED, &task, why).await;
    }
    record(hub, kind, &task, &intake.reason).await;
    if intake.holds() {
        let what = if verdict == IntakeVerdict::Unclear { "too unclear to start: say more, or start it anyway" } else { "big enough to plan first: plan it, or start it anyway" };
        hub.event(crate::model::Level::Warning, "task.held", None, Some(task.id), format!("Reflex: #{} is {what}", task.id)).await;
    }
    task.intake = Some(intake);
    hub.put_task(&mut task).await?;
    Ok(task)
}

/// The owner starts a held task anyway.
pub async fn start_anyway(hub: &Hub, id: i64) -> anyhow::Result<Task> {
    let mut task = hub.store.task(id).await?.ok_or_else(|| anyhow::anyhow!("no task #{id}"))?;
    let Some(intake) = task.intake.as_mut().filter(|i| i.holds()) else { anyhow::bail!("task #{id} is not held by the intake gate") };
    intake.released = true;
    record(hub, Kind::INTAKE_RELEASED, &task, "the owner started it anyway").await;
    hub.put_task(&mut task).await?;
    Ok(task)
}

/// The owner answers an unclear task: the note is added to its prompt and the intake gate looks again.
pub async fn clarify(hub: &Hub, id: i64, note: &str) -> anyhow::Result<Task> {
    let note = note.trim();
    if note.is_empty() {
        anyhow::bail!("say what the task should do");
    }
    let mut task = hub.store.task(id).await?.ok_or_else(|| anyhow::anyhow!("no task #{id}"))?;
    if task.status != TaskStatus::Pending {
        anyhow::bail!("only a pending task can be clarified");
    }
    task.prompt = format!("{}\n\nThe owner adds:\n{note}", task.prompt.trim_end());
    task.intake = None;
    hub.put_task(&mut task).await?;
    hub.event(crate::model::Level::Info, "task.clarified", None, Some(id), format!("#{id}: the owner added to the task; intake looks again")).await;
    Ok(task)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: i64, title: &str, prompt: &str, status: TaskStatus) -> Task {
        Task {
            id,
            title: title.into(),
            prompt: prompt.into(),
            depends_on: vec![],
            provider: None,
            model: None,
            project: Some("p".into()),
            status,
            session: None,
            attempts: 0,
            result: None,
            git: None,
            scope: None,
            intake: None,
            created_by: String::new(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn a_near_copy_of_a_done_task_is_a_duplicate() {
        let done = task(1, "เพิ่มปุ่มส่งออก CSV", "เพิ่มปุ่มส่งออก CSV ในหน้ารายงานยอดขาย", TaskStatus::Done);
        let open = task(2, "เพิ่มปุ่มส่งออก CSV", "เพิ่มปุ่มส่งออก CSV ในหน้ารายงานยอดขาย ด้วยครับ", TaskStatus::Pending);
        let other = task(3, "แก้บั๊กเข้าสู่ระบบ", "ผู้ใช้ล็อกอินไม่ได้หลังเปลี่ยนรหัสผ่าน", TaskStatus::Pending);
        let all = vec![done.clone(), open.clone(), other.clone()];
        assert_eq!(duplicate_of(&open, &all).map(|d| d.0), Some(1));
        assert_eq!(duplicate_of(&other, &all), None);
        // Not done yet, another project, or a dependency: no duplicate.
        let mut running = done.clone();
        running.status = TaskStatus::Running;
        assert_eq!(duplicate_of(&open, &[running, open.clone()]), None);
        let mut elsewhere = done.clone();
        elsewhere.project = Some("q".into());
        assert_eq!(duplicate_of(&open, &[elsewhere, open.clone()]), None);
        let mut follow = open.clone();
        follow.depends_on = vec![1];
        assert_eq!(duplicate_of(&follow, &all), None);
    }

    #[test]
    fn only_a_confident_unclear_or_plan_first_holds() {
        let held = |v| TaskIntake { verdict: v, ..TaskIntake::default() }.holds();
        assert!(held(IntakeVerdict::Unclear) && held(IntakeVerdict::PlanFirst) && held(IntakeVerdict::Duplicate));
        assert!(!held(IntakeVerdict::Ready) && !held(IntakeVerdict::Unsure));
        assert!(!TaskIntake { verdict: IntakeVerdict::Unclear, released: true, ..TaskIntake::default() }.holds());
    }
}
