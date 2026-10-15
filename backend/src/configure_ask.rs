//! Configure's "Ask an agent to do it" (`POST /api/configure/ask`): one headless run of the chosen provider that
//! reads a tab's config and the owner's request and proposes changes as [`ConfigOp`]s. MAP checks each op against the
//! tab's schema, drops the invalid ones (saying so in `evidence`) and returns the rest. It never writes config: the
//! dashboard applies the ops the owner checks and saves through the tab's own API.
//!
//! The CLI runs in a new empty temporary folder (never the project folder, so it has nothing to edit), with the
//! provider's own env as the audit has, and is killed after [`TIMEOUT`].

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use tokio::process::Command;

use crate::hub::Hub;
use crate::model::{
    AgentDef, AgentModels, AgentProvider, Capability, ConfigAskRequest, ConfigAskResult, ConfigLanguage, ConfigOp, ConfigOpKind, ConfigTab, Effort, Level,
    WorkflowEdgeKind, WorkflowNode, WorkflowNodeKind,
};

pub const TIMEOUT: Duration = Duration::from_secs(180);
/// The tab's config JSON sent to the agent is cut to this many characters.
const CONTEXT_CAP: usize = 24_000;
const PROMPT_CAP: usize = 4_000;
const RUNS_CAP: usize = 12_000;
const HOOK_EVENTS: &[&str] = &["pre_launch", "post_verify", "post_done", "on_fail", "on_stall"];
const NODE_KEYS: &[&str] = &["label", "agent", "prompt", "command", "max_attempts", "verdict", "event", "blocking", "timeout_secs", "outcome"];

fn tab_name(tab: ConfigTab) -> &'static str {
    match tab {
        ConfigTab::Agents => "agents",
        ConfigTab::Rules => "rules",
        ConfigTab::Skills => "skills",
        ConfigTab::Perms => "perms",
        ConfigTab::Hooks => "hooks",
        ConfigTab::Verify => "verify",
        ConfigTab::Workflows => "workflows",
    }
}

fn effort_name(e: Effort) -> &'static str {
    match e {
        Effort::Low => "low",
        Effort::Medium => "medium",
        Effort::High => "high",
        Effort::Xhigh => "xhigh",
        Effort::Max => "max",
    }
}

/// What the tab holds and the shape of its ops, for the prompt.
fn schema(tab: ConfigTab) -> &'static str {
    match tab {
        ConfigTab::Agents => {
            "Tab \"Agents\": custom sub-agents, each written as a file for Claude Code, Codex and agy.\n\
             Ops (target `agent:<name>`, name = lowercase letters, digits, `-`, `_`):\n\
             - add: value = the whole agent {\"name\", \"description\" (when to use it), \"instructions\" (its system prompt), \
             \"tools\": null (every tool) or a subset of [\"read\",\"search\",\"edit\",\"shell\",\"web\"], \
             \"models\": {\"claude\": null|\"opus\"|\"sonnet\"|\"haiku\"|\"fable\"|\"inherit\", \"codex\": null|<model>, \"gemini\": null|\"pro\"|\"flash\"|\"inherit\"}, \
             \"effort\": null|\"low\"|\"medium\"|\"high\"|\"xhigh\"|\"max\", \"targets\": a non-empty subset of [\"claude\",\"codex\",\"gemini\"]}\n\
             - change: value = only the fields that change, e.g. {\"instructions\": \"…\"} or {\"tools\": [\"read\",\"search\"]}\n\
             - remove: value = null"
        }
        ConfigTab::Rules => {
            "Tab \"Rules\": the context files every session loads (AGENTS.md, CLAUDE.md, GEMINI.md, ~/.claude/CLAUDE.md, ~/.codex/AGENTS.md).\n\
             Ops (target `rule:<file>`, e.g. `rule:AGENTS.md`):\n\
             - add: value = {\"append\": \"<markdown to add at the end>\"}\n\
             - change: value = {\"find\": \"<exact text now in the file>\", \"replace\": \"<new text>\"}\n\
             - remove: value = {\"find\": \"<exact text to delete>\"}\n\
             Keep rules short and imperative: every session pays for every line."
        }
        ConfigTab::Skills => {
            "Tab \"Skills\": skills (SKILL.md folders) and whether each is on for this workspace.\n\
             Ops (target `skill:<name>`):\n\
             - change: value = {\"enabled\": true|false}\n\
             - add: value = {\"name\", \"description\", \"body\": \"<the SKILL.md body, markdown>\"}\n\
             - remove: value = null"
        }
        ConfigTab::Perms => {
            "Tab \"Permissions\": tool rules per agent or provider, each allow, ask or deny.\n\
             Ops (target `perm:<rule>:<agent or provider>`, rule like `Bash(npm test:*)`, `Edit`, `WebFetch(domain:docs.rs)`):\n\
             - add or change: value = {\"mode\": \"allow\"|\"ask\"|\"deny\"}\n\
             - remove: value = null\n\
             Prefer the narrowest rule that does the job; never allow destructive commands broadly."
        }
        ConfigTab::Hooks => {
            "Tab \"Hooks\": commands MAP runs around a task, by event: pre_launch, post_verify, post_done, on_fail, on_stall.\n\
             Ops (target `hook:<event>`):\n\
             - add: value = {\"command\": \"<sh command>\", \"blocking\": true|false, \"timeout_secs\": 1-3600, \"enabled\": true|false}\n\
             - change: value = {\"command\": \"<the hook's current command>\", \"set\": {any of \"command\", \"blocking\", \"timeout_secs\", \"enabled\"}}\n\
             - remove: value = {\"command\": \"<the hook's current command>\"}"
        }
        ConfigTab::Verify => {
            "Tab \"Verify & audit\": the commands that must exit 0 after an agent reports done, and the audit by another agent.\n\
             Ops:\n\
             - target `verify.cmds`: add {\"command\", \"index\"?: position from 0}, remove {\"command\"}, change {\"from\": \"<current command>\", \"to\": \"<new command>\"}\n\
             - target `verify.max_attempts`: change {\"value\": 1-10}\n\
             - target `audit.enabled`: change {\"value\": true|false}\n\
             - target `audit.auditor`: change {\"value\": \"<agent or provider name>\"}\n\
             - target `audit.verdict`: change {\"value\": \"<the text that means pass, e.g. VERDICT: PASS>\"}\n\
             Put fast checks (lint, types) before slow ones (the full test suite)."
        }
        ConfigTab::Workflows => {
            "Tab \"Workflows\": a task's steps as a graph the orchestrator walks. Node kinds: trigger (exactly one, where the task \
             enters), agent (a worker session: \"agent\" = an agent or provider name, \"prompt\" = extra instructions for that step), \
             verify (\"command\" must exit 0, \"max_attempts\" 1-10), audit (\"agent\" = the auditor, \"verdict\" = the pass text), \
             hook (\"command\", \"event\": pre_launch|post_verify|post_done|on_fail, \"blocking\": true|false, \"timeout_secs\"), \
             end (\"outcome\": \"done\"|\"failed\"). Edges {\"from\", \"to\", \"kind\": \"next\"|\"pass\"|\"fail\"}: next or pass after \
             success, fail after a failure (a verify's or audit's fail edge usually goes back to an agent, which gets the failure output).\n\
             Each op has target `workflow.node:<node id>` and its value is one editor op:\n\
             - {\"op\": \"add\", \"after\": \"<node id>\", \"before\": \"<node id>\", \"node\": {\"id\", \"kind\", \"label\", …fields of its kind}, \
             \"fail_to\"?: \"<node id>\"}: a new step on the edge after → before (it must exist); op \"add\"\n\
             - {\"op\": \"insert_before\", \"target\": \"<node id>\", \"node\": {…}}: a new step (often a hook) on every edge into target; op \"add\"\n\
             - {\"op\": \"remove\", \"id\": \"<node id>\", \"bridge\"?: [\"<from id>\", \"<to id>\", \"next\"|\"pass\"|\"fail\"]}: remove a step \
             and link around it; op \"remove\"\n\
             - {\"op\": \"change\", \"id\": \"<node id>\", \"key\": \"label\"|\"agent\"|\"prompt\"|\"command\"|\"max_attempts\"|\"verdict\"|\"event\"|\"blocking\"|\"timeout_secs\"|\"outcome\", \
             \"to\": <new value>}; op \"change\"\n\
             New node ids are short, lowercase and unused. Never add or remove the trigger."
        }
    }
}

/// The last JSON object in `output` with an `ops` list.
fn parse_answer(output: &str) -> Option<Value> {
    output.match_indices('{').rev().find_map(|(i, _)| {
        let v: Value = serde_json::Deserializer::from_str(&output[i..]).into_iter::<Value>().next()?.ok()?;
        v.get("ops")?.as_array()?;
        Some(v)
    })
}

fn cap(text: &str, max: usize) -> (String, bool) {
    if text.chars().count() <= max {
        return (text.to_string(), false);
    }
    (text.chars().take(max).collect(), true)
}

/// The prompt the CLI gets.
fn prompt(req: &ConfigAskRequest, context: &Value, runs: Option<&Value>) -> String {
    let tab = tab_name(req.tab);
    let scope = match &req.project {
        Some(slug) => format!("project `{slug}` (a project value overrides the machine one with the same name)"),
        None => "the machine (every project)".to_string(),
    };
    let mut p = format!(
        "You are helping the owner configure MAP, a platform that runs coding-agent CLIs (Claude Code, Codex, agy) on tasks, on its \
         Configure screen, tab `{tab}`, scope: {scope}.\n\
         You are in an empty scratch folder. Do not create, edit or delete any file and do not run anything that changes the machine: \
         only read what is below and answer. MAP shows your proposals to the owner, who picks which to apply.\n\n{}\n\n",
        schema(req.tab)
    );
    let text = serde_json::to_string_pretty(context).unwrap_or_default();
    let (text, cut) = cap(&text, CONTEXT_CAP);
    p.push_str(&format!("The tab's current config (JSON{}):\n```json\n{text}\n```\n\n", if cut { ", cut short" } else { "" }));
    if let Some(runs) = runs {
        let text = serde_json::to_string_pretty(runs).unwrap_or_default();
        let (text, cut) = cap(&text, RUNS_CAP);
        p.push_str(&format!(
            "How this workflow did over its last runs (JSON{}): per node `runs` (tasks that reached it), `visits`, `avg_ms`, `fail_pct`; \
             `recent` lists each task with its status, duration and what failed:\n```json\n{text}\n```\n\n",
            if cut { ", cut short" } else { "" }
        ));
    }
    let request = req.prompt.trim();
    if request.is_empty() {
        p.push_str("The owner's request: suggest improvements to this workflow from its last runs: slow, failing or wasted steps, missing checks.\n\n");
    } else {
        let (request, _) = cap(request, PROMPT_CAP);
        p.push_str(&format!("The owner's request:\n\"\"\"\n{request}\n\"\"\"\n\n"));
    }
    if let Some(e) = req.effort {
        let how = match e {
            Effort::Low => "answer quickly; a short look is enough",
            Effort::Medium => "think it through at a normal depth",
            Effort::High | Effort::Xhigh | Effort::Max => "think carefully and check each proposal against the config before answering",
        };
        p.push_str(&format!("Reasoning effort: {} ({how}).\n", effort_name(e)));
    }
    match req.language {
        ConfigLanguage::En => p.push_str(
            "Output language: English. Write `summary`, `evidence`, every `text` and all prose inside `value` (instructions, rules, \
             descriptions, labels, prompts) in English, even when the request above is written in Thai or another language. \
             Keep names, file names, commands and JSON keys as they are.\n",
        ),
        ConfigLanguage::Th => p.push_str(
            "Output language: Thai (ภาษาไทย). Write `summary`, `evidence`, every `text` and all prose inside `value` (instructions, rules, \
             descriptions, labels, prompts) in Thai. Keep names, ids, file names, commands, tool names and JSON keys in English.\n",
        ),
    }
    p.push_str(
        "\nPropose only what the request or the evidence supports; a few precise changes beat many. Each op is \
         {\"op\": \"add\"|\"change\"|\"remove\", \"target\": \"…\", \"text\": \"<one line the owner reads>\", \"value\": …} as above. \
         Proposals that do not fit the schema are dropped.\n\
         End your reply with exactly one JSON object and nothing after it:\n\
         {\"summary\": \"<one or two sentences>\", \"evidence\": [\"<a fact you relied on>\", …], \"ops\": [ … ]}\n",
    );
    p
}

/// The workflow the ops are about, from the context the dashboard sent (`{nodes}` or `{workflow: {nodes}}`).
fn context_nodes(context: &Value) -> Option<HashSet<String>> {
    let nodes = context.get("nodes").or_else(|| context.get("workflow").and_then(|w| w.get("nodes")))?.as_array()?;
    Some(nodes.iter().filter_map(|n| n.get("id").and_then(Value::as_str)).map(String::from).collect())
}

fn obj(value: &Value) -> Result<&Map<String, Value>> {
    value.as_object().context("value must be an object")
}

fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).with_context(|| format!("value needs a non-empty `{key}`"))
}

fn named<'a>(target: &'a str, prefix: &str) -> Result<&'a str> {
    let name = target.strip_prefix(prefix).map(str::trim).filter(|s| !s.is_empty()).with_context(|| format!("target must be `{prefix}<name>`"))?;
    Ok(name)
}

/// One agent field as the change gives it: it must parse as that field's type.
fn agent_field(key: &str, v: &Value) -> Result<()> {
    let ok = match key {
        "name" | "description" | "instructions" => v.is_string(),
        "tools" => serde_json::from_value::<Option<Vec<Capability>>>(v.clone()).is_ok(),
        "models" => serde_json::from_value::<AgentModels>(v.clone()).is_ok(),
        "effort" => serde_json::from_value::<Option<Effort>>(v.clone()).is_ok(),
        "targets" => serde_json::from_value::<Vec<AgentProvider>>(v.clone()).is_ok_and(|t| !t.is_empty()),
        _ => bail!("agents have no field `{key}`"),
    };
    if !ok {
        bail!("`{key}` has the wrong type");
    }
    Ok(())
}

/// A workflow node from an op: our field names, plus the editor's (`type`, `cmd`, `maxAttempts`).
fn workflow_node(v: &Value) -> Result<WorkflowNode> {
    let mut v = v.clone();
    let m = v.as_object_mut().context("`node` must be an object")?;
    for (from, to) in [("type", "kind"), ("cmd", "command"), ("maxAttempts", "max_attempts"), ("timeoutSecs", "timeout_secs")] {
        if let Some(x) = m.remove(from) {
            m.entry(to).or_insert(x);
        }
    }
    if m.get("blocking").and_then(Value::as_str).is_some() {
        let yes = m.get("blocking").and_then(Value::as_str) == Some("yes");
        m.insert("blocking".into(), Value::Bool(yes));
    }
    let node: WorkflowNode = serde_json::from_value(v).context("`node` does not fit a workflow node")?;
    if !crate::workflows::valid_id(&node.id) {
        bail!("node id `{}` must be 1–40 letters, digits, `-` or `_`", node.id);
    }
    match node.kind {
        WorkflowNodeKind::Trigger => bail!("a workflow has one trigger; it cannot be added"),
        WorkflowNodeKind::Agent | WorkflowNodeKind::Audit if node.agent.as_deref().is_none_or(|a| a.trim().is_empty()) => bail!("a {} step needs an agent", node.kind.as_str()),
        WorkflowNodeKind::Verify if node.command.as_deref().is_none_or(|c| c.trim().is_empty()) || !node.max_attempts.is_some_and(|m| (1..=10).contains(&m)) => {
            bail!("a verify step needs a command and max_attempts 1–10")
        }
        WorkflowNodeKind::Hook if node.command.as_deref().is_none_or(|c| c.trim().is_empty()) => bail!("a hook needs a command"),
        _ => {}
    }
    Ok(node)
}

/// Checks a workflow op and returns it in our names (`insert_before`, `fail_to`, `kind`, `command`, …).
fn workflow_op(value: &Value, nodes: Option<&HashSet<String>>) -> Result<(ConfigOpKind, Value)> {
    let m = obj(value)?;
    let op = m.get("op").and_then(Value::as_str).context("value needs `op`: add, insert_before, remove or change")?;
    let exists = |id: &str| -> Result<()> {
        match nodes {
            Some(n) if !n.contains(id) => bail!("there is no node `{id}`"),
            _ => Ok(()),
        }
    };
    let fresh = |id: &str| -> Result<()> {
        if nodes.is_some_and(|n| n.contains(id)) {
            bail!("node id `{id}` is already used");
        }
        Ok(())
    };
    let node_json = |n: &WorkflowNode| serde_json::to_value(n).unwrap_or(Value::Null);
    match op {
        "add" => {
            let (after, before) = (string(value, "after")?, string(value, "before")?);
            exists(after)?;
            exists(before)?;
            let node = workflow_node(m.get("node").context("value needs `node`")?)?;
            fresh(&node.id)?;
            let fail_to = m.get("fail_to").or_else(|| m.get("failTo")).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
            if let Some(f) = fail_to.filter(|f| *f != node.id) {
                exists(f)?;
            }
            Ok((ConfigOpKind::Add, json!({"op": "add", "after": after, "before": before, "node": node_json(&node), "fail_to": fail_to})))
        }
        "insert_before" | "insertBefore" => {
            let target = string(value, "target")?;
            exists(target)?;
            let node = workflow_node(m.get("node").context("value needs `node`")?)?;
            fresh(&node.id)?;
            Ok((ConfigOpKind::Add, json!({"op": "insert_before", "target": target, "node": node_json(&node)})))
        }
        "remove" => {
            let id = string(value, "id")?;
            exists(id)?;
            let bridge = match m.get("bridge") {
                None | Some(Value::Null) => Value::Null,
                Some(b) => {
                    let parts = b.as_array().filter(|a| a.len() == 3).context("`bridge` is [from, to, kind]")?;
                    let (from, to) = (parts[0].as_str().context("bridge from")?, parts[1].as_str().context("bridge to")?);
                    exists(from)?;
                    exists(to)?;
                    if from == id || to == id {
                        bail!("the bridge cannot go through the removed node");
                    }
                    let kind: WorkflowEdgeKind = serde_json::from_value(parts[2].clone()).context("bridge kind is next, pass or fail")?;
                    json!([from, to, kind])
                }
            };
            Ok((ConfigOpKind::Remove, json!({"op": "remove", "id": id, "bridge": bridge})))
        }
        "change" => {
            let id = string(value, "id")?;
            exists(id)?;
            let key = string(value, "key")?;
            let key = match key {
                "cmd" => "command",
                "maxAttempts" => "max_attempts",
                "timeoutSecs" => "timeout_secs",
                k => k,
            };
            if !NODE_KEYS.contains(&key) {
                bail!("a node has no field `{key}` to change");
            }
            let to = m.get("to").cloned().context("value needs `to`")?;
            // The new value must fit the field's type.
            let probe = json!({"id": id, key: to});
            serde_json::from_value::<WorkflowNode>(probe).with_context(|| format!("`{key}` cannot be {to}"))?;
            if key == "max_attempts" && !to.as_u64().is_some_and(|m| (1..=10).contains(&m)) {
                bail!("max_attempts must be 1 to 10");
            }
            Ok((ConfigOpKind::Change, json!({"op": "change", "id": id, "key": key, "to": to})))
        }
        other => bail!("unknown workflow op `{other}`"),
    }
}

/// Checks one op against the tab's schema; the op as returned (normalised), or why it is dropped.
fn check_op(tab: ConfigTab, raw: &Value, nodes: Option<&HashSet<String>>) -> Result<ConfigOp> {
    let m = raw.as_object().context("not an object")?;
    let target = m.get("target").and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
    if target.is_empty() || target.chars().count() > 300 {
        bail!("no target");
    }
    let mut op: ConfigOpKind = serde_json::from_value(m.get("op").cloned().unwrap_or(Value::Null)).context("op must be add, change or remove")?;
    let mut value = m.get("value").cloned().unwrap_or(Value::Null);
    match tab {
        ConfigTab::Agents => {
            let name = named(&target, "agent:")?;
            if !crate::agents::valid_name(name) {
                bail!("`{name}` is not a valid agent name");
            }
            match op {
                ConfigOpKind::Add => {
                    let def: AgentDef = serde_json::from_value(value.clone()).context("value is not a whole agent")?;
                    if def.name != name || def.description.trim().is_empty() || def.targets.is_empty() {
                        bail!("the agent needs the target's name, a description and at least one target");
                    }
                }
                ConfigOpKind::Change => {
                    let fields = obj(&value)?;
                    if fields.is_empty() {
                        bail!("nothing to change");
                    }
                    for (k, v) in fields {
                        agent_field(k, v)?;
                    }
                }
                ConfigOpKind::Remove => value = Value::Null,
            }
        }
        ConfigTab::Rules => {
            let file = named(&target, "rule:")?;
            if file.split('/').any(|part| part == "..") {
                bail!("`{file}` leaves the folder");
            }
            match op {
                ConfigOpKind::Add => {
                    string(&value, "append")?;
                }
                ConfigOpKind::Change => {
                    if value.get("content").and_then(Value::as_str).is_none_or(|c| c.trim().is_empty()) {
                        string(&value, "find")?;
                        value.get("replace").and_then(Value::as_str).context("value needs `replace`")?;
                    }
                }
                ConfigOpKind::Remove => {
                    string(&value, "find")?;
                }
            }
        }
        ConfigTab::Skills => {
            let name = named(&target, "skill:")?;
            match op {
                ConfigOpKind::Change => {
                    value.get("enabled").and_then(Value::as_bool).context("value needs `enabled`: true or false")?;
                }
                ConfigOpKind::Add => {
                    if string(&value, "name")? != name {
                        bail!("the skill's name must be the target's");
                    }
                    string(&value, "description")?;
                    string(&value, "body")?;
                }
                ConfigOpKind::Remove => value = Value::Null,
            }
        }
        ConfigTab::Perms => {
            let rest = named(&target, "perm:")?;
            let (rule, agent) = rest.rsplit_once(':').context("target must be `perm:<rule>:<agent>`")?;
            if rule.trim().is_empty() || agent.trim().is_empty() {
                bail!("target must be `perm:<rule>:<agent>`");
            }
            match op {
                ConfigOpKind::Add | ConfigOpKind::Change => {
                    let mode = string(&value, "mode")?;
                    if !matches!(mode, "allow" | "ask" | "deny") {
                        bail!("mode must be allow, ask or deny");
                    }
                }
                ConfigOpKind::Remove => value = Value::Null,
            }
        }
        ConfigTab::Hooks => {
            let event = named(&target, "hook:")?;
            if !HOOK_EVENTS.contains(&event) {
                bail!("`{event}` is not a hook event ({})", HOOK_EVENTS.join(", "));
            }
            let fields = |v: &Value| -> Result<()> {
                if v.get("blocking").is_some_and(|b| !b.is_boolean()) || v.get("enabled").is_some_and(|b| !b.is_boolean()) {
                    bail!("blocking and enabled are true or false");
                }
                if v.get("timeout_secs").is_some_and(|t| !t.as_u64().is_some_and(|t| (1..=3600).contains(&t))) {
                    bail!("timeout_secs must be 1 to 3600");
                }
                Ok(())
            };
            string(&value, "command")?;
            match op {
                ConfigOpKind::Add => fields(&value)?,
                ConfigOpKind::Change => {
                    let set = value.get("set").filter(|s| s.as_object().is_some_and(|o| !o.is_empty())).context("value needs `set` with what changes")?;
                    fields(set)?;
                }
                ConfigOpKind::Remove => {}
            }
        }
        ConfigTab::Verify => match (target.as_str(), op) {
            ("verify.cmds", ConfigOpKind::Add) => {
                string(&value, "command")?;
            }
            ("verify.cmds", ConfigOpKind::Remove) => {
                string(&value, "command")?;
            }
            ("verify.cmds", ConfigOpKind::Change) => {
                string(&value, "from")?;
                string(&value, "to")?;
            }
            ("verify.max_attempts", ConfigOpKind::Change) => {
                value.get("value").and_then(Value::as_u64).filter(|m| (1..=10).contains(m)).context("value must be 1 to 10")?;
            }
            ("audit.enabled", ConfigOpKind::Change) => {
                value.get("value").and_then(Value::as_bool).context("value must be true or false")?;
            }
            ("audit.auditor" | "audit.verdict", ConfigOpKind::Change) => {
                string(&value, "value")?;
            }
            (t, o) => bail!("`{t}` cannot be {}", serde_json::to_value(o).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default()),
        },
        ConfigTab::Workflows => {
            if !target.starts_with("workflow.") {
                bail!("target must be `workflow.node:<id>`");
            }
            let (kind, normalised) = workflow_op(&value, nodes)?;
            op = kind;
            value = normalised;
        }
    }
    let text = m.get("text").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()).map(String::from).unwrap_or_else(|| target.clone());
    let (text, _) = cap(&text, 400);
    Ok(ConfigOp { op, target, text, value })
}

/// The ops that fit the tab, and a line per op dropped.
fn check_ops(tab: ConfigTab, raw: &[Value], context: &Value) -> (Vec<ConfigOp>, Vec<String>) {
    let nodes = if tab == ConfigTab::Workflows { context_nodes(context) } else { None };
    let (mut ops, mut dropped) = (vec![], vec![]);
    for (i, r) in raw.iter().enumerate().take(50) {
        match check_op(tab, r, nodes.as_ref()) {
            Ok(op) => ops.push(op),
            Err(err) => {
                let target = r.get("target").and_then(Value::as_str).unwrap_or("?");
                dropped.push(format!("MAP dropped proposal {} ({target}): {err:#}", i + 1));
            }
        }
    }
    (ops, dropped)
}

/// The headless command line: the template with `{effort}` arguments before the prompt's.
fn argv(spec: &crate::config::ProviderSpec, model: &str, effort: Option<Effort>, prompt: &str) -> Vec<String> {
    let mut out = vec![];
    let effort_args: Vec<String> = match effort {
        Some(e) if !spec.effort_args.is_empty() => {
            let value = if e == Effort::Max { "xhigh" } else { effort_name(e) };
            spec.effort_args.iter().map(|a| a.replace("{effort}", value)).collect()
        }
        _ => vec![],
    };
    let mut placed = effort_args.is_empty();
    for a in &spec.headless {
        if !placed && a.contains("{prompt}") {
            out.extend(effort_args.iter().cloned());
            placed = true;
        }
        out.push(a.replace("{binary}", &spec.binary).replace("{model}", model).replace("{session}", "configure-ask").replace("{prompt}", prompt));
    }
    if !placed {
        out.extend(effort_args);
    }
    out
}

/// A new empty folder for the run, under the system's temporary folder.
fn scratch() -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default();
    let dir = std::env::temp_dir().join(format!("map-ask-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Answer {
    summary: String,
    evidence: Vec<Value>,
    ops: Vec<Value>,
}

pub async fn ask(hub: &Hub, req: ConfigAskRequest) -> Result<ConfigAskResult> {
    ask_with(hub, req, TIMEOUT).await
}

pub(crate) async fn ask_with(hub: &Hub, mut req: ConfigAskRequest, timeout: Duration) -> Result<ConfigAskResult> {
    let provider = req.provider.trim().to_string();
    let spec = hub.spec(&provider)?;
    if spec.headless.is_empty() {
        bail!("{provider} has no headless command to ask");
    }
    if !hub.provider_available(&provider) {
        bail!("{provider} is not installed (`{}` is not on the agents' PATH)", spec.binary);
    }
    let from_runs = req.from_runs && req.tab == ConfigTab::Workflows;
    if req.prompt.trim().is_empty() && !from_runs {
        bail!("say what the agent should do");
    }
    let model = Hub::resolve_model(spec, req.model.as_deref())?;
    let project = req.project.clone().filter(|p| !p.is_empty());
    req.project = project.clone();
    let mut context = req.context.clone();
    let mut runs = None;
    if req.tab == ConfigTab::Workflows
        && let Some(id) = req.workflow_id.clone().filter(|i| !i.is_empty())
    {
        if context.is_null()
            && let Some(wf) = crate::workflows::find(hub, project.as_deref(), &id).await?
        {
            context = serde_json::to_value(wf)?;
        }
        if from_runs {
            runs = Some(serde_json::to_value(crate::workflows::stats(hub, project.as_deref(), &id).await?)?);
        }
    } else if from_runs {
        bail!("suggesting from runs needs the workflow (`workflow_id`)");
    }
    let text = prompt(&req, &context, runs.as_ref());
    let argv = argv(spec, &model, req.effort, &text);
    let dir = scratch()?;
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).current_dir(&dir).envs(hub.run_env(&provider, hub.command_env()));
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
    let started = Instant::now();
    let result = tokio::time::timeout(timeout, cmd.output()).await;
    let _ = std::fs::remove_dir_all(&dir);
    let out = match result {
        Ok(out) => out.with_context(|| format!("running {provider}"))?,
        Err(_) => bail!("{provider} gave no answer within {}s and was stopped", timeout.as_secs()),
    };
    let output = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let Some(answer) = parse_answer(&output) else {
        bail!("{provider}'s reply has no JSON object with `ops` ({})", crate::decisions::last_line(&output));
    };
    let answer: Answer = serde_json::from_value(answer).unwrap_or_default();
    let (ops, dropped) = check_ops(req.tab, &answer.ops, &context);
    let mut evidence: Vec<String> = answer.evidence.iter().filter_map(|e| e.as_str().map(str::trim).filter(|s| !s.is_empty()).map(|s| cap(s, 500).0)).take(20).collect();
    evidence.extend(dropped);
    let duration_ms = started.elapsed().as_millis() as i64;
    let place = project.as_deref().unwrap_or("the machine");
    hub.event(Level::Info, "configure.ask", None, None, format!("Asked {provider} ({model}) about {} for {place}: {} change(s) proposed", tab_name(req.tab), ops.len())).await;
    Ok(ConfigAskResult { summary: cap(answer.summary.trim(), 1000).0, evidence, ops, provider, model, language: req.language, duration_ms })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::store::Store;
    use crate::terminal::fake::FakeTerminal;
    use std::path::Path;
    use std::sync::Arc;

    /// A hub whose `claude` is a fake CLI script: it records its folder and arguments in `$ASK_OUT` and prints
    /// `$ASK_OUT/reply.txt`. No real provider CLI is ever run.
    async fn hub(script: &str) -> (Arc<Hub>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake-claude");
        std::fs::write(&fake, format!("#!/bin/sh\n{script}\n")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let text = format!(
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}/ws\"\n[workspace.env]\nASK_OUT = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n\
             [agents_home]\nmode = \"isolated\"\ndir = \"{0}/agent-home\"\n[reflex]\nbinary = \"/nonexistent/reflex\"\n\
             [providers.claude]\nbinary = \"{1}\"\ncredentials = []\nmodels = [\"opus\", \"sonnet\"]\ndefault_model = \"sonnet\"\nheadless = [\"{{binary}}\", \"-p\", \"{{prompt}}\", \"--model\", \"{{model}}\"]\n\
             [providers.codex]\nbinary = \"{1}\"\ncredentials = []\nmodels = []\ndefault_model = \"m\"\nheadless = [\"{{binary}}\", \"exec\", \"-m\", \"{{model}}\", \"{{prompt}}\"]\n\
             [providers.gemini]\nbinary = \"/nonexistent/agy\"\n",
            dir.path().display(),
            fake.display()
        );
        std::fs::create_dir_all(dir.path().join("ws")).unwrap();
        let config = Config::parse(&text, Path::new("/")).unwrap();
        (Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default())), dir)
    }

    const RECORD: &str = "pwd > \"$ASK_OUT/cwd\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$ASK_OUT/args\"\ncat \"$ASK_OUT/reply.txt\"";

    fn req(tab: ConfigTab, prompt: &str, context: Value) -> ConfigAskRequest {
        ConfigAskRequest {
            tab,
            project: None,
            provider: "claude".into(),
            model: Some("opus".into()),
            effort: Some(Effort::High),
            prompt: prompt.into(),
            language: ConfigLanguage::En,
            context,
            workflow_id: None,
            from_runs: false,
        }
    }

    fn reply(dir: &Path, text: &str) {
        std::fs::write(dir.join("reply.txt"), text).unwrap();
    }

    #[tokio::test]
    async fn agents_ops_come_back_from_json_in_the_middle_of_chatter() {
        let (hub, dir) = hub(RECORD).await;
        reply(
            dir.path(),
            "Let me look. {\"draft\": true}\nHere is my answer:\n```json\n{\"summary\": \"Add a reviewer.\", \"evidence\": [\"no agent reviews diffs\"], \"ops\": [\
             {\"op\": \"add\", \"target\": \"agent:reviewer\", \"text\": \"+ reviewer\", \"value\": {\"name\": \"reviewer\", \"description\": \"Reviews diffs\", \"instructions\": \"Review.\", \"tools\": [\"read\", \"search\"], \"models\": {\"claude\": \"opus\", \"codex\": null, \"gemini\": null}, \"effort\": \"high\", \"targets\": [\"claude\"]}},\
             {\"op\": \"change\", \"target\": \"agent:coder\", \"text\": \"~ coder: read only\", \"value\": {\"tools\": [\"read\"]}},\
             {\"op\": \"change\", \"target\": \"agent:coder\", \"text\": \"bad tool\", \"value\": {\"tools\": [\"teleport\"]}},\
             {\"op\": \"add\", \"target\": \"agent:Bad Name\", \"value\": {}},\
             {\"op\": \"rename\", \"target\": \"agent:coder\", \"value\": {}}\
             ]}\n```\nDone.",
        );
        let out = ask_with(&hub, req(ConfigTab::Agents, "add a reviewer", json!({"agents": [{"name": "coder"}]})), TIMEOUT).await.unwrap();
        assert_eq!(out.summary, "Add a reviewer.");
        assert_eq!(out.ops.iter().map(|o| (o.op, o.target.as_str())).collect::<Vec<_>>(), vec![(ConfigOpKind::Add, "agent:reviewer"), (ConfigOpKind::Change, "agent:coder")]);
        assert_eq!(out.evidence[0], "no agent reviews diffs");
        assert_eq!(out.evidence.iter().filter(|e| e.starts_with("MAP dropped")).count(), 3, "{:?}", out.evidence);
        assert_eq!((out.provider.as_str(), out.model.as_str()), ("claude", "opus"));
        // The CLI got the prompt and the model through its headless template, and ran in a scratch folder.
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(args.starts_with("-p\n") && args.contains("--model\nopus"), "{args}");
        assert!(args.contains("add a reviewer") && args.contains("\"coder\"") && args.contains("Reasoning effort: high"), "{args}");
        let cwd = std::fs::read_to_string(dir.path().join("cwd")).unwrap();
        let cwd = Path::new(cwd.trim());
        assert!(cwd.starts_with(std::fs::canonicalize(std::env::temp_dir()).unwrap()) || cwd.starts_with(std::env::temp_dir()), "{}", cwd.display());
        assert!(cwd.file_name().unwrap().to_string_lossy().starts_with("map-ask-"), "{}", cwd.display());
        assert_ne!(cwd, dir.path().join("ws"));
        assert!(!cwd.exists(), "the scratch folder is removed afterwards");
    }

    #[tokio::test]
    async fn the_cli_never_runs_in_the_project_folder() {
        let (hub, dir) = hub(RECORD).await;
        let folder = dir.path().join("app");
        let slug = crate::plans::tests::project(&hub, &folder, "App").await;
        reply(dir.path(), "{\"summary\": \"s\", \"evidence\": [], \"ops\": [{\"op\": \"add\", \"target\": \"verify.cmds\", \"text\": \"+ lint\", \"value\": {\"command\": \"bun run lint\", \"index\": 0}}, {\"op\": \"change\", \"target\": \"verify.max_attempts\", \"value\": {\"value\": 42}}]}");
        let mut r = req(ConfigTab::Verify, "lint first", json!({"verify": ["bun test"]}));
        r.project = Some(slug.clone());
        let out = ask_with(&hub, r, TIMEOUT).await.unwrap();
        assert_eq!(out.ops.len(), 1, "{:?}", out.evidence);
        assert_eq!(out.ops[0].value["command"], "bun run lint");
        assert!(out.evidence.iter().any(|e| e.contains("verify.max_attempts") && e.contains("1 to 10")), "{:?}", out.evidence);
        let cwd = std::fs::read_to_string(dir.path().join("cwd")).unwrap();
        assert!(!Path::new(cwd.trim()).starts_with(&folder) && !Path::new(cwd.trim()).starts_with(dir.path()), "{cwd}");
        // Nothing was written to the project.
        assert_eq!(std::fs::read_dir(&folder).unwrap().count(), 0);
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(args.contains(&format!("project `{slug}`")), "{args}");
    }

    #[tokio::test]
    async fn a_thai_request_with_english_config_still_asks_for_english() {
        let (hub, dir) = hub(RECORD).await;
        reply(dir.path(), "{\"summary\": \"ok\", \"ops\": []}");
        let out = ask_with(&hub, req(ConfigTab::Rules, "เพิ่มกฎให้รันเทสต์ก่อนคอมมิต", json!({"files": ["AGENTS.md"]})), TIMEOUT).await.unwrap();
        assert_eq!(out.language, ConfigLanguage::En);
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(args.contains("เพิ่มกฎให้รันเทสต์ก่อนคอมมิต"), "{args}");
        assert!(args.contains("Output language: English") && args.contains("even when the request above is written in Thai"), "{args}");
        let mut thai = req(ConfigTab::Rules, "add a rule", json!({}));
        thai.language = ConfigLanguage::Th;
        ask_with(&hub, thai, TIMEOUT).await.unwrap();
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(args.contains("Output language: Thai") && !args.contains("Output language: English"), "{args}");
    }

    #[tokio::test]
    async fn a_cli_that_hangs_is_stopped() {
        let (hub, _dir) = hub("exec sleep 30").await;
        let started = Instant::now();
        let err = ask_with(&hub, req(ConfigTab::Hooks, "add a hook", json!({})), Duration::from_millis(500)).await.unwrap_err();
        assert!(format!("{err:#}").contains("no answer within"), "{err:#}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[tokio::test]
    async fn a_reply_without_json_ops_is_an_error_and_bad_requests_are_refused() {
        let (hub, dir) = hub(RECORD).await;
        reply(dir.path(), "I would add a hook. {\"summary\": \"no ops key\"}");
        let err = ask_with(&hub, req(ConfigTab::Hooks, "add a hook", json!({})), TIMEOUT).await.unwrap_err();
        assert!(format!("{err:#}").contains("no JSON object with `ops`"), "{err:#}");
        assert!(ask_with(&hub, req(ConfigTab::Hooks, "  ", json!({})), TIMEOUT).await.is_err(), "an empty prompt");
        let mut gone = req(ConfigTab::Hooks, "x", json!({}));
        gone.provider = "gemini".into();
        assert!(format!("{:#}", ask_with(&hub, gone, TIMEOUT).await.unwrap_err()).contains("not installed"));
        let mut bad_model = req(ConfigTab::Hooks, "x", json!({}));
        bad_model.model = Some("gpt-9".into());
        assert!(ask_with(&hub, bad_model, TIMEOUT).await.is_err());
    }

    #[tokio::test]
    async fn codex_gets_a_real_effort_flag() {
        let (hub, dir) = hub(RECORD).await;
        let mut spec = hub.spec("codex").unwrap().clone();
        spec.effort_args = vec!["-c".into(), "model_reasoning_effort={effort}".into()];
        let args = argv(&spec, "m", Some(Effort::Max), "PROMPT");
        let n = args.len();
        assert_eq!(args[n - 3..], ["-c".to_string(), "model_reasoning_effort=xhigh".into(), "PROMPT".into()], "{args:?}");
        assert_eq!(argv(&spec, "m", None, "P").len(), n - 2);
        // claude has no effort flag: only the prompt says it.
        let claude = hub.spec("claude").unwrap();
        assert!(!argv(claude, "opus", Some(Effort::Low), "P").iter().any(|a| a.contains("effort")));
        let _ = dir;
    }

    #[tokio::test]
    async fn workflow_ops_are_checked_against_the_graph_and_runs_go_into_the_prompt() {
        let (hub, dir) = hub(RECORD).await;
        let wf = crate::workflows::save(&hub, None, crate::workflows::tests::bugfix()).await.unwrap();
        // One run: the verify failed once.
        let t = hub.store.create_task(crate::store::NewTask { title: "Fix login".into(), prompt: "p".into(), workflow: Some(wf.id.clone()), ..Default::default() }).await.unwrap();
        let mut t = hub.store.task(t.id).await.unwrap().unwrap();
        t.workflow_state = Some(Default::default());
        hub.put_task(&mut t).await.unwrap();
        hub.event(Level::Warning, "decision.workflow.verify.start", None, Some(t.id), "ver · `true` · visit 1 of 2").await;
        hub.event(Level::Warning, "decision.workflow.verify.fail", None, Some(t.id), "ver · 3 tests failing in auth.spec · visit 1 of 2 → code").await;
        reply(
            dir.path(),
            "{\"summary\": \"Lint first.\", \"evidence\": [\"verify failed in 1 of 1 runs\"], \"ops\": [\
             {\"op\": \"add\", \"target\": \"workflow.node:lint\", \"text\": \"+ lint before verify\", \"value\": {\"op\": \"add\", \"after\": \"code\", \"before\": \"ver\", \"failTo\": \"code\", \"node\": {\"id\": \"lint\", \"type\": \"verify\", \"label\": \"Lint\", \"cmd\": \"npm run lint\", \"maxAttempts\": 2}}},\
             {\"op\": \"add\", \"target\": \"workflow.node:notify\", \"text\": \"+ notify\", \"value\": {\"op\": \"insertBefore\", \"target\": \"failed\", \"node\": {\"id\": \"notify\", \"kind\": \"hook\", \"command\": \"echo failed\", \"event\": \"on_fail\", \"blocking\": \"no\"}}},\
             {\"op\": \"change\", \"target\": \"workflow.node:ver\", \"text\": \"~ 3 attempts\", \"value\": {\"op\": \"change\", \"id\": \"ver\", \"key\": \"maxAttempts\", \"to\": 3}},\
             {\"op\": \"change\", \"target\": \"workflow.node:ghost\", \"value\": {\"op\": \"change\", \"id\": \"ghost\", \"key\": \"label\", \"to\": \"x\"}},\
             {\"op\": \"add\", \"target\": \"workflow.node:code\", \"value\": {\"op\": \"add\", \"after\": \"t\", \"before\": \"code\", \"node\": {\"id\": \"code\", \"kind\": \"agent\", \"agent\": \"claude\"}}},\
             {\"op\": \"remove\", \"target\": \"workflow.node:code\", \"value\": {\"op\": \"remove\", \"id\": \"code\", \"bridge\": [\"t\", \"ver\", \"next\"]}},\
             {\"op\": \"change\", \"target\": \"workflow.node:ver\", \"value\": {\"op\": \"change\", \"id\": \"ver\", \"key\": \"max_attempts\", \"to\": 99}}\
             ]}",
        );
        let mut r = req(ConfigTab::Workflows, "", Value::Null);
        r.workflow_id = Some("bugfix".into());
        r.from_runs = true;
        let out = ask_with(&hub, r, TIMEOUT).await.unwrap();
        let got: Vec<(&str, &str)> = out.ops.iter().map(|o| (o.target.as_str(), o.value["op"].as_str().unwrap())).collect();
        assert_eq!(got, vec![("workflow.node:lint", "add"), ("workflow.node:notify", "insert_before"), ("workflow.node:ver", "change"), ("workflow.node:code", "remove")], "{:?}", out.evidence);
        // The editor's names come back in ours.
        assert_eq!(out.ops[0].value["node"]["kind"], "verify");
        assert_eq!(out.ops[0].value["node"]["command"], "npm run lint");
        assert_eq!(out.ops[0].value["fail_to"], "code");
        assert_eq!(out.ops[1].value["node"]["blocking"], false);
        assert_eq!(out.ops[2].value["key"], "max_attempts");
        assert_eq!(out.evidence.iter().filter(|e| e.starts_with("MAP dropped")).count(), 3, "{:?}", out.evidence);
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(args.contains("3 tests failing in auth.spec") && args.contains("suggest improvements"), "{args}");
        assert!(args.contains("\"bver\"") || args.contains("\"ver\""), "the stored workflow is the context: {args}");
    }
}
