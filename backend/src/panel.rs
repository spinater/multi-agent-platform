//! A panel fallback: two or more provider CLIs answer the same question and argue it out. A project's fallback
//! `panel:gemini/gemini-4-argon-high,claude/opus` asks each member headless, in parallel. When they agree, that is the
//! answer; when they do not, each sees the others' answers and reasons and answers once more; still split, the
//! decision abstains (the error says who said what). A member that is cooling down (quota) is left out, and one that
//! fails with a quota message is cooled down, so the rest answer alone.

use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::Value;
use tokio::process::Command;

use crate::decide::{Reply, parse_answer, same};
use crate::hub::Hub;
use crate::model::{Project, Question};

pub const PREFIX: &str = "panel:";

/// One panel member: a provider and the model it runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Member {
    pub provider: String,
    pub model: Option<String>,
}

/// The members of a `panel:` fallback (`provider/model` or `provider`, comma separated); `None` for any other fallback.
pub fn members(fallback: &str) -> Option<Vec<Member>> {
    let list = fallback.strip_prefix(PREFIX)?;
    let members = list
        .split(',')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .map(|m| match m.split_once('/') {
            Some((p, model)) => Member { provider: p.trim().into(), model: Some(model.trim().into()).filter(|m: &String| !m.is_empty()) },
            None => Member { provider: m.into(), model: None },
        })
        .collect();
    Some(members)
}

/// A `panel:` fallback names at least two members, each a provider with a headless command and a model it lists.
pub fn validate(hub: &Hub, fallback: &str) -> Result<()> {
    let Some(members) = members(fallback) else { bail!("not a panel") };
    if members.len() < 2 {
        bail!("a panel needs at least two members, as panel:gemini/gemini-4-argon-high,claude/opus");
    }
    for m in &members {
        let spec = hub.spec(&m.provider).with_context(|| format!("panel member `{}` is not a provider", m.provider))?;
        if spec.headless.is_empty() {
            bail!("provider `{}` has no headless command, so it cannot sit on a panel", m.provider);
        }
        Hub::resolve_model(spec, m.model.as_deref())?;
    }
    Ok(())
}

/// The fallback's reply to question `q` (`text` is its prompt): the panel's, or the single fallback's.
pub async fn ask(hub: &Hub, project: &Project, q: &Question, text: &str) -> Result<Reply> {
    match members(&project.fallback) {
        Some(members) => debate(hub, project, q, text, members).await,
        None => crate::decide::ask_fallback(hub, project, text).await,
    }
}

/// What one member said in one round.
struct Said {
    who: String,
    answer: Value,
    why: String,
    reply: Reply,
}

const FORMAT: &str = "Answer from the text above alone: run no command, read no file, use no tool. After that line, add one line that starts with `Why:` and gives your reason in one sentence.";

fn shown(v: &Value) -> String {
    match v {
        Value::Bool(b) => (if *b { "yes" } else { "no" }).into(),
        Value::String(s) => s.clone(),
        _ => "no usable answer".into(),
    }
}

/// The `Why:` line of a reply, else its last line.
fn why(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let line = lines.iter().find_map(|l| l.strip_prefix("Why:")).or(lines.get(1).copied()).unwrap_or("");
    line.trim().chars().take(400).collect()
}

/// The answer every member gave, when they all gave the same usable one.
fn agreed(said: &[Said]) -> Option<Value> {
    let first = &said.first()?.answer;
    (!first.is_null() && said.iter().all(|s| same(&s.answer, first))).then(|| first.clone())
}

fn summary(round: usize, said: &[Said]) -> String {
    let each: Vec<String> = said.iter().map(|s| format!("{} said {} ({})", s.who, shown(&s.answer), s.why)).collect();
    format!("Round {round}: {}", each.join(" · "))
}

async fn debate(hub: &Hub, project: &Project, q: &Question, text: &str, members: Vec<Member>) -> Result<Reply> {
    let (seated, mut notes): (Vec<Member>, Vec<String>) = members.into_iter().fold((vec![], vec![]), |(mut ok, mut out), m| {
        if !hub.provider_available(&m.provider) {
            out.push(format!("{} is not installed", m.provider));
        } else if let Some(until) = hub.cooldown_until(&m.provider) {
            out.push(format!("{} sat out (cooling down until {})", m.provider, until.format("%H:%M UTC")));
        } else {
            ok.push(m);
        }
        (ok, out)
    });
    let first = format!("{text}\n{FORMAT}");
    let mut said = round(hub, project, q, &seated, |_| first.clone(), &mut notes).await;
    if said.is_empty() {
        bail!("no panel member answered: {}", notes.join("; "));
    }
    let mut log = vec![summary(1, &said)];
    if agreed(&said).is_none() && said.len() > 1 {
        let earlier: Vec<(String, String)> = said.iter().filter(|s| !s.answer.is_null()).map(|s| (s.who.clone(), format!("`{}`. {}", shown(&s.answer), s.why))).collect();
        let still: Vec<Member> = seated.iter().filter(|m| said.iter().any(|s| s.who == label(m))).cloned().collect();
        let prompt = |m: &Member| {
            let others: Vec<String> = earlier.iter().filter(|(w, _)| *w != label(m)).map(|(w, a)| format!("- {w} answered {a}")).collect();
            let others = if others.is_empty() { vec!["- nobody else gave a usable answer".to_string()] } else { others };
            format!(
                "{text}\n\nYou answered this before. The other reviewers on the panel answered:\n{}\nWeigh their reasons against yours: keep your answer or change it.\nReply in the same format: the answer line first. {FORMAT}",
                others.join("\n")
            )
        };
        let second = round(hub, project, q, &still, prompt, &mut notes).await;
        if !second.is_empty() {
            log.push(summary(2, &second));
            said = second;
        }
    }
    log.extend(notes);
    let (tokens_in, tokens_out) = said.iter().fold((0, 0), |(i, o), s| (i + s.reply.tokens_in, o + s.reply.tokens_out));
    let reply = |first_line: String| Reply { text: format!("{first_line}\n{}", log.join("\n")), tokens_in, tokens_out, measured: false };
    match agreed(&said) {
        Some(answer) => Ok(reply(shown(&answer))),
        None if said.len() == 1 => Ok(reply(said[0].reply.text.lines().next().unwrap_or("").to_string())),
        None => bail!("the panel did not agree. {}", log.join(" ")),
    }
}

fn label(m: &Member) -> String {
    m.model.clone().unwrap_or_else(|| m.provider.clone())
}

/// Asks every member in parallel; a member that fails is left out (and cooled down on a quota message).
async fn round(hub: &Hub, project: &Project, q: &Question, members: &[Member], prompt: impl Fn(&Member) -> String, notes: &mut Vec<String>) -> Vec<Said> {
    let asks = members.iter().map(|m| {
        let p = prompt(m);
        // A reply with no usable answer (agy now and then prints nothing) is asked once more.
        async move {
            let first = run_cli(hub, &project.slug, &m.provider, m.model.as_deref(), &p).await;
            match first {
                Ok(r) if parse_answer(q, &r.text).is_null() => (m, run_cli(hub, &project.slug, &m.provider, m.model.as_deref(), &p).await, Some(r.text)),
                other => (m, other, None),
            }
        }
    });
    let mut said = vec![];
    for (m, result, retried) in futures::future::join_all(asks).await {
        if let Some(raw) = retried {
            let raw: String = raw.chars().take(200).collect();
            notes.push(format!("{} gave no usable answer ({:?}) and was asked again", label(m), raw.trim()));
        }
        match result {
            Ok(reply) => said.push(Said { who: label(m), answer: parse_answer(q, &reply.text), why: why(&reply.text), reply }),
            Err(err) => {
                let err = format!("{err:#}");
                if crate::orchestrator::looks_like_quota(&err) {
                    hub.cool_down(&m.provider, Utc::now() + chrono::Duration::minutes(hub.config.orchestrator.provider_cooldown_minutes as i64));
                    notes.push(format!("{} is out of quota, so the others answered alone", m.provider));
                } else {
                    notes.push(format!("{} failed: {}", label(m), err.chars().take(300).collect::<String>()));
                }
            }
        }
    }
    said
}

/// `provider`'s headless command run on `text`, in an empty directory of the project's (in the workspace the agent
/// would read CLAUDE.md and call the decide endpoint again).
pub async fn run_cli(hub: &Hub, slug: &str, provider: &str, model: Option<&str>, text: &str) -> Result<Reply> {
    let spec = hub.spec(provider)?;
    let model = Hub::resolve_model(spec, model)?;
    let argv: Vec<String> = spec
        .headless
        .iter()
        .map(|a| a.replace("{binary}", &spec.binary).replace("{model}", &model).replace("{session}", "decide").replace("{prompt}", text))
        .collect();
    let Some((program, args)) = argv.split_first() else { bail!("provider `{provider}` has no headless command") };
    let dir = hub.project_dir(slug).join("fallback");
    tokio::fs::create_dir_all(&dir).await?;
    let mut extra: Vec<(String, String)> = hub.config.workspace.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    extra.extend(crate::env_vars::vars_for(hub, Some(slug)).await);
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(&dir).envs(hub.run_env(provider, extra)).stdin(Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(180), cmd.output()).await.context("the fallback timed out")??;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // agy prints nothing and exits 0 when a tool it wanted was denied; its stderr says why.
    if !out.status.success() || stdout.is_empty() {
        bail!("{provider} failed: {} {}", String::from_utf8_lossy(&out.stderr).trim(), stdout.chars().take(300).collect::<String>());
    }
    Ok(Reply::new(stdout, text.chars().count() as i64, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_members() {
        let m = members("panel:gemini/gemini-4-argon-high, claude/opus,codex").unwrap();
        assert_eq!(m.len(), 3);
        assert_eq!(m[0], Member { provider: "gemini".into(), model: Some("gemini-4-argon-high".into()) });
        assert_eq!(m[2].model, None);
        assert!(members("claude").is_none());
    }

    #[test]
    fn reads_why_and_agreement() {
        assert_eq!(why("yes\nWhy: the invoice is 12 days old."), "the invoice is 12 days old.");
        assert_eq!(why("no\nbecause"), "because");
        let s = |a: Value| Said { who: "x".into(), answer: a, why: String::new(), reply: Reply::new(String::new(), 0, None) };
        assert_eq!(agreed(&[s(Value::Bool(true)), s(Value::Bool(true))]), Some(Value::Bool(true)));
        assert_eq!(agreed(&[s(Value::Bool(true)), s(Value::Bool(false))]), None);
        assert_eq!(agreed(&[s(Value::Null), s(Value::Null)]), None);
        assert_eq!(agreed(&[s("Light".into()), s("light".into())]), Some(Value::String("Light".into())));
    }
}
