//! Configure › Workflows: a task's steps as a graph (trigger → agent → verify → audit → hook → end) that the
//! orchestrator walks (`orchestrator/workflow.rs`) instead of its fixed pipeline.
//!
//! Machine workflows are JSON in `runtime_settings` (key `workflows`); a project's own are in its settings
//! (`ProjectSettings::workflows`), and one with a machine workflow's id overrides it in that project.
//!
//! Edges: `next` and `pass` are both "the way on after success" (`pass` out of a trigger or an agent is read as
//! `next`, `next` out of a verify, audit or hook as `pass`); `fail` leaves an agent that failed, a verify or audit
//! that did not pass, or a blocking hook that failed. A node has at most one success edge and one fail edge; a trigger
//! has no fail edge and an end no edges at all.

use std::collections::{BTreeMap, HashSet, VecDeque};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};

use crate::hub::Hub;
use crate::model::{
    AgentDef, AgentScope, ConfigSource, Task, Workflow, WorkflowEdgeKind as EK, WorkflowNode, WorkflowNodeKind as NK, WorkflowNodeStats, WorkflowOutcome,
    WorkflowRun, WorkflowStats,
};

const KEY: &str = "workflows";
/// Nodes one run may take in all; a graph whose loops never end fails after this many.
pub const MAX_STEPS: u32 = 200;
/// Runs the stats (and "suggest from runs") read.
pub const RUNS: usize = 30;
pub const DEFAULT_VERDICT: &str = "VERDICT: PASS";
pub const HOOK_TIMEOUT_SECS: u64 = 600;
/// The prefix of every workflow decision event: `decision.workflow.<node kind>.<outcome>`, message `<node id> · …`.
pub const EVENT_PREFIX: &str = "decision.workflow.";

pub fn valid_id(id: &str) -> bool {
    (1..=40).contains(&id.len()) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

// ---- storage

pub async fn machine(hub: &Hub) -> Result<Vec<Workflow>> {
    match hub.store.runtime_setting(KEY).await? {
        Some(text) => serde_json::from_str(&text).context("the stored machine workflows do not parse"),
        None => Ok(vec![]),
    }
}

async fn set_machine(hub: &Hub, list: &[Workflow]) -> Result<()> {
    let text = if list.is_empty() { None } else { Some(serde_json::to_string(list)?) };
    hub.store.set_runtime_setting(KEY, text.as_deref()).await
}

/// The machine's workflows, then the project's: one with a machine id replaces it in place (`override`).
pub fn merge(machine: Vec<Workflow>, project: Vec<Workflow>) -> Vec<Workflow> {
    let mut out: Vec<Workflow> = machine.into_iter().map(|w| Workflow { source: ConfigSource::Machine, ..w }).collect();
    for w in project {
        match out.iter_mut().find(|m| m.id == w.id) {
            Some(slot) => *slot = Workflow { source: ConfigSource::Override, ..w },
            None => out.push(Workflow { source: ConfigSource::Project, ..w }),
        }
    }
    out
}

/// The workflows tasks of `project` see (`None`: the default workspace, machine workflows only).
pub async fn list(hub: &Hub, project: Option<&str>) -> Result<Vec<Workflow>> {
    let own = match project {
        Some(slug) => hub.store.project(slug).await?.with_context(|| format!("project `{slug}` does not exist"))?.settings.workflows,
        None => vec![],
    };
    Ok(merge(machine(hub).await?, own))
}

pub async fn find(hub: &Hub, project: Option<&str>, id: &str) -> Result<Option<Workflow>> {
    Ok(list(hub, project).await?.into_iter().find(|w| w.id == id))
}

/// Trims what was typed; the name defaults to the id.
pub fn normalize(mut wf: Workflow) -> Workflow {
    wf.id = wf.id.trim().to_string();
    wf.name = wf.name.trim().to_string();
    if wf.name.is_empty() {
        wf.name = wf.id.clone();
    }
    let mut seen = HashSet::new();
    wf.trigger.keywords = wf.trigger.keywords.iter().map(|k| k.trim().to_lowercase()).filter(|k| !k.is_empty() && seen.insert(k.clone())).collect();
    wf.trigger.paths = wf.trigger.paths.iter().map(|p| p.trim().to_string()).filter(|p| !p.is_empty()).collect();
    let clean = |s: &mut Option<String>| {
        if let Some(v) = s.as_mut() {
            *v = v.trim().to_string();
        }
        if s.as_deref() == Some("") {
            *s = None;
        }
    };
    for n in &mut wf.nodes {
        n.id = n.id.trim().to_string();
        clean(&mut n.agent);
        clean(&mut n.command);
        clean(&mut n.verdict);
        clean(&mut n.event);
        if n.prompt.as_deref().is_some_and(|p| p.trim().is_empty()) {
            n.prompt = None;
        }
    }
    wf.source = ConfigSource::Machine;
    wf
}

/// Validates, then creates or replaces the workflow with this id in the project's (or the machine's) list.
pub async fn save(hub: &Hub, project: Option<&str>, wf: Workflow) -> Result<Workflow> {
    let wf = normalize(wf);
    let known = agent_names(hub, project).await;
    let problems = check(&wf, &|name| known.contains(name));
    if !problems.is_empty() {
        bail!("workflow `{}` is not valid: {}", wf.id, problems.join("; "));
    }
    match project {
        Some(slug) => {
            let mut p = hub.store.project(slug).await?.with_context(|| format!("project `{slug}` does not exist"))?;
            match p.settings.workflows.iter_mut().find(|w| w.id == wf.id) {
                Some(slot) => *slot = wf.clone(),
                None => p.settings.workflows.push(wf.clone()),
            }
            hub.put_project(&p).await?;
        }
        None => {
            let mut all = machine(hub).await?;
            match all.iter_mut().find(|w| w.id == wf.id) {
                Some(slot) => *slot = wf.clone(),
                None => all.push(wf.clone()),
            }
            set_machine(hub, &all).await?;
        }
    }
    find(hub, project, &wf.id).await?.context("workflow vanished after saving")
}

/// Removes the project's (or the machine's) workflow `id`; false when there was none. Removing a project override
/// brings the machine workflow back.
pub async fn delete(hub: &Hub, project: Option<&str>, id: &str) -> Result<bool> {
    match project {
        Some(slug) => {
            let mut p = hub.store.project(slug).await?.with_context(|| format!("project `{slug}` does not exist"))?;
            let before = p.settings.workflows.len();
            p.settings.workflows.retain(|w| w.id != id);
            if p.settings.workflows.len() == before {
                return Ok(false);
            }
            hub.put_project(&p).await?;
        }
        None => {
            let mut all = machine(hub).await?;
            let before = all.len();
            all.retain(|w| w.id != id);
            if all.len() == before {
                return Ok(false);
            }
            set_machine(hub, &all).await?;
        }
    }
    Ok(true)
}

// ---- agents a node may name

/// The agents in scope (the project's and the user's `AgentDef`s) and every configured provider.
pub async fn agent_names(hub: &Hub, project: Option<&str>) -> HashSet<String> {
    let mut names: HashSet<String> = hub.config.providers.keys().cloned().collect();
    let (cfg, workspace) = (hub.config.agents.clone(), hub.work_dir(project).await);
    if let Ok(overview) = tokio::task::spawn_blocking(move || crate::agents::overview(&cfg, &workspace)).await {
        names.extend(overview.rows.into_iter().map(|r| r.name));
    }
    names
}

/// The agent `name` as defined for `project`: the project's own, else the user's.
pub async fn agent_def(hub: &Hub, project: Option<&str>, name: &str) -> Option<AgentDef> {
    let (cfg, workspace, name) = (hub.config.agents.clone(), hub.work_dir(project).await, name.to_string());
    tokio::task::spawn_blocking(move || {
        [AgentScope::Project, AgentScope::User].into_iter().find_map(|scope| crate::agents::detail(&cfg, &workspace, scope, &name)).map(|d| d.def)
    })
    .await
    .ok()
    .flatten()
}

// ---- the graph

pub fn node<'a>(wf: &'a Workflow, id: &str) -> Option<&'a WorkflowNode> {
    wf.nodes.iter().find(|n| n.id == id)
}

pub fn trigger(wf: &Workflow) -> Option<&WorkflowNode> {
    wf.nodes.iter().find(|n| n.kind == NK::Trigger)
}

/// Where `from` goes after success (its `next` or `pass` edge).
pub fn success<'a>(wf: &'a Workflow, from: &str) -> Option<&'a str> {
    wf.edges.iter().find(|e| e.from == from && e.kind != EK::Fail).map(|e| e.to.as_str())
}

/// Where `from` goes after a failure (its `fail` edge).
pub fn fail<'a>(wf: &'a Workflow, from: &str) -> Option<&'a str> {
    wf.edges.iter().find(|e| e.from == from && e.kind == EK::Fail).map(|e| e.to.as_str())
}

/// The first end node whose outcome is failed.
pub fn failed_end(wf: &Workflow) -> Option<&str> {
    wf.nodes.iter().find(|n| n.kind == NK::End && n.outcome == Some(WorkflowOutcome::Failed)).map(|n| n.id.as_str())
}

/// The verify commands of the workflow, in node order: what the worker is told will run.
pub fn verify_commands(wf: &Workflow) -> Vec<String> {
    wf.nodes.iter().filter(|n| n.kind == NK::Verify).filter_map(|n| n.command.clone()).collect()
}

/// Everything wrong with `wf`; empty when it can be saved. `known` says whether an agent or provider name exists.
pub fn check(wf: &Workflow, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    let mut p = vec![];
    if !valid_id(&wf.id) {
        p.push(format!("id `{}` must be 1–40 letters, digits, `-` or `_`", wf.id));
    }
    let mut ids = HashSet::new();
    for n in &wf.nodes {
        if !valid_id(&n.id) {
            p.push(format!("node id `{}` must be 1–40 letters, digits, `-` or `_`", n.id));
        } else if !ids.insert(n.id.as_str()) {
            p.push(format!("node id `{}` is used twice", n.id));
        }
    }
    let triggers: Vec<&WorkflowNode> = wf.nodes.iter().filter(|n| n.kind == NK::Trigger).collect();
    if triggers.len() != 1 {
        p.push(format!("a workflow needs exactly one trigger, this one has {}", triggers.len()));
    }
    if !wf.nodes.iter().any(|n| n.kind == NK::End) {
        p.push("a workflow needs at least one end".into());
    }
    for n in &wf.nodes {
        let what = format!("{} `{}`", n.kind.as_str(), n.id);
        match n.kind {
            NK::Agent | NK::Audit => match n.agent.as_deref() {
                None => p.push(format!("{what} needs an agent")),
                Some(a) if !known(a) => p.push(format!("{what}: no agent or provider is called `{a}` here")),
                Some(_) => {}
            },
            NK::Verify => {
                if n.command.is_none() {
                    p.push(format!("{what} needs a command"));
                }
                match n.max_attempts {
                    Some(1..=10) => {}
                    Some(m) => p.push(format!("{what}: max attempts {m} must be 1 to 10")),
                    None => p.push(format!("{what} needs max attempts (1 to 10)")),
                }
            }
            NK::Hook => {
                if n.command.is_none() {
                    p.push(format!("{what} needs a command"));
                }
                if n.timeout_secs.is_some_and(|t| !(1..=3600).contains(&t)) {
                    p.push(format!("{what}: timeout must be 1 to 3600 seconds"));
                }
            }
            NK::Trigger | NK::End => {}
        }
        if n.kind == NK::Audit && n.max_attempts.is_some_and(|m| !(1..=10).contains(&m)) {
            p.push(format!("{what}: max attempts must be 1 to 10"));
        }
    }
    let kind_of = |id: &str| wf.nodes.iter().find(|n| n.id == id).map(|n| n.kind);
    let mut outs: BTreeMap<(&str, bool), usize> = BTreeMap::new();
    for e in &wf.edges {
        let (Some(from), Some(_)) = (kind_of(&e.from), kind_of(&e.to)) else {
            p.push(format!("edge {} → {} names a node that does not exist", e.from, e.to));
            continue;
        };
        if e.from == e.to {
            p.push(format!("edge {} → {} goes back to the same node", e.from, e.to));
        }
        if from == NK::End {
            p.push(format!("end `{}` cannot have an edge out", e.from));
        }
        if from == NK::Trigger && e.kind == EK::Fail {
            p.push(format!("trigger `{}` cannot have a fail edge", e.from));
        }
        if kind_of(&e.to) == Some(NK::Trigger) {
            p.push(format!("edge {} → {}: nothing goes back into the trigger", e.from, e.to));
        }
        *outs.entry((e.from.as_str(), e.kind == EK::Fail)).or_default() += 1;
    }
    for ((from, is_fail), n) in &outs {
        if *n > 1 {
            p.push(format!("`{from}` has {n} {} edges; one is the most", if *is_fail { "fail" } else { "next/pass" }));
        }
    }
    for n in wf.nodes.iter().filter(|n| n.kind != NK::End) {
        if !outs.contains_key(&(n.id.as_str(), false)) {
            p.push(format!("{} `{}` has no next/pass edge: where does the task go after it?", n.kind.as_str(), n.id));
        }
    }
    if let [t] = triggers.as_slice() {
        let mut seen: HashSet<&str> = HashSet::from([t.id.as_str()]);
        let mut queue = VecDeque::from([t.id.as_str()]);
        while let Some(at) = queue.pop_front() {
            for e in wf.edges.iter().filter(|e| e.from == at) {
                if seen.insert(e.to.as_str()) {
                    queue.push_back(e.to.as_str());
                }
            }
        }
        // A failed end needs no edge in: a verify or audit out of attempts goes there.
        let implicit = |n: &WorkflowNode| n.kind == NK::End && n.outcome == Some(WorkflowOutcome::Failed);
        for n in wf.nodes.iter().filter(|n| !seen.contains(n.id.as_str()) && !implicit(n)) {
            p.push(format!("{} `{}` cannot be reached from the trigger", n.kind.as_str(), n.id));
        }
    }
    p
}

// ---- which workflow a task runs

/// Does `keyword` occur in `text` (both lowercase)? ASCII words as whole words, anything else (Thai has no spaces)
/// as a substring.
fn mentions(text: &str, keyword: &str) -> bool {
    if !keyword.is_ascii() {
        return text.contains(keyword);
    }
    text.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-')).any(|w| w == keyword)
}

/// The workflow a task without one takes: the first whose keyword is in its title or prompt (the project's own
/// before the machine's), else the first `default` one; with why. `None`: the fixed pipeline.
pub fn choose(list: &[Workflow], task: &Task) -> Option<(Workflow, String)> {
    let text = format!("{}\n{}", task.title, task.prompt).to_lowercase();
    let ordered: Vec<&Workflow> = list.iter().filter(|w| w.source != ConfigSource::Machine).chain(list.iter().filter(|w| w.source == ConfigSource::Machine)).collect();
    for w in &ordered {
        if let Some(k) = w.trigger.keywords.iter().find(|k| mentions(&text, &k.to_lowercase())) {
            return Some(((*w).clone(), format!("keyword {k}")));
        }
    }
    ordered.into_iter().find(|w| w.trigger.default).map(|w| (w.clone(), "default".into()))
}

// ---- run stats from the task events

/// Outcomes that count as a failed visit.
fn failed(outcome: &str) -> bool {
    matches!(outcome, "fail" | "exhausted" | "retry" | "failed" | "error")
}

/// `decision.workflow.verify.pass` + "ver · …" → ("verify", "pass", "ver").
pub fn parse_event<'a>(kind: &'a str, message: &'a str) -> Option<(&'a str, &'a str, &'a str)> {
    let rest = kind.strip_prefix(EVENT_PREFIX)?;
    let (node_kind, outcome) = rest.split_once('.')?;
    let node = message.split(" · ").next().unwrap_or("").trim();
    Some((node_kind, outcome, node))
}

fn short(text: &str, max: usize) -> String {
    let t: String = text.chars().take(max).collect();
    if text.chars().count() > max { format!("{t}…") } else { t }
}

/// Per node over the last [`RUNS`] tasks that ran workflow `id` (of `project`; `None`: of every project).
pub async fn stats(hub: &Hub, project: Option<&str>, id: &str) -> Result<WorkflowStats> {
    let mut tasks: Vec<Task> =
        hub.store.tasks().await?.into_iter().filter(|t| t.workflow.as_deref() == Some(id) && t.workflow_state.is_some() && (project.is_none() || t.project.as_deref() == project)).collect();
    tasks.sort_by_key(|t| std::cmp::Reverse(t.id));
    tasks.truncate(RUNS);
    let ids: Vec<i64> = tasks.iter().map(|t| t.id).collect();
    let events = hub.store.events_of_tasks(&ids).await?;

    #[derive(Default)]
    struct Acc {
        runs: HashSet<i64>,
        visits: u32,
        durations: Vec<i64>,
        finished: u32,
        fails: u32,
    }
    let mut acc: BTreeMap<String, Acc> = BTreeMap::new();
    let mut recent = vec![];
    for t in &tasks {
        let mine: Vec<_> = events.iter().filter(|e| e.task_id == Some(t.id)).collect();
        let mut open: BTreeMap<&str, DateTime<Utc>> = BTreeMap::new();
        let mut first: Option<DateTime<Utc>> = None;
        let mut failures = vec![];
        for e in &mine {
            let Some((kind, outcome, node)) = parse_event(&e.kind, &e.message) else { continue };
            if kind == "chosen" || kind == "error" {
                first.get_or_insert(e.at);
                continue;
            }
            first.get_or_insert(e.at);
            let a = acc.entry(node.to_string()).or_default();
            a.runs.insert(t.id);
            if outcome == "start" {
                a.visits += 1;
                open.insert(node, e.at);
                continue;
            }
            if matches!(kind, "trigger" | "end") {
                a.visits += 1;
            }
            if let Some(at) = open.remove(node) {
                a.durations.push((e.at - at).num_milliseconds());
            }
            if !matches!(kind, "trigger" | "end") {
                a.finished += 1;
                if failed(outcome) {
                    a.fails += 1;
                    failures.push(short(&e.message, 300));
                }
            }
        }
        let last = mine.last().map(|e| e.at);
        let duration_ms = first.zip(last).map(|(a, b)| (b - a).num_milliseconds());
        recent.push(WorkflowRun { task_id: t.id, title: t.title.clone(), status: t.status, duration_ms, failures, at: last.unwrap_or(t.updated_at) });
    }
    // The workflow's own node order first, then nodes only the events still name.
    let order: Vec<String> = find(hub, project, id).await.ok().flatten().map(|w| w.nodes.into_iter().map(|n| n.id).collect()).unwrap_or_default();
    let mut names: Vec<String> = order.clone();
    names.extend(acc.keys().filter(|k| !order.contains(k)).cloned());
    let nodes = names
        .into_iter()
        .map(|node| {
            let a = acc.remove(&node).unwrap_or_default();
            WorkflowNodeStats {
                node,
                runs: a.runs.len() as u32,
                visits: a.visits,
                avg_ms: (!a.durations.is_empty()).then(|| a.durations.iter().sum::<i64>() / a.durations.len() as i64),
                fail_pct: (a.finished > 0).then(|| (a.fails as f64 * 1000.0 / a.finished as f64).round() / 10.0),
            }
        })
        .collect();
    Ok(WorkflowStats { workflow: id.to_string(), project: project.map(String::from), runs: tasks.len() as u32, nodes, recent })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{WorkflowEdge, WorkflowTrigger};

    pub(crate) fn n(id: &str, kind: NK) -> WorkflowNode {
        WorkflowNode { id: id.into(), kind, label: id.into(), ..Default::default() }
    }

    pub(crate) fn e(from: &str, to: &str, kind: EK) -> WorkflowEdge {
        WorkflowEdge { from: from.into(), to: to.into(), kind }
    }

    /// trigger → code → ver (pass → done, fail → code, ≤ 2).
    pub(crate) fn bugfix() -> Workflow {
        Workflow {
            id: "bugfix".into(),
            name: "bugfix".into(),
            trigger: WorkflowTrigger { keywords: vec!["bug".into(), "fix".into()], ..Default::default() },
            nodes: vec![
                n("t", NK::Trigger),
                WorkflowNode { agent: Some("claude".into()), ..n("code", NK::Agent) },
                WorkflowNode { command: Some("true".into()), max_attempts: Some(2), ..n("ver", NK::Verify) },
                WorkflowNode { outcome: Some(WorkflowOutcome::Done), ..n("done", NK::End) },
                WorkflowNode { outcome: Some(WorkflowOutcome::Failed), ..n("failed", NK::End) },
            ],
            edges: vec![e("t", "code", EK::Next), e("code", "ver", EK::Next), e("ver", "done", EK::Pass), e("ver", "code", EK::Fail)],
            ..Default::default()
        }
    }

    fn known(name: &str) -> bool {
        matches!(name, "claude" | "codex" | "reviewer")
    }

    #[test]
    fn a_good_workflow_has_no_problems() {
        assert_eq!(check(&bugfix(), &known), Vec::<String>::new());
    }

    #[test]
    fn the_checks_name_each_problem() {
        let mut w = bugfix();
        w.nodes.push(n("t2", NK::Trigger));
        w.nodes.push(WorkflowNode { agent: Some("ghost".into()), ..n("lost", NK::Agent) });
        w.nodes[2].max_attempts = Some(11);
        w.nodes[2].command = None;
        w.edges.push(e("done", "code", EK::Next));
        w.edges.push(e("code", "nowhere", EK::Next));
        w.edges.push(e("code", "failed", EK::Pass));
        w.edges.push(e("ver", "failed", EK::Fail));
        let p = check(&w, &known).join("\n");
        for want in [
            "exactly one trigger, this one has 2",
            "no agent or provider is called `ghost`",
            "max attempts 11 must be 1 to 10",
            "verify `ver` needs a command",
            "end `done` cannot have an edge out",
            "names a node that does not exist",
            "`ver` has 2 fail edges",
            "`code` has 2 next/pass edges",
        ] {
            assert!(p.contains(want), "missing {want:?} in\n{p}");
        }
        // With two triggers, reachability is not checked; with one, an island is.
        let mut w = bugfix();
        w.nodes.push(WorkflowNode { command: Some("x".into()), ..n("island", NK::Hook) });
        w.edges.push(e("island", "done", EK::Next));
        assert!(check(&w, &known).iter().any(|p| p.contains("hook `island` cannot be reached")), "{:?}", check(&w, &known));
        let mut w = bugfix();
        w.nodes.retain(|n| n.kind != NK::End);
        w.edges.retain(|e| e.to != "done");
        let p = check(&w, &known).join("\n");
        assert!(p.contains("at least one end") && p.contains("verify `ver` has no next/pass edge"), "{p}");
        let mut w = bugfix();
        w.edges.push(e("t", "failed", EK::Fail));
        w.edges.push(e("code", "t", EK::Fail));
        let p = check(&w, &known).join("\n");
        assert!(p.contains("trigger `t` cannot have a fail edge") && p.contains("nothing goes back into the trigger"), "{p}");
    }

    #[test]
    fn pass_out_of_an_agent_reads_as_next() {
        let mut w = bugfix();
        w.edges[1].kind = EK::Pass;
        assert!(check(&w, &known).is_empty());
        assert_eq!(success(&w, "code"), Some("ver"));
        assert_eq!(fail(&w, "ver"), Some("code"));
        assert_eq!(failed_end(&w), Some("failed"));
    }

    fn task(title: &str, prompt: &str) -> Task {
        Task {
            id: 1,
            title: title.into(),
            prompt: prompt.into(),
            depends_on: vec![],
            provider: None,
            model: None,
            project: None,
            status: crate::model::TaskStatus::Pending,
            session: None,
            attempts: 0,
            result: None,
            git: None,
            scope: None,
            intake: None,
            created_by: String::new(),
            workflow: None,
            workflow_state: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn keywords_match_whole_words_and_the_project_comes_first() {
        let mut feature = bugfix();
        feature.id = "feature".into();
        feature.trigger = WorkflowTrigger { default: true, ..Default::default() };
        let machine = vec![bugfix(), feature.clone()];
        let list = merge(machine, vec![]);
        assert_eq!(choose(&list, &task("Fix the login", "")).map(|(w, why)| (w.id, why)), Some(("bugfix".into(), "keyword fix".into())));
        assert_eq!(choose(&list, &task("Prefix routes", "debugger")).map(|(w, why)| (w.id, why)), Some(("feature".into(), "default".into())));
        // A project workflow with the same keyword wins over the machine's.
        let mut mine = bugfix();
        mine.id = "mine".into();
        let list = merge(vec![bugfix()], vec![mine]);
        assert_eq!(choose(&list, &task("a bug", "")).unwrap().0.id, "mine");
        assert!(choose(&merge(vec![bugfix()], vec![]), &task("Add a page", "")).is_none());
        // Thai keywords match as substrings.
        let mut thai = bugfix();
        thai.trigger.keywords = vec!["แก้บั๊ก".into()];
        assert!(choose(&[thai], &task("ช่วยแก้บั๊กหน้าล็อกอิน", "")).is_some());
    }

    #[test]
    fn a_project_workflow_with_a_machine_id_overrides_it() {
        let mut over = bugfix();
        over.name = "ours".into();
        let mut own = bugfix();
        own.id = "own".into();
        let list = merge(vec![bugfix()], vec![over, own]);
        let got: Vec<(&str, ConfigSource, &str)> = list.iter().map(|w| (w.id.as_str(), w.source, w.name.as_str())).collect();
        assert_eq!(got, vec![("bugfix", ConfigSource::Override, "ours"), ("own", ConfigSource::Project, "bugfix")]);
    }
}
