//! Reflex savings (`GET /api/reflex/savings`): the LLM tokens Reflex saved by answering locally, against what the
//! fallbacks spent on the rest. The store loads one `Sample` per decision; everything after that is arithmetic in
//! pure functions.
//!
//! - **Spent**: input + output tokens of each llm-lane decision, as the API reported them, else estimated at about
//!   4 chars per token of the prompt and the reply (a headless CLI reports nothing).
//! - **Saved**: what each reflex-lane decision's prompt would have cost. A project with 3 or more measured llm-lane
//!   decisions in the last 30 days prices it at their mean tokens per call, scaled by this prompt's length against
//!   their mean prompt length; otherwise `ceil(prompt chars / 4) + 16`.
//! - **Overhead**: `[reflex] fallback_overhead_tokens` adds a constant per call of that fallback to both.

use std::collections::{BTreeMap, HashMap};

use anyhow::Result;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde_json::Value;

use crate::decide;
use crate::hub::Hub;
use crate::model::{Lane, Question, SavingsDay, SavingsReport, SavingsRow, TokenMethod};
use crate::store::DecisionCost;

pub const CHARS_PER_TOKEN: i64 = 4;
/// Reply tokens assumed for an answer the fallback never gave: one option, with some slack.
pub const ESTIMATED_REPLY_TOKENS: i64 = 16;
/// Measured calls a project needs before its saved tokens are priced from them.
pub const MIN_MEASURED_CALLS: i64 = 3;
/// How far back measured calls are looked for.
pub const BASELINE_DAYS: i64 = 30;
/// The longest window: 30 days.
pub const MAX_HOURS: i64 = 24 * 30;

/// Tokens in `chars` characters, at about 4 per token, rounded up.
pub fn estimate_tokens(chars: i64) -> i64 {
    (chars.max(0) + CHARS_PER_TOKEN - 1) / CHARS_PER_TOKEN
}

/// One decision as the report reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub project: String,
    pub at: DateTime<Utc>,
    pub lane: Lane,
    pub ms: i64,
    /// Length of the fallback prompt, sent or not.
    pub prompt_chars: i64,
    /// Input + output tokens the fallback used, and whether the provider reported them.
    pub spent: Option<(i64, bool)>,
    /// The project's fallback at the time; `None` on rows from before it was recorded.
    pub fallback: Option<String>,
    pub skipped_thai: bool,
}

impl Sample {
    /// A row recorded with its cost (migration 0008 on).
    pub fn recorded(project: String, at: DateTime<Utc>, lane: Lane, ms: i64, cost: &DecisionCost) -> Self {
        let prompt_chars = cost.prompt_chars.unwrap_or(0);
        let spent = match (cost.tokens_in, cost.tokens_out) {
            (Some(tokens_in), Some(tokens_out)) => Some((tokens_in + tokens_out, cost.measured)),
            // An llm-lane row without its tokens: priced like a prompt Reflex answered.
            _ if lane == Lane::Llm => Some((estimate_tokens(prompt_chars) + ESTIMATED_REPLY_TOKENS, false)),
            _ => None,
        };
        Self { project, at, lane, ms, prompt_chars, spent, fallback: cost.fallback.clone(), skipped_thai: cost.skipped_thai }
    }

    /// A row from before migration 0008: the prompt length from its state and question (the state was stored cut to
    /// 1000 chars, so a longer one counts short), Thai from the state, and an llm-lane answer's tokens estimated
    /// from the prompt and the answer it gave (its raw reply was not kept).
    pub fn legacy(project: String, at: DateTime<Utc>, lane: Lane, ms: i64, state: &str, question: &Question, answer: &Value) -> Self {
        let prompt_chars = decide::prompt(state, question).chars().count() as i64;
        let answer_chars = match answer {
            Value::String(s) => s.chars().count() as i64,
            Value::Bool(true) => 3,
            Value::Bool(false) => 2,
            _ => 0,
        };
        let spent = (lane == Lane::Llm).then(|| (estimate_tokens(prompt_chars) + estimate_tokens(answer_chars).max(1), false));
        Self { project, at, lane, ms, prompt_chars, spent, fallback: None, skipped_thai: decide::has_thai(state) }
    }
}

/// A project's measured fallback calls: mean tokens and mean prompt length per call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Baseline {
    pub tokens_per_call: f64,
    pub chars_per_call: f64,
}

impl Baseline {
    /// From `calls` measured calls with `tokens` tokens and `chars` prompt chars in all; `None` under 3 calls.
    pub fn from_calls(calls: i64, tokens: i64, chars: i64) -> Option<Self> {
        (calls >= MIN_MEASURED_CALLS && chars > 0).then(|| Self { tokens_per_call: tokens as f64 / calls as f64, chars_per_call: chars as f64 / calls as f64 })
    }
}

/// What a prompt of `prompt_chars` would have cost the fallback, and whether that rests on measured calls.
pub fn would_cost(prompt_chars: i64, baseline: Option<&Baseline>) -> (f64, bool) {
    match baseline {
        Some(b) => (b.tokens_per_call * prompt_chars as f64 / b.chars_per_call, true),
        None => ((estimate_tokens(prompt_chars) + ESTIMATED_REPLY_TOKENS) as f64, false),
    }
}

/// A project of the report, with its current fallback (for rows that did not record theirs).
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectRef {
    pub slug: String,
    pub name: String,
    pub fallback: String,
}

#[derive(Debug, Clone, Default)]
struct Acc {
    reflex: i64,
    llm: i64,
    abstain: i64,
    thai: i64,
    saved: f64,
    spent: f64,
    measured: i64,
    estimated: i64,
    ms_reflex: i64,
    ms_llm: i64,
}

impl Acc {
    /// Adds one decision; returns the tokens it saved and spent.
    fn add(&mut self, s: &Sample, baseline: Option<&Baseline>, overhead: f64) -> (f64, f64) {
        if s.skipped_thai {
            self.thai += 1;
        }
        let mut count = |measured: bool| if measured { self.measured += 1 } else { self.estimated += 1 };
        match s.lane {
            Lane::Reflex => {
                let (tokens, measured) = would_cost(s.prompt_chars, baseline);
                count(measured);
                self.reflex += 1;
                self.ms_reflex += s.ms;
                self.saved += tokens + overhead;
                (tokens + overhead, 0.0)
            }
            Lane::Llm => {
                let (tokens, measured) = s.spent.unwrap_or((0, false));
                count(measured);
                self.llm += 1;
                self.ms_llm += s.ms;
                self.spent += tokens as f64 + overhead;
                (0.0, tokens as f64 + overhead)
            }
            Lane::Abstain => {
                self.abstain += 1;
                (0.0, 0.0)
            }
        }
    }

    fn merge(&mut self, o: &Acc) {
        self.reflex += o.reflex;
        self.llm += o.llm;
        self.abstain += o.abstain;
        self.thai += o.thai;
        self.saved += o.saved;
        self.spent += o.spent;
        self.measured += o.measured;
        self.estimated += o.estimated;
        self.ms_reflex += o.ms_reflex;
        self.ms_llm += o.ms_llm;
    }

    fn row(&self, slug: &str, name: &str) -> SavingsRow {
        let avg = |sum: i64, n: i64| (n > 0).then(|| sum as f64 / n as f64);
        let (avg_ms_reflex, avg_ms_llm) = (avg(self.ms_reflex, self.reflex), avg(self.ms_llm, self.llm));
        let (saved, spent) = (self.saved.round() as i64, self.spent.round() as i64);
        SavingsRow {
            slug: slug.into(),
            name: name.into(),
            decisions: self.reflex + self.llm + self.abstain,
            reflex: self.reflex,
            llm: self.llm,
            abstain: self.abstain,
            skipped_thai: self.thai,
            llm_calls_avoided: self.reflex,
            tokens_saved: saved,
            tokens_spent: spent,
            saved_pct: (saved + spent > 0).then(|| 100.0 * saved as f64 / (saved + spent) as f64),
            method: match (self.measured, self.estimated) {
                (0, _) => TokenMethod::Estimated,
                (_, 0) => TokenMethod::Measured,
                _ => TokenMethod::Mixed,
            },
            avg_ms_reflex,
            avg_ms_llm,
            time_saved_ms: avg_ms_llm.map(|llm| (self.reflex as f64 * (llm - avg_ms_reflex.unwrap_or(0.0)).max(0.0)).round() as i64),
        }
    }
}

/// The report over the `hours` up to `now`, from the decisions in it. `only`: the report covers one project.
pub fn report(
    samples: &[Sample],
    projects: &[ProjectRef],
    baselines: &HashMap<String, Baseline>,
    overhead: &BTreeMap<String, u64>,
    only: Option<&str>,
    hours: i64,
    now: DateTime<Utc>,
) -> SavingsReport {
    let hours = hours.clamp(1, MAX_HOURS);
    let since = now - Duration::hours(hours);
    let first = since.date_naive();
    let n_days = (now.date_naive() - first).num_days() + 1;
    let mut days: Vec<(i64, i64, f64, f64)> = vec![(0, 0, 0.0, 0.0); n_days as usize];
    let mut accs: Vec<Acc> = vec![Acc::default(); projects.len()];
    let index: HashMap<&str, usize> = projects.iter().enumerate().map(|(i, p)| (p.slug.as_str(), i)).collect();

    for s in samples {
        if s.at < since || s.at > now {
            continue;
        }
        let Some(&i) = index.get(s.project.as_str()) else { continue };
        let fallback = s.fallback.as_deref().unwrap_or(&projects[i].fallback);
        let extra = overhead.get(fallback).copied().unwrap_or(0) as f64;
        let (saved, spent) = accs[i].add(s, baselines.get(&s.project), extra);
        let day = (s.at.date_naive() - first).num_days();
        if let Some(d) = usize::try_from(day).ok().and_then(|d| days.get_mut(d)) {
            d.0 += i64::from(s.lane == Lane::Reflex);
            d.1 += i64::from(s.lane == Lane::Llm);
            d.2 += saved;
            d.3 += spent;
        }
    }

    let rows: Vec<SavingsRow> = projects.iter().zip(&accs).map(|(p, a)| a.row(&p.slug, &p.name)).collect();
    let mut all = Acc::default();
    accs.iter().for_each(|a| all.merge(a));
    let (slug, name) = match only.and_then(|s| projects.iter().find(|p| p.slug == s)) {
        Some(p) => (p.slug.as_str(), p.name.as_str()),
        None => ("", "All projects"),
    };
    let mut totals = all.row(slug, name);
    // Time is saved per project, against that project's own fallback.
    let times: Vec<i64> = rows.iter().filter_map(|r| r.time_saved_ms).collect();
    totals.time_saved_ms = (!times.is_empty()).then(|| times.iter().sum());

    SavingsReport {
        since,
        hours,
        method_note: method_note(overhead),
        totals,
        projects: rows,
        days: days
            .into_iter()
            .enumerate()
            .map(|(i, (reflex, llm, saved, spent))| SavingsDay {
                day: day_key(first + Duration::days(i as i64)),
                saved: saved.round() as i64,
                spent: spent.round() as i64,
                reflex,
                llm,
            })
            .collect(),
        overhead_tokens: overhead.clone(),
    }
}

pub fn method_note(overhead: &BTreeMap<String, u64>) -> String {
    let mut note = format!(
        "Saved: each Reflex answer is priced at what its prompt would have cost the fallback: with {MIN_MEASURED_CALLS} or more API calls that \
         reported usage in the last {BASELINE_DAYS} days, their mean tokens per call scaled by prompt length; otherwise ceil(prompt chars / \
         {CHARS_PER_TOKEN}) + {ESTIMATED_REPLY_TOKENS}. Spent: the tokens of each llm-lane answer as the API reported them, else ceil(chars / \
         {CHARS_PER_TOKEN}) of prompt and reply. Thai input never reaches Reflex, so it cannot save anything."
    );
    let set: Vec<String> = overhead.iter().filter(|(_, n)| **n > 0).map(|(f, n)| format!("{f} {n}")).collect();
    if set.is_empty() {
        note.push_str(" A headless CLI's own system prompt is not counted (fallback_overhead_tokens is not set).");
    } else {
        note.push_str(&format!(" Added per fallback call (fallback_overhead_tokens): {}.", set.join(", ")));
    }
    note
}

/// The report from the database: `project` one project's (it must exist), `None` every project's.
pub async fn load(hub: &Hub, project: Option<&str>, hours: i64, now: DateTime<Utc>) -> Result<SavingsReport> {
    let hours = hours.clamp(1, MAX_HOURS);
    let projects: Vec<ProjectRef> = hub
        .store
        .projects()
        .await?
        .into_iter()
        .filter(|p| project.is_none_or(|s| s == p.slug))
        .map(|p| ProjectRef { slug: p.slug, name: p.name, fallback: p.fallback })
        .collect();
    let samples = hub.store.savings_samples(project, now - Duration::hours(hours)).await?;
    let baselines = hub
        .store
        .measured_calls(now - Duration::days(BASELINE_DAYS))
        .await?
        .into_iter()
        .filter_map(|(slug, calls, tokens, chars)| Baseline::from_calls(calls, tokens, chars).map(|b| (slug, b)))
        .collect();
    Ok(report(&samples, &projects, &baselines, &hub.config.reflex.fallback_overhead_tokens, project, hours, now))
}

/// A day as the report writes it: `YYYY-MM-DD`.
pub fn day_key(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::QuestionKind;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 14, 12, 0, 0).unwrap()
    }

    fn projects() -> Vec<ProjectRef> {
        vec![
            ProjectRef { slug: "ops".into(), name: "Ops".into(), fallback: "claude".into() },
            ProjectRef { slug: "support".into(), name: "Support".into(), fallback: "api".into() },
        ]
    }

    fn sample(project: &str, lane: Lane, prompt_chars: i64, spent: Option<(i64, bool)>, ms: i64) -> Sample {
        Sample { project: project.into(), at: now() - Duration::hours(1), lane, ms, prompt_chars, spent, fallback: None, skipped_thai: false }
    }

    fn run(samples: &[Sample], baselines: &HashMap<String, Baseline>, overhead: &BTreeMap<String, u64>, only: Option<&str>, hours: i64) -> SavingsReport {
        report(samples, &projects(), baselines, overhead, only, hours, now())
    }

    fn row<'a>(r: &'a SavingsReport, slug: &str) -> &'a SavingsRow {
        r.projects.iter().find(|p| p.slug == slug).unwrap()
    }

    #[test]
    fn tokens_are_estimated_at_four_chars() {
        assert_eq!((estimate_tokens(0), estimate_tokens(1), estimate_tokens(4), estimate_tokens(5), estimate_tokens(-3)), (0, 1, 1, 2, 0));
        assert_eq!(would_cost(100, None), (41.0, false));
    }

    #[test]
    fn estimated_without_measured_calls() {
        let s = [
            sample("ops", Lane::Reflex, 100, None, 4),
            sample("ops", Lane::Reflex, 101, None, 6),
            sample("ops", Lane::Llm, 100, Some((27, false)), 1505),
            sample("ops", Lane::Abstain, 100, None, 3),
        ];
        let r = run(&s, &HashMap::new(), &BTreeMap::new(), None, 24);
        let ops = row(&r, "ops");
        // (25 + 16) + (26 + 16) saved, 27 spent.
        assert_eq!((ops.decisions, ops.reflex, ops.llm, ops.abstain, ops.llm_calls_avoided), (4, 2, 1, 1, 2));
        assert_eq!((ops.tokens_saved, ops.tokens_spent, ops.method), (83, 27, TokenMethod::Estimated));
        assert_eq!(ops.saved_pct, Some(100.0 * 83.0 / 110.0));
        assert_eq!((ops.avg_ms_reflex, ops.avg_ms_llm, ops.time_saved_ms), (Some(5.0), Some(1505.0), Some(3000)));
        // The other project is in the report with nothing.
        assert_eq!(row(&r, "support").decisions, 0);
        assert_eq!((r.totals.slug.as_str(), r.totals.name.as_str(), r.totals.tokens_saved), ("", "All projects", 83));
    }

    #[test]
    fn measured_calls_price_the_saved_ones() {
        // 3 measured calls: 300 tokens over 600 prompt chars → 100 tokens per 200-char call.
        let b = Baseline::from_calls(3, 300, 600).unwrap();
        assert_eq!(Baseline::from_calls(2, 200, 400), None);
        assert_eq!(Baseline::from_calls(3, 300, 0), None);
        assert_eq!(would_cost(400, Some(&b)), (200.0, true));
        let baselines = HashMap::from([("support".to_string(), b)]);
        let s = [sample("support", Lane::Reflex, 400, None, 3), sample("support", Lane::Reflex, 100, None, 3), sample("support", Lane::Llm, 200, Some((95, true)), 900)];
        let r = run(&s, &baselines, &BTreeMap::new(), Some("support"), 24);
        let sup = row(&r, "support");
        assert_eq!((sup.tokens_saved, sup.tokens_spent, sup.method), (250, 95, TokenMethod::Measured));
        assert_eq!((r.totals.slug.as_str(), r.totals.name.as_str()), ("support", "Support"));
    }

    #[test]
    fn method_is_mixed_when_both_kinds_count() {
        let baselines = HashMap::from([("ops".to_string(), Baseline::from_calls(4, 400, 800).unwrap())]);
        // Saved from measured calls, but the CLI answer's tokens are estimated.
        let s = [sample("ops", Lane::Reflex, 200, None, 3), sample("ops", Lane::Llm, 200, Some((60, false)), 900)];
        let r = run(&s, &baselines, &BTreeMap::new(), None, 24);
        assert_eq!(row(&r, "ops").method, TokenMethod::Mixed);
        assert_eq!(r.totals.method, TokenMethod::Mixed);
        // Only abstentions: nothing counted, so nothing measured.
        let r = run(&[sample("ops", Lane::Abstain, 200, None, 3)], &baselines, &BTreeMap::new(), None, 24);
        assert_eq!(row(&r, "ops").method, TokenMethod::Estimated);
    }

    #[test]
    fn overhead_is_added_per_call_of_that_fallback() {
        let overhead = BTreeMap::from([("claude".to_string(), 1000), ("codex".to_string(), 7)]);
        let mut codex = sample("ops", Lane::Llm, 100, Some((30, false)), 900);
        codex.fallback = Some("codex".into());
        // `ops` falls back to claude now; the codex row recorded its own fallback.
        let s = [sample("ops", Lane::Reflex, 100, None, 3), sample("ops", Lane::Llm, 100, Some((30, false)), 900), codex, sample("support", Lane::Reflex, 100, None, 3)];
        let r = run(&s, &HashMap::new(), &overhead, None, 24);
        let ops = row(&r, "ops");
        assert_eq!((ops.tokens_saved, ops.tokens_spent), (41 + 1000, 30 + 1000 + 30 + 7));
        // support's fallback is the API: no overhead set for it.
        assert_eq!(row(&r, "support").tokens_saved, 41);
        assert!(r.method_note.contains("claude 1000, codex 7"));
        assert_eq!(r.overhead_tokens["claude"], 1000);
        assert!(run(&[], &HashMap::new(), &BTreeMap::new(), None, 24).method_note.contains("not set"));
    }

    #[test]
    fn empty_report() {
        let r = run(&[], &HashMap::new(), &BTreeMap::new(), None, 24);
        let t = &r.totals;
        assert_eq!((t.decisions, t.tokens_saved, t.tokens_spent, t.saved_pct, t.method), (0, 0, 0, None, TokenMethod::Estimated));
        assert_eq!((t.avg_ms_reflex, t.avg_ms_llm, t.time_saved_ms), (None, None, None));
        assert_eq!(r.projects.len(), 2);
        assert_eq!(r.since, now() - Duration::hours(24));
        assert_eq!(r.days.iter().map(|d| d.day.as_str()).collect::<Vec<_>>(), ["2026-10-13", "2026-10-14"]);
        // No projects at all.
        let r = report(&[], &[], &HashMap::new(), &BTreeMap::new(), None, 24, now());
        assert!(r.projects.is_empty());
        assert_eq!(r.totals.decisions, 0);
    }

    #[test]
    fn rows_from_before_the_migration_are_estimated_from_state() {
        let q = Question { id: "urgent".into(), kind: QuestionKind::Noul, prompt: "Urgent?".into(), options: None, criteria: None };
        let at = now() - Duration::hours(2);
        let reflex = Sample::legacy("ops".into(), at, Lane::Reflex, 4, "db-2 is down", &q, &Value::Bool(true));
        let chars = decide::prompt("db-2 is down", &q).chars().count() as i64;
        assert_eq!((reflex.prompt_chars, reflex.spent, reflex.fallback.clone(), reflex.skipped_thai), (chars, None, None, false));
        let llm = Sample::legacy("ops".into(), at, Lane::Llm, 900, "ฐานข้อมูล db-2 ตอบช้ามาก", &q, &Value::Bool(false));
        let chars_thai = decide::prompt("ฐานข้อมูล db-2 ตอบช้ามาก", &q).chars().count() as i64;
        assert_eq!(llm.spent, Some((estimate_tokens(chars_thai) + 1, false)));
        assert!(llm.skipped_thai);

        let r = run(&[reflex, llm], &HashMap::new(), &BTreeMap::new(), None, 24);
        let ops = row(&r, "ops");
        assert_eq!(ops.tokens_saved, estimate_tokens(chars) + ESTIMATED_REPLY_TOKENS);
        assert_eq!((ops.tokens_spent, ops.skipped_thai), (estimate_tokens(chars_thai) + 1, 1));
    }

    #[test]
    fn recorded_rows_read_their_cost() {
        let cost = DecisionCost { prompt_chars: Some(120), tokens_in: Some(40), tokens_out: Some(2), measured: true, fallback: Some("api".into()), skipped_thai: false };
        let s = Sample::recorded("support".into(), now(), Lane::Llm, 800, &cost);
        assert_eq!((s.prompt_chars, s.spent, s.fallback.as_deref()), (120, Some((42, true)), Some("api")));
        // A reflex answer: nothing spent, only the prompt length.
        let s = Sample::recorded("support".into(), now(), Lane::Reflex, 3, &DecisionCost { tokens_in: None, tokens_out: None, ..cost.clone() });
        assert_eq!(s.spent, None);
    }

    #[test]
    fn thai_decisions_are_counted() {
        let mut thai = sample("ops", Lane::Llm, 90, Some((25, false)), 1000);
        thai.skipped_thai = true;
        let s = [thai.clone(), thai, sample("ops", Lane::Reflex, 90, None, 2)];
        let r = run(&s, &HashMap::new(), &BTreeMap::new(), None, 24);
        assert_eq!((row(&r, "ops").skipped_thai, r.totals.skipped_thai), (2, 2));
    }

    #[test]
    fn days_are_utc_buckets_of_the_window() {
        let at = |d: u32, h: u32, m: u32| Utc.with_ymd_and_hms(2026, 10, d, h, m, 0).unwrap();
        let mk = |t: DateTime<Utc>, lane: Lane| Sample { at: t, ..sample("ops", lane, 100, Some((30, false)), 5) };
        let s = [
            mk(at(13, 23, 30), Lane::Reflex),
            mk(at(14, 0, 30), Lane::Reflex),
            mk(at(14, 0, 45), Lane::Llm),
            mk(at(7, 11, 0), Lane::Reflex), // before the 7-day window (since 10-07 12:00)
            mk(at(7, 12, 30), Lane::Llm),
        ];
        let r = run(&s, &HashMap::new(), &BTreeMap::new(), None, 168);
        assert_eq!(r.days.len(), 8);
        assert_eq!(r.days[0].day, day_key(NaiveDate::from_ymd_opt(2026, 10, 7).unwrap()));
        assert_eq!((r.days[0].reflex, r.days[0].llm, r.days[0].spent), (0, 1, 30));
        let d13 = &r.days[6];
        assert_eq!((d13.day.as_str(), d13.reflex, d13.saved, d13.llm), ("2026-10-13", 1, 41, 0));
        let d14 = &r.days[7];
        assert_eq!((d14.day.as_str(), d14.reflex, d14.llm, d14.saved, d14.spent), ("2026-10-14", 1, 1, 41, 30));
        assert_eq!(r.totals.decisions, 4);
        // The last 24 h (since 10-13 12:00) still hold the 13th at 23:30.
        let r = run(&s, &HashMap::new(), &BTreeMap::new(), None, 24);
        assert_eq!(r.totals.decisions, 3);
        // Windows are clamped to 30 days.
        assert_eq!(run(&[], &HashMap::new(), &BTreeMap::new(), None, 10_000).hours, MAX_HOURS);
    }

    #[test]
    fn time_saved_needs_llm_samples_and_never_goes_negative() {
        let r = run(&[sample("ops", Lane::Reflex, 100, None, 4)], &HashMap::new(), &BTreeMap::new(), None, 24);
        assert_eq!(row(&r, "ops").time_saved_ms, None);
        let s = [sample("ops", Lane::Reflex, 100, None, 50), sample("ops", Lane::Llm, 100, Some((30, false)), 20)];
        assert_eq!(row(&run(&s, &HashMap::new(), &BTreeMap::new(), None, 24), "ops").time_saved_ms, Some(0));
        // Totals add each project's own figure.
        let s = [
            sample("ops", Lane::Reflex, 100, None, 0),
            sample("ops", Lane::Llm, 100, Some((30, false)), 1000),
            sample("support", Lane::Reflex, 100, None, 0),
            sample("support", Lane::Reflex, 100, None, 0),
            sample("support", Lane::Llm, 100, Some((30, true)), 100),
        ];
        let r = run(&s, &HashMap::new(), &BTreeMap::new(), None, 24);
        assert_eq!(r.totals.time_saved_ms, Some(1000 + 2 * 100));
    }
}
