//! Backend of the dashboard's Reflex screen: labeled calibration cases (scored by Reflex as they are added, then
//! fitted into the project's threshold), feedback on logged decisions, and archiving corpus domains past Reflex's
//! limit of 8.

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::{Value, json};

use crate::decide::{self, same};
use crate::hub::Hub;
use crate::model::{ArchiveResult, CaseScore, DecisionFeedback, FitResult, Project, Question, QuestionKind, ReflexCase, RepoFile};
use crate::{reflex, repo};

/// Fitting a threshold on fewer cases than this is noise.
pub const MIN_CASES: usize = 16;

/// The expected answer in the question's terms: an option's exact text (matched without case), or a boolean for a
/// yes/no question (`yes`/`no`/`true`/`false` accepted).
pub fn parse_expected(q: &Question, v: &Value) -> Result<Value> {
    if q.kind == QuestionKind::Noul {
        return match v {
            Value::Bool(b) => Ok(Value::Bool(*b)),
            Value::String(s) => match s.trim().to_lowercase().as_str() {
                "yes" | "y" | "true" | "1" | "ใช่" => Ok(Value::Bool(true)),
                "no" | "n" | "false" | "0" | "ไม่" | "ไม่ใช่" => Ok(Value::Bool(false)),
                other => bail!("`{other}` is not yes or no (question `{}`)", q.id),
            },
            _ => bail!("question `{}` takes yes or no", q.id),
        };
    }
    let text = match v {
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    };
    let options = q.options.as_deref().unwrap_or_default();
    options
        .iter()
        .find(|o| o.trim().eq_ignore_ascii_case(&text))
        .map(|o| Value::String(o.clone()))
        .with_context(|| format!("`{text}` is not an option of `{}` ({})", q.id, options.join(", ")))
}

pub struct NewCase {
    pub question_id: String,
    pub text: String,
    pub expected: Value,
}

fn question<'a>(project: &'a Project, id: &str) -> Result<&'a Question> {
    project.questions.iter().find(|q| q.id == id).with_context(|| format!("`{id}` is not one of the project's saved questions"))
}

/// Runs one case through Reflex alone, never the fallback.
async fn score(hub: &Hub, project: &Project, q: &Question, text: &str, expected: &Value) -> Result<CaseScore> {
    let read = decide::for_reflex(project, text, std::slice::from_ref(q));
    let v = hub.reflex.post(project.port, "/decide", &json!({ "state": read.state, "questions": read.questions })).await?;
    let a = &v["answers"][0];
    let answer = decide::decode(q, &a["outcome"]);
    let correct = !answer.is_null() && same(&answer, expected);
    Ok(CaseScore { confidence: a["confidence"].as_f64().unwrap_or(0.0), correct, answer, at: Utc::now() })
}

/// Stores the cases, then scores each with Reflex so the scatter shows them at once. A case Reflex cannot score
/// (it is down) keeps no score until the next fit.
pub async fn add_cases(hub: &Hub, project: &Project, cases: Vec<NewCase>) -> Result<Vec<ReflexCase>> {
    if cases.is_empty() {
        bail!("no cases to add");
    }
    let mut rows = vec![];
    for c in cases {
        let q = question(project, &c.question_id)?;
        let text = c.text.trim().to_string();
        if text.is_empty() {
            bail!("a case needs a message");
        }
        if let Some(g) = decide::for_reflex(project, &text, std::slice::from_ref(q)).thai.filter(|g| !g.readable()) {
            bail!(
                "only {:.0}% of this Thai is in the glossary, so Reflex cannot read it (add its words under Thai words: {}): `{}`",
                g.coverage() * 100.0,
                g.unknown.iter().take(5).cloned().collect::<Vec<_>>().join(", "),
                text.chars().take(40).collect::<String>()
            );
        }
        rows.push((q.id.clone(), text, parse_expected(q, &c.expected)?));
    }
    let mut added = hub.store.add_cases(&project.slug, &rows).await?;
    for case in &mut added {
        let q = question(project, &case.question_id)?;
        match score(hub, project, q, &case.text, &case.expected).await {
            Ok(s) => {
                hub.store.save_case_score(case.id, Some(&s)).await?;
                case.score = Some(s);
            }
            Err(err) => {
                tracing::info!("{}: case {} not scored: {err:#}", project.slug, case.id);
                break;
            }
        }
    }
    Ok(added)
}

/// Re-scores every case whose question still exists and fits the threshold from them: the ρ-quantile of Reflex's
/// confidence on the cases it got right. `apply` saves it as the project's threshold.
pub async fn fit(hub: &Hub, project: &mut Project, rho: f64, apply: bool) -> Result<FitResult> {
    let cases: Vec<(ReflexCase, Question)> =
        hub.store.cases(&project.slug).await?.into_iter().filter_map(|c| question(project, &c.question_id).ok().cloned().map(|q| (c, q))).collect();
    if cases.len() < MIN_CASES {
        bail!("calibration needs at least {MIN_CASES} cases for saved questions; there are {}", cases.len());
    }
    let input: Vec<(String, Question, Value)> = cases.iter().map(|(c, q)| (c.text.clone(), q.clone(), c.expected.clone())).collect();
    let mut result = decide::fit(hub, project, &input, rho).await?;
    let at = Utc::now();
    for ((case, _), row) in cases.iter().zip(&result.cases) {
        let s = CaseScore { answer: row.answer.clone(), confidence: row.confidence, correct: row.correct, at };
        hub.store.save_case_score(case.id, Some(&s)).await?;
    }
    if apply {
        if result.threshold.is_none() {
            bail!("Reflex got none of the {} cases right, so there is no threshold to save; improve the corpus first", result.n);
        }
        project.threshold = result.threshold;
        hub.put_project(project).await?;
        result.applied = true;
    }
    Ok(result)
}

/// Records the user's verdict on a decision. A correct answer confirms `decision.answer`; a wrong one needs the
/// right `expected`. Reflex's calibrator is told whether Reflex's own answer was right (whatever lane answered),
/// and with `add_case` the message becomes a calibration case.
pub async fn feedback(hub: &Hub, project: &Project, id: i64, correct: bool, expected: Option<Value>, add_case: bool) -> Result<DecisionFeedback> {
    let mut decision = hub.store.decision(&project.slug, id).await?.with_context(|| format!("no decision {id} in this project"))?;
    if decision.feedback.is_some() {
        bail!("this decision already has feedback");
    }
    let q = project.questions.iter().find(|q| q.id == decision.question.id).unwrap_or(&decision.question).clone();
    let expected = if correct {
        if decision.answer.is_null() {
            bail!("nobody answered this decision; mark it wrong and pick the correct answer");
        }
        decision.answer.clone()
    } else {
        let e = parse_expected(&q, &expected.context("pick the correct answer")?)?;
        if same(&e, &decision.answer) {
            bail!("that is the answer it gave; mark it correct instead");
        }
        e
    };
    hub.store.set_decision_feedback(&project.slug, id, Some(correct)).await?;
    decision.feedback = Some(correct);

    let saved = project.questions.iter().any(|x| x.id == q.id);
    let readable = decide::for_reflex(project, &decision.state, std::slice::from_ref(&q)).unreadable().is_none();
    let case = if add_case && saved && readable {
        add_cases(hub, project, vec![NewCase { question_id: q.id.clone(), text: decision.state.clone(), expected: expected.clone() }]).await?.pop()
    } else {
        None
    };

    let (mut reflex_outcome, mut refit, mut reflex_error) = (None, false, None);
    if let (false, Some(p)) = (decision.reflex_answer.is_null(), decision.confidence) {
        let outcome = same(&decision.reflex_answer, &expected);
        reflex_outcome = Some(outcome);
        // A gate question was answered by the gates Reflex: its calibrator learns.
        let port = if crate::gates::is_gate(&q.id) { hub.gates_target().port } else { project.port };
        match hub.reflex.post(port, "/feedback", &json!({ "p": p, "outcome": outcome })).await {
            Ok(v) => refit = v["refit"].as_bool().unwrap_or(false),
            Err(err) => reflex_error = Some(format!("{err:#}")),
        }
    }
    hub.announce_project(&project.slug).await;
    Ok(DecisionFeedback { decision, case, reflex_outcome, refit, reflex_error })
}

/// Domain → its file paths and their total size in bytes.
pub type Domains = BTreeMap<String, (Vec<String>, u64)>;

/// Files per corpus domain, live and archived. Only `.md` files count, as in
/// `repo::export_corpus`.
pub fn corpus_domains(files: &[RepoFile]) -> (Domains, Domains) {
    let (mut live, mut archived) = (BTreeMap::new(), BTreeMap::new());
    for f in files {
        let segs: Vec<&str> = f.path.split('/').collect();
        if !f.path.ends_with(".md") || segs.iter().any(|s| s.starts_with('.')) {
            continue;
        }
        let (map, rest): (&mut Domains, &[&str]) =
            if segs[0] == repo::ARCHIVE_DIR { (&mut archived, &segs[1..]) } else { (&mut live, &segs[..]) };
        let Some(first) = rest.first() else { continue };
        if rest.len() == 1 && first.eq_ignore_ascii_case("readme.md") {
            continue;
        }
        let domain = first.trim_end_matches(".md").to_string();
        let entry = map.entry(domain).or_insert_with(|| (vec![], 0));
        entry.0.push(f.path.clone());
        entry.1 += f.size;
    }
    (live, archived)
}

/// The domains to archive when none are named: the smallest (fewest files, then fewest bytes) until 8 are left.
pub fn smallest_over_limit(live: &Domains) -> Vec<String> {
    let mut by_size: Vec<(&String, usize, u64)> = live.iter().map(|(d, (paths, bytes))| (d, paths.len(), *bytes)).collect();
    by_size.sort_by(|a, b| (a.1, a.2, a.0).cmp(&(b.1, b.2, b.0)));
    by_size.into_iter().take(live.len().saturating_sub(reflex::MAX_DOMAINS)).map(|(d, ..)| d.clone()).collect()
}

/// Moves corpus domains to `_archive/` (or back with `restore`) in one commit, then restarts Reflex.
pub async fn archive(hub: &Hub, slug: &str, domains: Option<Vec<String>>, restore: bool) -> Result<ArchiveResult> {
    let repo_path = hub.corpus_repo(slug);
    let files = repo::files(&repo_path).await?;
    let (live, archived) = corpus_domains(&files);
    let (from, to_count) = if restore { (&archived, live.len()) } else { (&live, 0) };
    let targets = match domains.filter(|d| !d.is_empty()) {
        Some(d) => d,
        None if restore => bail!("name the domains to restore"),
        None => smallest_over_limit(&live),
    };
    if targets.is_empty() {
        bail!("the corpus has {} domains, within Reflex's limit of {}: nothing to archive", live.len(), reflex::MAX_DOMAINS);
    }
    if restore && to_count + targets.len() > reflex::MAX_DOMAINS {
        bail!("restoring {} would make {} domains; Reflex takes at most {}", targets.join(", "), to_count + targets.len(), reflex::MAX_DOMAINS);
    }
    let mut changes = vec![];
    for d in &targets {
        let (paths, _) = from.get(d).with_context(|| format!("no {} domain `{d}`", if restore { "archived" } else { "live" }))?;
        for path in paths {
            let content = repo::read(&repo_path, path).await.with_context(|| format!("reading {path}"))?;
            let dest = match path.strip_prefix(&format!("{}/", repo::ARCHIVE_DIR)) {
                Some(rest) if restore => rest.to_string(),
                _ => format!("{}/{path}", repo::ARCHIVE_DIR),
            };
            changes.push((dest, Some(content)));
            changes.push((path.clone(), None));
        }
    }
    let message = format!("{} {}", if restore { "Restore domains" } else { "Archive domains" }, targets.join(", "));
    let commit = repo::commit(&repo_path, &changes, &message).await?;
    let reflex = hub.restart_reflex(slug).await?;
    Ok(ArchiveResult { moved: targets, files: changes.len() / 2, commit, reflex })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(kind: QuestionKind, options: Option<&[&str]>) -> Question {
        Question { id: "q".into(), kind, prompt: "p".into(), options: options.map(|o| o.iter().map(|s| s.to_string()).collect()), criteria: None }
    }

    #[test]
    fn expected_answers_are_normalised() {
        let choice = q(QuestionKind::Choice, Some(&["db-failover", "none"]));
        assert_eq!(parse_expected(&choice, &json!(" DB-Failover ")).unwrap(), json!("db-failover"));
        assert!(parse_expected(&choice, &json!("rollback")).is_err());
        let yn = q(QuestionKind::Noul, None);
        assert_eq!(parse_expected(&yn, &json!("Yes")).unwrap(), json!(true));
        assert_eq!(parse_expected(&yn, &json!(false)).unwrap(), json!(false));
        assert!(parse_expected(&yn, &json!("maybe")).is_err());
    }

    fn file(path: &str, size: u64) -> RepoFile {
        RepoFile { path: path.into(), size }
    }

    #[test]
    fn domains_and_the_smallest_over_the_limit() {
        let mut files = vec![file("README.md", 5), file("notes.txt", 5), file(".hidden/x.md", 5), file("_archive/old/a.md", 7)];
        for (i, d) in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"].iter().enumerate() {
            // `c` and `f` have one file each; `c` is smaller in bytes.
            files.push(file(&format!("{d}/one.md"), 100 + i as u64));
            if *d != "c" && *d != "f" {
                files.push(file(&format!("{d}/two.md"), 100));
            }
        }
        files.push(file("solo.md", 1));
        let (live, archived) = corpus_domains(&files);
        assert_eq!(live.len(), 11);
        assert_eq!(archived.keys().collect::<Vec<_>>(), vec!["old"]);
        assert_eq!(live["solo"].0, vec!["solo.md"]);
        assert_eq!(smallest_over_limit(&live), vec!["solo", "c", "f"]);
        let few: BTreeMap<_, _> = live.into_iter().take(8).collect();
        assert!(smallest_over_limit(&few).is_empty());
    }

    #[tokio::test]
    async fn archives_the_smallest_domains_and_restores_them() {
        use crate::config::Config;
        use crate::store::Store;
        use crate::terminal::fake::FakeTerminal;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[reflex]\nbinary = \"{0}/no-reflex\"\n",
            dir.path().display()
        );
        let config = Config::parse(&text, std::path::Path::new("/")).unwrap();
        let hub = Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default()));
        assert_eq!(hub.server_info().reflex_version, None);
        let p = crate::projects::create(
            &hub,
            crate::projects::NewProject { name: "Ops".into(), slug: None, description: String::new(), fallback: Some("none".into()), model: None },
        )
        .await
        .unwrap();
        let repo_path = hub.corpus_repo(&p.slug);
        let mut files: Vec<(String, Option<String>)> = vec![("example/example.md".into(), None)];
        for d in ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"] {
            files.push((format!("{d}/one.md"), Some(format!("{d} notes"))));
            if d != "b" {
                files.push((format!("{d}/two.md"), Some("more".into())));
            }
        }
        repo::commit(&repo_path, &files, "ten domains").await.unwrap();

        let r = archive(&hub, &p.slug, None, false).await.unwrap();
        assert_eq!(r.moved, vec!["b", "a"]);
        assert_eq!(r.files, 3);
        let paths: Vec<String> = repo::files(&repo_path).await.unwrap().into_iter().map(|f| f.path).collect();
        assert!(paths.contains(&"_archive/a/two.md".to_string()) && !paths.contains(&"a/two.md".to_string()));
        assert_eq!(repo::read(&repo_path, "_archive/b/one.md").await.as_deref(), Some("b notes"));
        let live = dir.path().join("live");
        assert_eq!(repo::export_corpus(&repo_path, &live).await.unwrap().len(), 8);

        assert!(archive(&hub, &p.slug, None, false).await.is_err());
        assert!(archive(&hub, &p.slug, Some(vec!["a".into()]), true).await.is_err(), "9 domains is over the limit");
        archive(&hub, &p.slug, Some(vec!["c".into()]), false).await.unwrap();
        let r = archive(&hub, &p.slug, Some(vec!["b".into()]), true).await.unwrap();
        assert_eq!(r.moved, vec!["b"]);
        assert_eq!(repo::read(&repo_path, "b/one.md").await.as_deref(), Some("b notes"));
    }
}
