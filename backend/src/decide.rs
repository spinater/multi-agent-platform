//! The decision gate: ask the project's Reflex first, accept its answer at or above the project's threshold, and
//! send only the rest to the fallback (the Anthropic API or a provider CLI run headless).

use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::{Value, json};
use tokio::process::Command;

use crate::hub::Hub;
use crate::model::{Answer, DecideResponse, Decision, FitCaseResult, FitResult, Lane, Project, Question, QuestionKind};
use crate::projects::validate_questions;
use crate::savings::estimate_tokens;
use crate::store::DecisionCost;

/// Which path the caller wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Reflex, then the fallback for whatever is below the threshold.
    #[default]
    Auto,
    /// Reflex's own answer whatever its confidence; never the fallback.
    Reflex,
    /// Only the fallback.
    Llm,
}

/// Reflex's outcome as the option text (choice, score) or a boolean (noul); `null` when it abstained.
pub fn decode(q: &Question, outcome: &Value) -> Value {
    let options = q.options.as_deref().unwrap_or_default();
    let pick = |i: Option<u64>| i.and_then(|i| options.get(i as usize)).map_or(Value::Null, |o| Value::String(o.clone()));
    if let Some(c) = outcome.get("choice") {
        pick(c["index"].as_u64())
    } else if let Some(s) = outcome.get("score") {
        pick(s["level"].as_u64())
    } else if let Some(n) = outcome.get("noul") {
        n.get("yes").filter(|v| v.is_boolean()).cloned().unwrap_or(Value::Null)
    } else {
        Value::Null
    }
}

/// What the fallback is sent for one question. Its length is recorded for every decision (savings.rs).
pub fn prompt(state: &str, q: &Question) -> String {
    let mut p = format!("State:\n{state}\n\nQuestion: {}\n", q.prompt);
    if let Some(c) = &q.criteria {
        p.push_str(&format!("Criteria: {c}\n"));
    }
    let options = q.options.as_deref().unwrap_or_default().join(", ");
    match q.kind {
        QuestionKind::Noul => p.push_str("Reply with exactly one word, yes or no, and nothing else."),
        QuestionKind::Score => p.push_str(&format!("Rate the state with exactly one of these levels (lowest first): {options}. Reply with the level only.")),
        QuestionKind::Choice => p.push_str(&format!("Reply with exactly one of these options: {options}. Reply with the option only.")),
    }
    p
}

/// Maps the model's reply onto the question's answer type; `null` when it named nothing recognisable.
/// The answer is expected on a line of its own: the first line, else the last (a model may think aloud first).
pub fn parse_answer(q: &Question, raw: &str) -> Value {
    let lines: Vec<&str> = raw.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let mut candidates: Vec<&str> = lines.first().into_iter().chain(lines.last()).copied().collect();
    candidates.dedup();
    let clean = |s: &str| s.trim().trim_matches(|c: char| "\"'`.*".contains(c)).trim().to_lowercase();
    if q.kind == QuestionKind::Noul {
        for line in &candidates {
            let t = clean(line);
            if t.starts_with("yes") || t.starts_with("ใช่") {
                return Value::Bool(true);
            }
            if t.starts_with("no") || t.starts_with("ไม่") {
                return Value::Bool(false);
            }
        }
        return Value::Null;
    }
    let options = q.options.as_deref().unwrap_or_default();
    for line in &candidates {
        if let Some(o) = options.iter().find(|o| o.to_lowercase() == clean(line)) {
            return Value::String(o.clone());
        }
    }
    // Longest first, so "needs-attention" wins over "attention".
    let mut by_len: Vec<&String> = options.iter().collect();
    by_len.sort_by_key(|o| std::cmp::Reverse(o.len()));
    for line in &candidates {
        let lower = clean(line);
        if let Some(o) = by_len.iter().find(|o| lower.contains(&o.to_lowercase())) {
            return Value::String((*o).clone());
        }
    }
    Value::Null
}

/// The fallback's raw reply and the tokens it took.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub text: String,
    pub tokens_in: i64,
    pub tokens_out: i64,
    /// The provider reported the tokens; else they are estimated from the prompt and the reply.
    pub measured: bool,
}

impl Reply {
    /// Tokens as the provider reported them (`usage`), else about 4 chars per token of the prompt and the reply.
    /// A headless CLI reports nothing, and its own system prompt is not counted (`fallback_overhead_tokens`).
    pub fn new(text: String, prompt_chars: i64, usage: Option<(i64, i64)>) -> Self {
        match usage {
            Some((tokens_in, tokens_out)) => Self { text, tokens_in, tokens_out, measured: true },
            None => {
                let tokens_out = estimate_tokens(text.chars().count() as i64);
                Self { text, tokens_in: estimate_tokens(prompt_chars), tokens_out, measured: false }
            }
        }
    }
}

/// `usage` of an Anthropic Messages response: input tokens (with cache writes and reads) and output tokens.
pub fn api_usage(body: &Value) -> Option<(i64, i64)> {
    let u = body.get("usage")?;
    let input = u["input_tokens"].as_i64()? + u["cache_creation_input_tokens"].as_i64().unwrap_or(0) + u["cache_read_input_tokens"].as_i64().unwrap_or(0);
    Some((input, u["output_tokens"].as_i64()?))
}

/// Returns the fallback's raw reply to `text`, the question's prompt.
async fn ask_fallback(hub: &Hub, project: &Project, text: &str) -> Result<Reply> {
    let prompt_chars = text.chars().count() as i64;
    match project.fallback.as_str() {
        "none" => bail!("this project has no fallback"),
        "api" => {
            let env = &hub.config.reflex.api_key_env;
            let key = std::env::var(env).ok().filter(|k| !k.is_empty()).with_context(|| format!("{env} is not set on the server"))?;
            let model = project.model.clone().unwrap_or_else(|| hub.config.reflex.api_model.clone());
            let resp = hub
                .http
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
                .timeout(Duration::from_secs(60))
                .json(&json!({ "model": model, "max_tokens": 64, "messages": [{ "role": "user", "content": text }] }))
                .send()
                .await?;
            let code = resp.status();
            let body: Value = resp.json().await?;
            if !code.is_success() {
                bail!("Anthropic API {code}: {}", body["error"]["message"].as_str().unwrap_or("?"));
            }
            let reply = body["content"].as_array().into_iter().flatten().filter_map(|b| b["text"].as_str()).collect();
            Ok(Reply::new(reply, prompt_chars, api_usage(&body)))
        }
        provider => {
            let spec = hub.spec(provider)?;
            let model = Hub::resolve_model(spec, project.model.as_deref())?;
            let argv: Vec<String> = spec
                .headless
                .iter()
                .map(|a| a.replace("{binary}", &spec.binary).replace("{model}", &model).replace("{session}", "decide").replace("{prompt}", text))
                .collect();
            let Some((program, args)) = argv.split_first() else { bail!("provider `{provider}` has no headless command") };
            // An empty directory of its own: in the workspace the agent would read CLAUDE.md and call this endpoint again.
            let dir = hub.project_dir(&project.slug).join("fallback");
            tokio::fs::create_dir_all(&dir).await?;
            let mut cmd = Command::new(program);
            cmd.args(args)
                .current_dir(&dir)
                .envs(&hub.config.workspace.env)
                .stdin(Stdio::null())
                .kill_on_drop(true);
            let out = tokio::time::timeout(Duration::from_secs(180), cmd.output()).await.context("the fallback timed out")??;
            if !out.status.success() {
                bail!("{provider} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
            Ok(Reply::new(String::from_utf8_lossy(&out.stdout).trim().to_string(), prompt_chars, None))
        }
    }
}

/// Reflex tokenises English only: Thai text (U+0E00–U+0E7F) never reaches it and goes to the fallback.
pub fn has_thai(text: &str) -> bool {
    text.chars().any(|c| ('\u{0E00}'..='\u{0E7F}').contains(&c))
}

/// Confidence as the dashboard writes it: `2.1e-4`.
pub fn fmt_conf(c: f64) -> String {
    if c == 0.0 { "0".into() } else { format!("{c:.1e}") }
}

/// What Reflex's part of a decision amounted to, before the fallback (if any) is asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Gate {
    /// The caller asked for the fallback only.
    LlmOnly,
    /// Thai input: Reflex was not asked.
    Thai,
    /// Reflex could not be asked.
    Unreachable,
    /// Reflex answered nothing (`outcome: null`).
    Abstained,
    /// No threshold yet: Reflex scored in shadow, its answer is not used.
    Shadow(f64),
    Above(f64, f64),
    Below(f64, f64),
    /// `route: reflex`: Reflex's answer whatever its confidence.
    Forced(f64),
}

impl Gate {
    pub fn passes(&self) -> bool {
        matches!(self, Gate::Above(..) | Gate::Forced(_))
    }

    /// The reason for the lane, given who the fallback is (`None`: it is not asked).
    pub fn reason(&self, fallback: Option<&str>) -> String {
        let to = |what: String| match fallback {
            Some(f) => format!("{what}, so it went to {f}."),
            None => format!("{what}, and no fallback was asked."),
        };
        match self {
            Gate::LlmOnly => to("Fallback only was requested".into()),
            Gate::Thai => to("Thai input: Reflex reads English only and was not asked".into()),
            Gate::Unreachable => to("Reflex is not reachable".into()),
            Gate::Abstained => to("Reflex abstained".into()),
            Gate::Shadow(c) => to(format!("Not calibrated: Reflex scored {} in shadow, but there is no threshold yet", fmt_conf(*c))),
            Gate::Above(c, t) => format!("Confidence {} is at or above the threshold {}: answered locally, no LLM call.", fmt_conf(*c), fmt_conf(*t)),
            Gate::Below(c, t) => to(format!("Confidence {} is below the threshold {}", fmt_conf(*c), fmt_conf(*t))),
            Gate::Forced(c) => format!("Reflex only was requested: its answer at confidence {}, threshold not applied.", fmt_conf(*c)),
        }
    }
}

/// Reflex's verdict on one question, from its answer (`None` when Reflex was not asked or not reachable).
pub fn gate(route: Route, thai: bool, threshold: Option<f64>, reflex_answer: &Value, confidence: Option<f64>, asked: bool) -> Gate {
    if route == Route::Llm {
        return Gate::LlmOnly;
    }
    if thai {
        return Gate::Thai;
    }
    if !asked {
        return Gate::Unreachable;
    }
    let c = confidence.unwrap_or(0.0);
    if reflex_answer.is_null() {
        return Gate::Abstained;
    }
    if route == Route::Reflex {
        return Gate::Forced(c);
    }
    match threshold {
        None => Gate::Shadow(c),
        Some(t) if c >= t => Gate::Above(c, t),
        Some(t) => Gate::Below(c, t),
    }
}

pub async fn decide(hub: &Hub, project: &Project, state: &str, questions: &[Question], route: Route) -> Result<DecideResponse> {
    validate_questions(questions)?;
    let started = Instant::now();
    let thai = has_thai(state);
    let (reflex, reflex_error) = if route == Route::Llm || thai {
        (None, None)
    } else {
        match hub.reflex.post(project.port, "/decide", &json!({ "state": state, "questions": questions })).await {
            Ok(v) => (Some(v), None),
            Err(err) => (None, Some(format!("{err:#}"))),
        }
    };
    // One Reflex call answers every question; each answer's time includes it.
    let reflex_ms = started.elapsed().as_millis() as u64;
    let by_id = |id: &str| reflex.as_ref().and_then(|v| v["answers"].as_array()).and_then(|a| a.iter().find(|x| x["question_id"] == id)).cloned();

    let mut answers = vec![];
    for q in questions {
        let t0 = Instant::now();
        let r = by_id(&q.id);
        let confidence = r.as_ref().and_then(|a| a["confidence"].as_f64());
        let probabilities = r.as_ref().and_then(|a| serde_json::from_value::<Vec<f64>>(a["probabilities"].clone()).ok());
        let reflex_answer = r.as_ref().map_or(Value::Null, |a| decode(q, &a["outcome"]));
        let g = gate(route, thai, project.threshold, &reflex_answer, confidence, r.is_some());
        let use_fallback = match route {
            Route::Llm => true,
            Route::Reflex => false,
            Route::Auto => !g.passes() && project.fallback != "none",
        };
        let mut reason = g.reason(use_fallback.then_some(project.fallback.as_str()));
        // What the fallback is (or would have been) sent, recorded for every decision: the savings report prices a
        // reflex-lane answer by it.
        let text = prompt(state, q);
        let mut cost = DecisionCost {
            prompt_chars: Some(text.chars().count() as i64),
            fallback: Some(project.fallback.clone()),
            skipped_thai: g == Gate::Thai,
            ..DecisionCost::default()
        };

        let (answer, lane, llm_raw, error) = if use_fallback {
            match ask_fallback(hub, project, &text).await {
                Ok(reply) => {
                    (cost.tokens_in, cost.tokens_out, cost.measured) = (Some(reply.tokens_in), Some(reply.tokens_out), reply.measured);
                    let parsed = parse_answer(q, &reply.text);
                    let lane = if parsed.is_null() { Lane::Abstain } else { Lane::Llm };
                    let error = parsed.is_null().then(|| "the fallback's reply matched no option".to_string());
                    (parsed, lane, Some(reply.text), error)
                }
                Err(err) => (Value::Null, Lane::Abstain, None, Some(format!("{err:#}"))),
            }
        } else if g.passes() {
            (reflex_answer.clone(), Lane::Reflex, None, None)
        } else {
            (Value::Null, Lane::Abstain, None, None)
        };
        if use_fallback && lane == Lane::Abstain {
            reason.push_str(" The fallback gave no usable answer either.");
        }

        let ms = reflex_ms + t0.elapsed().as_millis() as u64;
        let record = Decision {
            id: 0,
            project: project.slug.clone(),
            at: Utc::now(),
            state: state.chars().take(1000).collect(),
            question: q.clone(),
            answer: answer.clone(),
            lane,
            confidence,
            reflex_answer: reflex_answer.clone(),
            ms: ms as i64,
            error: error.clone(),
            feedback: None,
        };
        let decision_id = match hub.store.insert_decision(&record, &cost).await {
            Ok(id) => id,
            Err(err) => {
                tracing::warn!("could not record a decision: {err:#}");
                0
            }
        };
        answers.push(Answer { question_id: q.id.clone(), answer, lane, confidence, probabilities, reflex_answer, llm_raw, error, ms, reason, decision_id });
    }
    hub.announce_project(&project.slug).await;
    Ok(DecideResponse {
        answers,
        threshold: project.threshold,
        reflex_reason: reflex.as_ref().and_then(|v| v["routing"]["reason"].as_str().map(String::from)),
        reflex_error,
        ms: started.elapsed().as_millis() as u64,
    })
}

pub fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::String(x), Value::String(y)) => x.trim().eq_ignore_ascii_case(y.trim()),
        _ => a == b,
    }
}

/// The ρ-quantile of the confidences Reflex had on the cases it got right (the reflex-integration skill's recipe):
/// at that threshold roughly ρ of its correct answers still go to the fallback, trading coverage for precision.
pub fn quantile_threshold(mut correct: Vec<f64>, rho: f64) -> Option<f64> {
    if correct.is_empty() {
        return None;
    }
    correct.sort_by(f64::total_cmp);
    let i = (rho.clamp(0.0, 1.0) * (correct.len() - 1) as f64).floor() as usize;
    Some(correct[i])
}

/// Runs labeled cases through Reflex only (never the fallback) and fits the threshold from them.
pub async fn fit(hub: &Hub, project: &Project, cases: &[(String, Question, Value)], rho: f64) -> Result<FitResult> {
    if cases.is_empty() {
        bail!("no cases to fit on");
    }
    let mut rows = vec![];
    for (state, q, expected) in cases {
        validate_questions(std::slice::from_ref(q))?;
        let v = hub.reflex.post(project.port, "/decide", &json!({ "state": state, "questions": [q] })).await?;
        let a = &v["answers"][0];
        let answer = decode(q, &a["outcome"]);
        let correct = !answer.is_null() && same(&answer, expected);
        rows.push(FitCaseResult { confidence: a["confidence"].as_f64().unwrap_or(0.0), correct, answer });
    }
    let threshold = quantile_threshold(rows.iter().filter(|r| r.correct).map(|r| r.confidence).collect(), rho);
    let passing: Vec<&FitCaseResult> = match threshold {
        Some(t) => rows.iter().filter(|r| !r.answer.is_null() && r.confidence >= t).collect(),
        None => vec![],
    };
    let n = rows.len();
    Ok(FitResult {
        threshold,
        rho,
        n,
        n_answered: rows.iter().filter(|r| !r.answer.is_null()).count(),
        n_correct: rows.iter().filter(|r| r.correct).count(),
        n_pass: passing.len(),
        pass_precision: (!passing.is_empty()).then(|| passing.iter().filter(|r| r.correct).count() as f64 / passing.len() as f64),
        coverage: passing.len() as f64 / n as f64,
        applied: false,
        cases: rows,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice() -> Question {
        Question {
            id: "route".into(),
            kind: QuestionKind::Choice,
            prompt: "Route".into(),
            options: Some(vec!["refund".into(), "shipping".into(), "needs-attention".into(), "attention".into()]),
            criteria: None,
        }
    }

    #[test]
    fn decodes_reflex_outcomes() {
        let q = choice();
        assert_eq!(decode(&q, &json!({"choice": {"index": 1}})), json!("shipping"));
        assert_eq!(decode(&q, &json!({"choice": {"index": 9}})), Value::Null);
        assert_eq!(decode(&q, &Value::Null), Value::Null);
        let yn = Question { kind: QuestionKind::Noul, options: None, ..q };
        assert_eq!(decode(&yn, &json!({"noul": {"yes": false}})), json!(false));
    }

    #[test]
    fn parses_fallback_replies() {
        let q = choice();
        assert_eq!(parse_answer(&q, "Shipping."), json!("shipping"));
        assert_eq!(parse_answer(&q, "I think it is needs-attention"), json!("needs-attention"));
        assert_eq!(parse_answer(&q, "thinking...\n**refund**"), json!("refund"));
        assert_eq!(parse_answer(&q, "refund\n\nNote: I could not reach the shipping endpoint."), json!("refund"));
        assert_eq!(parse_answer(&q, "no idea"), Value::Null);
        let yn = Question { kind: QuestionKind::Noul, options: None, ..q };
        assert_eq!(parse_answer(&yn, "Yes"), json!(true));
        assert_eq!(parse_answer(&yn, "ไม่ใช่"), json!(false));
    }

    #[test]
    fn thai_input_skips_reflex() {
        assert!(has_thai("ฐานข้อมูล db-2 ตอบช้ามาก"));
        assert!(has_thai("replica lag on db-2 ไหม"));
        assert!(!has_thai("Replica lag 45s on db-2, primary CPU at 95%"));
        assert!(!has_thai("café ünïcode 日本語"));
        let answer = json!("billing");
        let g = gate(Route::Auto, true, Some(1e-4), &answer, Some(5e-4), true);
        assert_eq!(g, Gate::Thai);
        assert!(!g.passes());
        assert_eq!(g.reason(Some("claude")), "Thai input: Reflex reads English only and was not asked, so it went to claude.");
        assert_eq!(gate(Route::Reflex, true, None, &Value::Null, None, false), Gate::Thai);
    }

    #[test]
    fn gate_follows_threshold_and_route() {
        let a = json!("billing");
        assert_eq!(gate(Route::Auto, false, Some(1e-4), &a, Some(2.1e-4), true), Gate::Above(2.1e-4, 1e-4));
        assert!(gate(Route::Auto, false, Some(1e-4), &a, Some(2.1e-4), true).passes());
        assert_eq!(gate(Route::Auto, false, Some(1e-4), &a, Some(3e-5), true), Gate::Below(3e-5, 1e-4));
        // Not calibrated: shadow mode, never passes.
        let shadow = gate(Route::Auto, false, None, &a, Some(2e-4), true);
        assert_eq!(shadow, Gate::Shadow(2e-4));
        assert!(!shadow.passes());
        assert_eq!(gate(Route::Auto, false, Some(1e-4), &Value::Null, Some(9e-4), true), Gate::Abstained);
        assert_eq!(gate(Route::Auto, false, Some(1e-4), &Value::Null, None, false), Gate::Unreachable);
        assert!(gate(Route::Reflex, false, None, &a, Some(1e-5), true).passes());
        assert_eq!(gate(Route::Llm, false, Some(1e-4), &a, Some(1e-3), true), Gate::LlmOnly);
        assert_eq!(Gate::Above(2.13e-4, 1e-4).reason(None), "Confidence 2.1e-4 is at or above the threshold 1.0e-4: answered locally, no LLM call.");
        assert_eq!(Gate::Below(3e-5, 1e-4).reason(None), "Confidence 3.0e-5 is below the threshold 1.0e-4, and no fallback was asked.");
    }

    #[test]
    fn fallback_tokens_are_measured_or_estimated() {
        // The shape of a Messages API response; no test calls the API.
        let body = json!({ "content": [{ "type": "text", "text": "refund" }], "usage": { "input_tokens": 61, "output_tokens": 3 } });
        assert_eq!(api_usage(&body), Some((61, 3)));
        let cached = json!({ "usage": { "input_tokens": 10, "cache_read_input_tokens": 40, "cache_creation_input_tokens": 5, "output_tokens": 2 } });
        assert_eq!(api_usage(&cached), Some((55, 2)));
        assert_eq!(api_usage(&json!({ "content": [] })), None);
        assert_eq!(api_usage(&json!({ "usage": { "input_tokens": 3 } })), None);

        let r = Reply::new("refund".into(), 201, Some((61, 3)));
        assert_eq!((r.tokens_in, r.tokens_out, r.measured), (61, 3, true));
        // CLI: ceil(201 / 4) = 51 in, ceil(6 / 4) = 2 out.
        let r = Reply::new("refund".into(), 201, None);
        assert_eq!((r.tokens_in, r.tokens_out, r.measured), (51, 2, false));
        // Characters, not bytes.
        assert_eq!(Reply::new("ใช่".into(), 0, None).tokens_out, 1);

        let q = choice();
        let p = prompt("charged twice", &q);
        assert!(p.starts_with("State:\ncharged twice\n\nQuestion: Route\n"));
        assert!(p.ends_with("Reply with the option only."));
    }

    #[test]
    fn threshold_is_the_rho_quantile_of_correct_confidences() {
        assert_eq!(quantile_threshold(vec![], 0.3), None);
        assert_eq!(quantile_threshold(vec![0.4, 0.1, 0.3, 0.2], 0.0), Some(0.1));
        assert_eq!(quantile_threshold(vec![0.4, 0.1, 0.3, 0.2], 0.5), Some(0.2));
        assert_eq!(quantile_threshold(vec![0.4, 0.1, 0.3, 0.2], 1.0), Some(0.4));
    }
}
