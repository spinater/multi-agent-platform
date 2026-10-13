//! Reading a captured tmux pane: rate limits and their reset time, auth prompts, pending human input, errors.
//! Ported from the Anti-Gravity watcher (`detector.py`) and the orchestrator gates (`orchestrator-gates.ts`).

use std::sync::LazyLock;

use chrono::{DateTime, Duration, Local, NaiveDateTime, TimeZone, Utc};
use regex::Regex;

use crate::config::ProviderSpec;

static RATE_LIMIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        resource_exhausted
        | \b(?:http|status|error|code)[\s:=]*429\b
        | \b429\b[^\n]{0,40}(?:too\s+many|rate|quota|exhausted)
        | rate[\s_-]*limits?\s+(?:reached|exceeded|hit)
        | quota\s+(?:exceeded|reached|exhausted)
        | too\s+many\s+requests
        | exhausted\s+your\s+quota
        | quota\s+has\s+been\s+exhausted
        | (?:usage|hour|weekly|session)\s+limit\s+reached
        | you'?ve\s+hit\s+your\s+(?:usage\s+)?limit
        | (?:try\s+again\s+in|retry\s+after|resets?\s+in)\s+\d+\s*(?:s|sec|seconds?|m|min|minutes?|h|hr|hours?)\b
        | quota\s+resets?\s+at
        ",
    )
    .unwrap()
});

/// Bare `rate limit`, unless it is a setting (`rate limit: 50`) or part of a longer word (`rate_limiter`).
static BARE_RATE_LIMIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)rate[\s_-]*limit").unwrap());
static SETTING_AFTER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[:=]\s*\d").unwrap());

static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;:?]*[A-Za-z]|\x1b[^\[]").unwrap());
static SGR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[([0-9;:]*)([A-Za-z])|\x1b[^\[]").unwrap());

static ERROR_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:FATAL|Traceback \(most recent call last\):|panic(?:ked)?:|failed with exit code \d+|command failed with code \d+|Cannot find module|ECONNREFUSED|UnhandledPromiseRejection)[^\n]*",
    )
    .unwrap()
});

pub fn strip_ansi(text: &str) -> String {
    ANSI.replace_all(text, "").into_owned()
}

fn bare_rate_limit(line: &str) -> bool {
    BARE_RATE_LIMIT.find_iter(line).any(|m| {
        let rest = &line[m.end()..];
        let next_is_word = rest.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_');
        !next_is_word && !SETTING_AFTER.is_match(rest)
    })
}

/// The bottom-most line of `text` that reports a rate limit or quota exhaustion.
pub fn rate_limit_line(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .find(|line| RATE_LIMIT.is_match(line) || bare_rate_limit(line))
        .map(|line| line.trim().to_string())
}

static RELATIVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:try\s+again\s+in|retry\s+after|resets?\s+in|wait)\s+((?:\d+(?:\.\d+)?\s*(?:hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)\b[\s,]*(?:and\s+)?)+)",
    )
    .unwrap()
});
static DURATION_PART: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(\d+(?:\.\d+)?)\s*(hours?|hrs?|h|minutes?|mins?|m|seconds?|secs?|s)\b").unwrap());
static RETRY_INFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(?:retry_?delay"?\s*[:=]\s*\{?\s*(?:seconds\s*[:=]\s*)?"?|\bseconds\s*[:=]\s*)(\d+)"#).unwrap()
});
static ABSOLUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:resets?|try\s+again|available\s+(?:again\s+)?)\s+(?:at\s+)?(\d{1,2})(?::(\d{2}))?(?::(\d{2}))?\s*(am|pm)?\b",
    )
    .unwrap()
});
static ISO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:\.\d+)?(Z|[+-]\d{2}:?\d{2})?)").unwrap());

/// When the limit lifts, read from the message: a relative delay (`try again in 1h 5m`), a Google `RetryInfo`
/// delay, a wall-clock time (`resets 3pm`, `try again at 02:30`, taken as local time, rolled to tomorrow when
/// already past) or an ISO-8601 timestamp. `None` when the message carries no time.
pub fn parse_reset_time(text: &str, now: DateTime<Local>) -> Option<DateTime<Utc>> {
    if let Some(caps) = RELATIVE.captures(text) {
        let mut secs = 0.0;
        for part in DURATION_PART.captures_iter(&caps[1]) {
            let value: f64 = part[1].parse().unwrap_or(0.0);
            let unit = part[2].to_ascii_lowercase();
            secs += match unit.chars().next() {
                Some('h') => value * 3600.0,
                Some('m') => value * 60.0,
                _ => value,
            };
        }
        return Some((now + Duration::milliseconds((secs * 1000.0) as i64)).with_timezone(&Utc));
    }

    if let Some(caps) = RETRY_INFO.captures(text) {
        let secs: i64 = caps[1].parse().ok()?;
        return Some((now + Duration::seconds(secs)).with_timezone(&Utc));
    }

    for caps in ABSOLUTE.captures_iter(text) {
        let minutes = caps.get(2);
        let meridiem = caps.get(4).map(|m| m.as_str().to_ascii_lowercase());
        // "resets 3" alone is too vague; need minutes or am/pm.
        if minutes.is_none() && meridiem.is_none() {
            continue;
        }
        let mut hour: u32 = caps[1].parse().ok()?;
        let minute: u32 = minutes.map_or(Some(0), |m| m.as_str().parse().ok())?;
        let second: u32 = caps.get(3).map_or(Some(0), |m| m.as_str().parse().ok())?;
        match meridiem.as_deref() {
            Some("pm") if hour < 12 => hour += 12,
            Some("am") if hour == 12 => hour = 0,
            _ => {}
        }
        let date = now.date_naive();
        let naive = date.and_hms_opt(hour, minute, second)?;
        let mut target = Local.from_local_datetime(&naive).earliest()?;
        if target <= now {
            target += Duration::days(1);
        }
        return Some(target.with_timezone(&Utc));
    }

    if let Some(caps) = ISO.captures(text) {
        let raw = caps[1].replace(' ', "T");
        let parsed = if caps.get(2).is_some() {
            DateTime::parse_from_rfc3339(&raw.replace("Z", "+00:00")).ok().map(|d| d.with_timezone(&Utc))
        } else {
            NaiveDateTime::parse_from_str(&raw, "%Y-%m-%dT%H:%M:%S%.f")
                .ok()
                .and_then(|n| Local.from_local_datetime(&n).earliest())
                .map(|d| d.with_timezone(&Utc))
        };
        if let Some(at) = parsed.filter(|at| *at > now.with_timezone(&Utc)) {
            return Some(at);
        }
    }
    None
}

/// Visible characters of a line captured with `tmux capture-pane -e`, each flagged when it is real typing and not
/// faint (SGR 2), bright black (90) or reverse video (7, the cursor block). Claude Code draws its suggestion faint.
fn styled_chars(raw: &str) -> (Vec<char>, Vec<bool>) {
    let (mut faint, mut reverse) = (false, false);
    let mut text = Vec::new();
    let mut typed = Vec::new();
    let mut at = 0;
    for m in SGR.captures_iter(raw) {
        let whole = m.get(0).unwrap();
        for c in raw[at..whole.start()].chars() {
            text.push(c);
            typed.push(!(faint || reverse));
        }
        at = whole.end();
        if m.get(2).map(|g| g.as_str()) != Some("m") {
            continue;
        }
        let params = m.get(1).map_or("", |g| g.as_str());
        let codes: Vec<u32> = if params.is_empty() { vec![0] } else { params.split([';', ':']).map(|c| c.parse().unwrap_or(0)).collect() };
        for code in codes {
            match code {
                0 => {
                    faint = false;
                    reverse = false;
                }
                2 | 90 => faint = true,
                22 | 39 => faint = false,
                7 => reverse = true,
                27 => reverse = false,
                _ => {}
            }
        }
    }
    for c in raw[at..].chars() {
        text.push(c);
        typed.push(!(faint || reverse));
    }
    (text, typed)
}

static INPUT_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([│|\s]*[>❯][ \t]?)(.*?)[│|\s]*$").unwrap());

/// Text a person has typed on the input line (after the `>` or `❯` marker) of a pane captured with escapes;
/// empty when nobody is typing. Greyed suggestions and the cursor block do not count.
pub fn pending_input(pane: &str) -> String {
    let lines: Vec<&str> = pane.split('\n').collect();
    for raw in lines.iter().rev().take(15) {
        let (chars, typed) = styled_chars(raw);
        let text: String = chars.iter().collect();
        let Some(caps) = INPUT_LINE.captures(&text) else { continue };
        let from = caps[1].chars().count();
        let to = from + caps[2].chars().count();
        let out: String = (from..to).filter(|&k| typed[k]).map(|k| chars[k]).collect();
        return out.trim().to_string();
    }
    String::new()
}

/// Ending a runner is allowed only when nobody is attached and nobody has unsent text on the input line.
#[derive(Debug, PartialEq, Eq)]
pub enum EndDecision {
    End,
    KeepAttached,
    KeepInput,
}

pub fn end_decision(clients: usize, pane: &str) -> EndDecision {
    if !pending_input(pane).is_empty() {
        EndDecision::KeepInput
    } else if clients > 0 {
        EndDecision::KeepAttached
    } else {
        EndDecision::End
    }
}

/// The agent's last message in a plain pane: from the last `●`/`⏺` line to the input box, else the last 30 lines.
pub fn last_assistant_message(pane: &str) -> String {
    let lines: Vec<&str> = pane.lines().collect();
    let Some(start) = lines.iter().rposition(|l| l.trim_start().starts_with(['●', '⏺'])) else {
        let kept: Vec<&str> = lines.iter().copied().filter(|l| !l.trim().is_empty()).collect();
        return kept[kept.len().saturating_sub(30)..].join("\n");
    };
    let mut out = Vec::new();
    for line in &lines[start..] {
        let boxed = line.starts_with(['╭', '╰', '─', '━', '│']) || line.trim_start().starts_with(['>', '❯']);
        if !out.is_empty() && boxed {
            break;
        }
        out.push(*line);
    }
    out.join("\n").trim().to_string()
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct PaneSignals {
    /// The bottom-most rate-limit line and the reset time parsed from it.
    pub rate_limit: Option<(String, Option<DateTime<Utc>>)>,
    /// How many rate-limit lines the scanned tail holds; a rise means a new hit even if the text repeats.
    pub rate_limit_count: usize,
    pub needs_auth: Option<String>,
    pub error: Option<String>,
    pub pending_input: bool,
    pub idle_marker: bool,
    pub working_marker: bool,
    pub last_line: Option<String>,
}

/// Everything the watcher needs from one capture (taken with `-e`). Only the bottom `scan_lines` lines are read.
pub fn scan(spec: &ProviderSpec, raw_pane: &str, scan_lines: usize, now: DateTime<Local>) -> PaneSignals {
    let plain = strip_ansi(raw_pane);
    let lines: Vec<&str> = plain.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(scan_lines)..].join("\n");
    let bottom = lines[lines.len().saturating_sub(8)..].join("\n");

    let rate_limit_count = tail.lines().filter(|l| rate_limit_line(l).is_some()).count();
    let rate_limit = rate_limit_line(&tail).map(|line| {
        let reset = parse_reset_time(&line, now);
        (line, reset)
    });
    let needs_auth = spec
        .auth_markers
        .iter()
        .find(|m| tail.contains(m.as_str()))
        .map(|m| m.to_string());
    PaneSignals {
        rate_limit,
        rate_limit_count,
        needs_auth,
        error: ERROR_LINE.find(&bottom).map(|m| m.as_str().trim().chars().take(180).collect()),
        pending_input: !pending_input(raw_pane).is_empty(),
        idle_marker: spec.idle_markers.iter().any(|m| bottom.contains(m.as_str())),
        working_marker: spec.working_markers.iter().any(|m| bottom.contains(m.as_str())),
        last_line: lines.last().map(|l| l.trim().chars().take(200).collect()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn at(h: u32, m: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 13, h, m, 0).unwrap()
    }

    #[test]
    fn detects_provider_rate_limit_messages() {
        for text in [
            "Error: RESOURCE_EXHAUSTED: quota",
            "HTTP 429 Too Many Requests",
            "5-hour limit reached ∙ resets 3pm",
            "You've hit your usage limit. Try again in 20 minutes",
            "Rate limit reached for gpt-6-sol",
            "API error: rate limit",
        ] {
            assert!(rate_limit_line(text).is_some(), "missed: {text}");
        }
    }

    #[test]
    fn ignores_settings_and_unrelated_numbers() {
        for text in ["rate_limiter.rs compiled", "rate limit: 50 per minute", "src/app.ts line 429: ok", "ratelimiting middleware"] {
            assert!(rate_limit_line(text).is_none(), "false positive: {text}");
        }
    }

    #[test]
    fn relative_minutes_seconds_hours_and_compound() {
        let now = at(10, 0);
        let secs = |t: &str| (parse_reset_time(t, now).unwrap() - now.with_timezone(&Utc)).num_seconds();
        assert_eq!(secs("Please try again in 15 minutes"), 900);
        assert_eq!(secs("retry after 30s"), 30);
        assert_eq!(secs("quota resets in 2 hours"), 7200);
        assert_eq!(secs("try again in 1h 30m"), 5400);
    }

    #[test]
    fn google_retry_info() {
        let now = at(10, 0);
        let t = parse_reset_time(r#"{"@type": "RetryInfo", "retryDelay": "36s"}"#, now).unwrap();
        assert_eq!((t - now.with_timezone(&Utc)).num_seconds(), 36);
        let t = parse_reset_time("retry_delay { seconds: 120 }", now).unwrap();
        assert_eq!((t - now.with_timezone(&Utc)).num_seconds(), 120);
    }

    #[test]
    fn absolute_time_rolls_to_tomorrow_when_past() {
        let t = parse_reset_time("Quota resets at 02:30 PM", at(10, 0)).unwrap().with_timezone(&Local);
        assert_eq!((t.hour(), t.minute(), t.date_naive()), (14, 30, at(10, 0).date_naive()));
        let t = parse_reset_time("limit reached ∙ resets 3pm (Asia/Bangkok)", at(16, 0)).unwrap().with_timezone(&Local);
        assert_eq!(t.hour(), 15);
        assert_eq!(t.date_naive(), at(16, 0).date_naive().succ_opt().unwrap());
    }

    #[test]
    fn iso_timestamp_in_the_future_only() {
        let now = at(10, 0);
        assert!(parse_reset_time("resetAt 2099-01-01T00:00:00Z", now).is_some());
        assert!(parse_reset_time("resetAt 2001-01-01T00:00:00Z", now).is_none());
    }

    #[test]
    fn no_time_in_message() {
        assert!(parse_reset_time("Too many requests", at(10, 0)).is_none());
    }

    #[test]
    fn pending_input_ignores_faint_suggestion_and_cursor() {
        assert_eq!(pending_input("╭──╮\n│ > \x1b[2mTry \"fix lint\"\x1b[0m │\n╰──╯"), "");
        assert_eq!(pending_input("│ > \x1b[7m \x1b[0m │"), "");
        assert_eq!(pending_input("│ > deploy now\x1b[7m \x1b[0m │"), "deploy now");
        assert_eq!(pending_input("❯ hello"), "hello");
    }

    #[test]
    fn end_decision_protects_humans() {
        assert_eq!(end_decision(0, "❯ "), EndDecision::End);
        assert_eq!(end_decision(1, "❯ "), EndDecision::KeepAttached);
        assert_eq!(end_decision(0, "❯ wait"), EndDecision::KeepInput);
    }

    #[test]
    fn last_message_runs_to_input_box() {
        let pane = "● first\nold\n● Done: all tests pass\nsecond line\n╭────\n│ >\n";
        assert_eq!(last_assistant_message(pane), "● Done: all tests pass\nsecond line");
    }

    #[test]
    fn scan_only_reads_the_bottom_of_the_pane() {
        let spec = crate::config::builtin_providers().remove("claude").unwrap();
        let mut pane = String::from("HTTP 429 Too Many Requests\n");
        for i in 0..50 {
            pane.push_str(&format!("line {i}\n"));
        }
        pane.push_str("? for shortcuts\n");
        let s = scan(&spec, &pane, 40, at(10, 0));
        assert!(s.rate_limit.is_none());
        assert!(s.idle_marker);
        assert_eq!(s.last_line.as_deref(), Some("? for shortcuts"));
    }
}
