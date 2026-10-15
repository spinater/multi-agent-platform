//! Plan usage per provider account: the limits each CLI's own usage command shows, tokens per day and per project
//! from the CLIs' session logs, a pace projection from stored readings, and the skip line the orchestrator obeys.
//!
//! - **Claude**: `GET api.anthropic.com/api/oauth/usage` with Claude Code's own sign-in (what `/usage` reads). The
//!   token is read from the credentials file for that one request and goes nowhere else: never logged, stored or
//!   returned.
//! - **Codex**: the newest `rate_limits` snapshot in `~/.codex/sessions/**/rollout-*.jsonl`, written on Codex's last
//!   run. It is reported as a snapshot with its age.
//! - **OpenRouter**: `GET openrouter.ai/api/v1/key` with the owner's `OPENROUTER_API_KEY`: the key's credit limit
//!   and the free models' daily cap. The key goes to openrouter.ai only, for that request.
//! - Anything else (`agy`, Cursor, Copilot, custom CLIs): no local source exists, so the report says so instead of
//!   guessing.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::Result;
use chrono::{DateTime, Local, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use sqlx::Row;

use crate::config::Config;
use crate::hub::Hub;
use crate::model::{Level, ProjectShare, ProviderUsage, ShareKind, TokenTotals, UsageReport, UsageStatus, UsageWindow};
use crate::store::Store;

const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
const OPENROUTER_KEY_URL: &str = "https://openrouter.ai/api/v1/key";
/// "Read now" within this long of the last read returns that read instead of asking the providers again.
const DEBOUNCE: Duration = Duration::from_secs(10);
/// Pace needs readings at least this far apart.
const MIN_PACE_SPAN_MINUTES: i64 = 15;
/// Readings older than this are deleted.
const KEEP_SAMPLES_DAYS: i64 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reader {
    Claude,
    Codex,
    /// The OpenRouter API key's own limit.
    OpenRouter,
    None,
}

/// The reader for `provider`, or `None` when its reader is turned off. A provider not named `claude`/`codex`/`openrouter`
/// gets one when its binary is, or when `[usage.providers.<name>] reader` says so.
pub fn reader_for(config: &Config, provider: &str) -> Option<Reader> {
    let settings = config.usage.providers.get(provider);
    if settings.is_some_and(|p| !p.enabled) {
        return None;
    }
    let by = |s: &str| match s {
        "claude" => Reader::Claude,
        "codex" => Reader::Codex,
        "openrouter" => Reader::OpenRouter,
        _ => Reader::None,
    };
    if let Some(r) = settings.and_then(|p| p.reader.as_deref()) {
        return Some(by(r));
    }
    let named = by(provider);
    if named != Reader::None {
        return Some(named);
    }
    let binary = config.providers.get(provider).map(|s| s.binary.as_str()).unwrap_or("");
    Some(by(Path::new(binary).file_name().and_then(|n| n.to_str()).unwrap_or("")))
}

/// Configured providers, `provider_order` first.
pub(crate) fn ordered_providers(config: &Config) -> Vec<String> {
    let mut out: Vec<String> = config.orchestrator.provider_order.iter().filter(|p| config.providers.contains_key(*p)).cloned().collect();
    for name in config.providers.keys() {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

fn source_of(reader: Option<Reader>) -> &'static str {
    match reader {
        Some(Reader::Claude) => "/usage endpoint",
        Some(Reader::Codex) => "codex session logs",
        Some(Reader::OpenRouter) => "openrouter.ai key endpoint",
        Some(Reader::None) => "no local reader",
        None => "reader off",
    }
}

fn blank(provider: &str, reader: Option<Reader>, status: UsageStatus) -> ProviderUsage {
    ProviderUsage {
        provider: provider.to_string(),
        plan: None,
        source: source_of(reader).into(),
        status,
        error: None,
        read_at: None,
        windows: vec![],
        tokens_today: None,
        by_project: vec![],
        by_project_since: None,
        skipped: false,
    }
}

/// The latest report, the lock that serializes reads, and the per-file cache of parsed session logs.
pub struct UsageState {
    report: Mutex<UsageReport>,
    refresh: tokio::sync::Mutex<()>,
    cache: Arc<Mutex<ScanCache>>,
}

impl UsageState {
    pub fn new(config: &Config) -> Self {
        let providers = ordered_providers(config)
            .iter()
            .map(|name| {
                let reader = reader_for(config, name);
                let status = match reader {
                    None => UsageStatus::Disabled,
                    Some(Reader::None) => UsageStatus::NoReader,
                    Some(_) => UsageStatus::Pending,
                };
                blank(name, reader, status)
            })
            .collect();
        let report = UsageReport { read_at: None, skip_pct: config.usage.skip_pct, interval_secs: config.usage.interval_secs, providers };
        Self { report: Mutex::new(report), refresh: tokio::sync::Mutex::new(()), cache: Arc::default() }
    }

    pub fn report(&self) -> UsageReport {
        self.report.lock().unwrap().clone()
    }

    pub fn set_report(&self, report: UsageReport) {
        *self.report.lock().unwrap() = report;
    }

    /// Until when the orchestrator should give `provider` no new tasks because of its plan usage.
    pub fn skip_until(&self, provider: &str, config: &Config) -> Option<DateTime<Utc>> {
        if !config.usage.enabled {
            return None;
        }
        let report = self.report.lock().unwrap();
        let p = report.providers.iter().find(|p| p.provider == provider)?;
        skip_until(&p.windows, config.usage.skip_pct, Utc::now(), Duration::from_secs(config.usage.interval_secs))
    }
}

/// The latest reset among windows at or above the skip line. A window that already reset no longer counts; one
/// without a reset time counts until the next reading (`recheck`).
pub fn skip_until(windows: &[UsageWindow], skip_pct: f64, now: DateTime<Utc>, recheck: Duration) -> Option<DateTime<Utc>> {
    windows
        .iter()
        .filter(|w| w.used_pct.is_some_and(|u| u >= skip_pct))
        .filter_map(|w| match w.resets_at {
            Some(r) if r > now => Some(r),
            Some(_) => None,
            None => Some(now + chrono::Duration::from_std(recheck).unwrap_or_default()),
        })
        .max()
}

/// Polls every `usage.interval_secs`.
pub async fn run(hub: Arc<Hub>) {
    if !hub.config.usage.enabled {
        return;
    }
    let interval = Duration::from_secs(hub.config.usage.interval_secs);
    loop {
        refresh(&hub).await;
        tokio::time::sleep(interval).await;
    }
}

/// Reads every provider now, stores the readings, updates the cached report and pushes it to the dashboard.
pub async fn refresh(hub: &Arc<Hub>) -> UsageReport {
    let _one_at_a_time = hub.usage.refresh.lock().await;
    let prev = hub.usage.report();
    let now = Utc::now();
    if let Some(at) = prev.read_at
        && (now - at).to_std().unwrap_or_default() < DEBOUNCE
    {
        return prev;
    }
    let places = Places::load(hub).await;
    let cfg = &hub.config.usage;
    let mut providers = vec![];
    for name in ordered_providers(&hub.config) {
        let reader = reader_for(&hub.config, &name);
        let last = prev.providers.iter().find(|p| p.provider == name);
        let mut p = match reader {
            None => blank(&name, reader, UsageStatus::Disabled),
            Some(Reader::None) => blank(&name, reader, UsageStatus::NoReader),
            Some(Reader::Claude) => read_claude(hub, &name, last, &places, now).await,
            Some(Reader::Codex) => read_codex(hub, &name, last, &places, now).await,
            Some(Reader::OpenRouter) => read_openrouter(hub, &name, last, now).await,
        };
        apply_pace(&hub.store, &mut p, now).await;
        p.skipped = skip_until(&p.windows, cfg.skip_pct, now, Duration::from_secs(cfg.interval_secs)).is_some();
        announce_skip_change(hub, last, &p, cfg.skip_pct).await;
        providers.push(p);
    }
    if let Err(err) = hub.store.prune_usage_samples(now - chrono::Duration::days(KEEP_SAMPLES_DAYS)).await {
        tracing::warn!("pruning usage samples: {err:#}");
    }
    let report = UsageReport { read_at: Some(now), skip_pct: cfg.skip_pct, interval_secs: cfg.interval_secs, providers };
    hub.usage.set_report(report.clone());
    hub.announce_usage(report.clone());
    report
}

async fn announce_skip_change(hub: &Hub, last: Option<&ProviderUsage>, p: &ProviderUsage, skip_pct: f64) {
    let was = last.is_some_and(|l| l.skipped);
    if p.skipped == was {
        return;
    }
    if p.skipped {
        let top = p.windows.iter().filter(|w| w.used_pct.is_some_and(|u| u >= skip_pct)).max_by(|a, b| a.used_pct.partial_cmp(&b.used_pct).unwrap());
        let what = top.map_or(String::new(), |w| {
            let until = w.resets_at.map_or(String::new(), |r| format!(" until {}", r.with_timezone(&Local).format("%a %H:%M")));
            format!(": {} at {:.0}%, no new tasks{until}", w.label, w.used_pct.unwrap_or_default())
        });
        hub.event(Level::Warning, "usage.skip", None, None, format!("{} passed the {skip_pct:.0}% skip line{what}", p.provider)).await;
    } else {
        hub.event(Level::Info, "usage.resume", None, None, format!("{} is below the {skip_pct:.0}% skip line again", p.provider)).await;
    }
}

// ---- pace

/// Percentage points per hour: the least-squares slope of readings of one window instance spanning at least
/// [`MIN_PACE_SPAN_MINUTES`]. A fit over every reading, not just the ends, smooths the whole-percent steps the
/// providers report. Never negative.
pub fn pace(samples: &[(DateTime<Utc>, f64)]) -> Option<f64> {
    let first = samples.iter().map(|s| s.0).min()?;
    let last = samples.iter().map(|s| s.0).max()?;
    if last - first < chrono::Duration::minutes(MIN_PACE_SPAN_MINUTES) {
        return None;
    }
    let hours: Vec<f64> = samples.iter().map(|s| (s.0 - first).num_milliseconds() as f64 / 3_600_000.0).collect();
    let n = samples.len() as f64;
    let (mt, mu) = (hours.iter().sum::<f64>() / n, samples.iter().map(|s| s.1).sum::<f64>() / n);
    let cov: f64 = hours.iter().zip(samples).map(|(t, s)| (t - mt) * (s.1 - mu)).sum();
    let var: f64 = hours.iter().map(|t| (t - mt).powi(2)).sum();
    Some((cov / var).max(0.0))
}

/// Estimated use at `resets_at` if `rate_per_hour` continues.
pub fn projected(used_pct: f64, rate_per_hour: f64, resets_at: DateTime<Utc>, now: DateTime<Utc>) -> f64 {
    let hours = ((resets_at - now).num_seconds().max(0) as f64) / 3600.0;
    used_pct + rate_per_hour * hours
}

/// Fills `rate_per_hour` and `projected_pct` from the last hour of stored readings of the same window instance.
async fn apply_pace(store: &Store, p: &mut ProviderUsage, now: DateTime<Utc>) {
    for w in &mut p.windows {
        let (Some(used), Some(resets_at)) = (w.used_pct, w.resets_at) else { continue };
        let samples = match store.usage_samples(&p.provider, &w.id, now - chrono::Duration::hours(1)).await {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!("usage samples for {}: {err:#}", p.provider);
                continue;
            }
        };
        let same: Vec<(DateTime<Utc>, f64)> =
            samples.into_iter().filter(|(_, _, r)| r.is_some_and(|r| (r - resets_at).num_minutes().abs() <= 5)).map(|(at, u, _)| (at, u)).collect();
        if let Some(rate) = pace(&same) {
            w.rate_per_hour = Some(rate);
            w.projected_pct = Some(projected(used, rate, resets_at, now));
        }
    }
}

impl Store {
    pub async fn insert_usage_sample(&self, provider: &str, window: &str, at: DateTime<Utc>, used_pct: f64, resets_at: Option<DateTime<Utc>>) -> Result<()> {
        sqlx::query("INSERT OR IGNORE INTO usage_samples (provider, window_id, at_ms, used_pct, resets_at_ms) VALUES (?, ?, ?, ?, ?)")
            .bind(provider)
            .bind(window)
            .bind(at.timestamp_millis())
            .bind(used_pct)
            .bind(resets_at.map(|r| r.timestamp_millis()))
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Readings of one window since `since`, oldest first: (at, used %, resets at).
    pub async fn usage_samples(&self, provider: &str, window: &str, since: DateTime<Utc>) -> Result<Vec<(DateTime<Utc>, f64, Option<DateTime<Utc>>)>> {
        let rows = sqlx::query("SELECT at_ms, used_pct, resets_at_ms FROM usage_samples WHERE provider = ? AND window_id = ? AND at_ms >= ? ORDER BY at_ms")
            .bind(provider)
            .bind(window)
            .bind(since.timestamp_millis())
            .fetch_all(&self.pool)
            .await?;
        let ms = |v: i64| Utc.timestamp_millis_opt(v).single();
        Ok(rows.iter().filter_map(|r| Some((ms(r.get("at_ms"))?, r.get("used_pct"), r.get::<Option<i64>, _>("resets_at_ms").and_then(ms)))).collect())
    }

    pub async fn prune_usage_samples(&self, before: DateTime<Utc>) -> Result<()> {
        sqlx::query("DELETE FROM usage_samples WHERE at_ms < ?").bind(before.timestamp_millis()).execute(&self.pool).await?;
        Ok(())
    }
}

// ---- where tokens were spent

struct Place {
    kind: ShareKind,
    slug: Option<String>,
    name: String,
    dirs: Vec<PathBuf>,
}

/// MAP projects (everything under their data directory: workspace, fallback runs) and `workspace.dir`.
pub struct Places(Vec<Place>);

impl Places {
    async fn load(hub: &Hub) -> Self {
        let both = |p: PathBuf| {
            let canon = std::fs::canonicalize(&p).ok().filter(|c| *c != p);
            std::iter::once(p).chain(canon).collect::<Vec<_>>()
        };
        let mut places = vec![];
        for p in hub.store.projects().await.unwrap_or_default() {
            places.push(Place { kind: ShareKind::Project, dirs: both(hub.project_dir(&p.slug)), slug: Some(p.slug), name: p.name });
        }
        let ws = hub.config.workspace.dir.clone();
        let name = ws.file_name().and_then(|n| n.to_str()).unwrap_or("workspace").to_string();
        places.push(Place { kind: ShareKind::Workspace, slug: None, name, dirs: both(ws) });
        Self(places)
    }

    #[cfg(test)]
    fn fixed(projects: &[(&str, &str)], workspace: &str) -> Self {
        let mut v: Vec<Place> =
            projects.iter().map(|(slug, dir)| Place { kind: ShareKind::Project, slug: Some(slug.to_string()), name: slug.to_string(), dirs: vec![dir.into()] }).collect();
        v.push(Place { kind: ShareKind::Workspace, slug: None, name: "workspace".into(), dirs: vec![workspace.into()] });
        Self(v)
    }

    /// Index of the place `cwd` is in; projects are checked before the workspace (they may live inside it).
    /// Without a cwd, Claude's project folder name (the cwd with every non-alphanumeric character as `-`) is matched.
    fn classify(&self, cwd: Option<&str>, folder: Option<&str>) -> Option<usize> {
        if let Some(cwd) = cwd {
            return self.0.iter().position(|p| p.dirs.iter().any(|d| Path::new(cwd).starts_with(d)));
        }
        let folder = folder?;
        self.0.iter().position(|p| {
            p.dirs.iter().any(|d| {
                let enc = claude_folder_name(d);
                folder == enc || folder.strip_prefix(enc.as_str()).is_some_and(|rest| rest.starts_with('-'))
            })
        })
    }
}

/// Claude Code names a project's transcript folder after its cwd with every non-alphanumeric character as `-`.
pub fn claude_folder_name(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Token totals since `today`, and shares by place since `window_start`.
fn aggregate<'a>(scans: impl Iterator<Item = (&'a FileScan, Option<&'a str>)>, places: &Places, today: DateTime<Utc>, window_start: Option<DateTime<Utc>>) -> (TokenTotals, Vec<ProjectShare>) {
    let mut total = TokenTotals::default();
    let mut by: HashMap<Option<usize>, u64> = HashMap::new();
    for (scan, folder) in scans {
        for e in &scan.events {
            if e.at >= today {
                total.add(&e.tokens);
            }
            if window_start.is_some_and(|s| e.at >= s) {
                let cwd = e.cwd.as_deref().or(scan.cwd.as_deref());
                *by.entry(places.classify(cwd, folder)).or_default() += e.tokens.weight();
            }
        }
    }
    let mut shares: Vec<ProjectShare> = by
        .into_iter()
        .filter(|(_, n)| *n > 0)
        .map(|(i, tokens)| match i.map(|i| &places.0[i]) {
            Some(p) => ProjectShare { kind: p.kind, slug: p.slug.clone(), name: p.name.clone(), tokens },
            None => ProjectShare { kind: ShareKind::Other, slug: None, name: "other".into(), tokens },
        })
        .collect();
    shares.sort_by(|a, b| b.tokens.cmp(&a.tokens).then_with(|| a.name.cmp(&b.name)));
    (total, shares)
}

fn local_midnight(now: DateTime<Utc>) -> DateTime<Utc> {
    let day = now.with_timezone(&Local).date_naive().and_hms_opt(0, 0, 0).unwrap_or_default();
    Local.from_local_datetime(&day).earliest().map_or(now - chrono::Duration::hours(24), |d| d.with_timezone(&Utc))
}

// ---- session-log scans (cached per file by modification time and size)

#[derive(Debug, Clone)]
struct TokenEvent {
    at: DateTime<Utc>,
    cwd: Option<String>,
    tokens: TokenTotals,
}

#[derive(Debug, Clone, Default)]
pub struct FileScan {
    /// The session's working directory (Codex `session_meta`).
    cwd: Option<String>,
    events: Vec<TokenEvent>,
    /// Codex rate-limit snapshots, in file order.
    limits: Vec<(DateTime<Utc>, CodexRateLimits)>,
}

#[derive(Default)]
pub struct ScanCache {
    files: HashMap<PathBuf, (SystemTime, u64, Arc<FileScan>)>,
}

impl ScanCache {
    fn get(&mut self, path: &Path, scan: fn(&mut dyn BufRead) -> FileScan) -> Option<Arc<FileScan>> {
        let meta = std::fs::metadata(path).ok()?;
        let (mtime, len) = (meta.modified().ok()?, meta.len());
        if let Some((m, l, s)) = self.files.get(path)
            && *m == mtime
            && *l == len
        {
            return Some(s.clone());
        }
        let mut reader = BufReader::new(File::open(path).ok()?);
        let parsed = Arc::new(scan(&mut reader));
        self.files.insert(path.to_path_buf(), (mtime, len, parsed.clone()));
        Some(parsed)
    }

    fn keep_only(&mut self, seen: &HashSet<PathBuf>) {
        self.files.retain(|p, _| seen.contains(p));
    }
}

/// Every file under `root` (recursively) accepted by `want`, with its modification time, newest first.
fn files_under(root: &Path, want: fn(&str) -> bool) -> Vec<(PathBuf, SystemTime)> {
    let mut out = vec![];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file()
                && path.file_name().and_then(|n| n.to_str()).is_some_and(want)
                && let Ok(m) = entry.metadata().and_then(|m| m.modified())
            {
                out.push((path, m));
            }
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(f.1));
    out
}

fn lines(reader: &mut dyn BufRead) -> impl Iterator<Item = String> + '_ {
    reader.lines().map_while(|l| l.ok())
}

// ---- Claude

#[derive(Deserialize)]
struct ClaudeLine {
    #[serde(rename = "type")]
    kind: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    cwd: Option<String>,
    message: Option<ClaudeMessage>,
}

#[derive(Deserialize)]
struct ClaudeMessage {
    id: Option<String>,
    usage: Option<ClaudeTokens>,
}

#[derive(Deserialize)]
struct ClaudeTokens {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

/// Assistant turns of a Claude Code transcript. One API message is written as several lines (one per content
/// block) that repeat its usage, so each message id counts once.
pub fn scan_claude(reader: &mut dyn BufRead) -> FileScan {
    let mut seen = HashSet::new();
    let mut scan = FileScan::default();
    for line in lines(reader) {
        if !line.contains("\"usage\"") || !line.contains("\"assistant\"") {
            continue;
        }
        let Ok(l) = serde_json::from_str::<ClaudeLine>(&line) else { continue };
        let (Some("assistant"), Some(at), Some(msg)) = (l.kind.as_deref(), l.timestamp, l.message) else { continue };
        let Some(u) = msg.usage else { continue };
        if let Some(id) = msg.id
            && !seen.insert(id)
        {
            continue;
        }
        let tokens = TokenTotals { input: u.input_tokens, output: u.output_tokens, cache_read: u.cache_read_input_tokens, cache_write: u.cache_creation_input_tokens };
        scan.events.push(TokenEvent { at, cwd: l.cwd, tokens });
    }
    scan
}

#[derive(Deserialize)]
struct ClaudeCredentials {
    #[serde(rename = "claudeAiOauth")]
    oauth: Option<ClaudeOauth>,
}

/// Deliberately not `Debug`: it holds the access token.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeOauth {
    access_token: String,
    expires_at: Option<i64>,
    subscription_type: Option<String>,
    rate_limit_tier: Option<String>,
}

/// "Max 20x" from `rateLimitTier = default_claude_max_20x`, else the capitalized `subscriptionType`.
pub fn claude_plan(subscription: Option<&str>, tier: Option<&str>) -> Option<String> {
    if let Some(tier) = tier
        && let Some(i) = tier.find("max_")
    {
        let mult = &tier[i + 4..];
        if mult.ends_with('x') && mult[..mult.len() - 1].chars().all(|c| c.is_ascii_digit()) {
            return Some(format!("Max {mult}"));
        }
    }
    subscription.filter(|s| !s.is_empty()).map(|s| {
        let mut c = s.chars();
        c.next().map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
    })
}

fn window(id: &str, label: &str, short: &str, kind: &str, minutes: Option<u64>, used_pct: Option<f64>, resets_at: Option<DateTime<Utc>>) -> UsageWindow {
    UsageWindow {
        id: id.into(),
        label: label.into(),
        short: short.into(),
        kind: kind.into(),
        used_pct,
        used: None,
        cap: None,
        resets_at,
        window_minutes: minutes,
        rate_per_hour: None,
        projected_pct: None,
        note: None,
    }
}

fn time(v: &Value) -> Option<DateTime<Utc>> {
    v.as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok()).map(|d| d.with_timezone(&Utc))
}

/// The windows of an `/api/oauth/usage` response: its `limits` list when present (session, weekly, weekly per
/// model), else the older `five_hour` / `seven_day` / `seven_day_<model>` fields.
pub fn parse_claude_usage(v: &Value) -> Vec<UsageWindow> {
    let mut out = vec![];
    if let Some(limits) = v.get("limits").and_then(Value::as_array).filter(|l| !l.is_empty()) {
        for l in limits {
            let kind = l.get("kind").and_then(Value::as_str).unwrap_or("");
            let group = l.get("group").and_then(Value::as_str).unwrap_or("");
            let pct = l.get("percent").and_then(Value::as_f64);
            let resets = l.get("resets_at").and_then(time);
            let model = l.pointer("/scope/model/display_name").and_then(Value::as_str);
            let w = match (kind, model) {
                ("session", _) => window("session", "Current session", "5h session", "5-hour window", Some(300), pct, resets),
                ("weekly_all", _) => window("weekly_all", "Weekly · all models", "7d all", "7-day window", Some(10080), pct, resets),
                ("weekly_scoped", Some(m)) => {
                    let id = format!("weekly_{}", m.to_lowercase().replace(' ', "_"));
                    window(&id, &format!("Weekly · {m}"), &format!("7d {m}"), "7-day window", Some(10080), pct, resets)
                }
                _ => {
                    let minutes = match group {
                        "session" => Some(300),
                        "weekly" => Some(10080),
                        _ => None,
                    };
                    let label = kind.replace('_', " ");
                    let kind_text = minutes.map_or("plan limit", |m| if m == 300 { "5-hour window" } else { "7-day window" });
                    window(kind, &label, &label, kind_text, minutes, pct, resets)
                }
            };
            if !out.iter().any(|o: &UsageWindow| o.id == w.id) {
                out.push(w);
            }
        }
        return out;
    }
    let fields = [
        ("five_hour", "session", "Current session", "5h session", "5-hour window", 300),
        ("seven_day", "weekly_all", "Weekly · all models", "7d all", "7-day window", 10080),
        ("seven_day_opus", "weekly_opus", "Weekly · Opus", "7d Opus", "7-day window", 10080),
        ("seven_day_sonnet", "weekly_sonnet", "Weekly · Sonnet", "7d Sonnet", "7-day window", 10080),
    ];
    for (field, id, label, short, kind, minutes) in fields {
        let Some(w) = v.get(field).filter(|w| w.is_object()) else { continue };
        let Some(pct) = w.get("utilization").and_then(Value::as_f64) else { continue };
        out.push(window(id, label, short, kind, Some(minutes), Some(pct), w.get("resets_at").and_then(time)));
    }
    out
}

/// Fetches the account's limits. Errors are short and never contain the token.
async fn fetch_claude(hub: &Hub, oauth: &ClaudeOauth) -> Result<Vec<UsageWindow>, String> {
    let response = hub
        .http
        .get(CLAUDE_USAGE_URL)
        .bearer_auth(&oauth.access_token)
        .header("anthropic-beta", "oauth-2025-04-20")
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| if e.is_timeout() { "the usage endpoint timed out".to_string() } else { "could not reach api.anthropic.com".to_string() })?;
    let status = response.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(format!("the usage endpoint refused Claude Code's sign-in ({status}); run `claude` once to refresh it"));
    }
    if !status.is_success() {
        return Err(format!("the usage endpoint answered {status}"));
    }
    let body: Value = response.json().await.map_err(|_| "the usage endpoint sent a body that is not JSON".to_string())?;
    let windows = parse_claude_usage(&body);
    if windows.is_empty() {
        return Err("the usage endpoint listed no limits".into());
    }
    Ok(windows)
}

async fn read_claude(hub: &Hub, name: &str, last: Option<&ProviderUsage>, places: &Places, now: DateTime<Utc>) -> ProviderUsage {
    let cfg = &hub.config.usage;
    let mut p = blank(name, Some(Reader::Claude), UsageStatus::Live);
    let creds = std::fs::read_to_string(&cfg.claude_credentials).ok().and_then(|t| serde_json::from_str::<ClaudeCredentials>(&t).ok());
    let outcome = match creds.as_ref().map(|c| c.oauth.as_ref()) {
        None => Err((UsageStatus::SignedOut, format!("not signed in: no readable {}", cfg.claude_credentials.display()))),
        Some(None) => Err((UsageStatus::SignedOut, "no Claude plan sign-in (claudeAiOauth) in the credentials file".to_string())),
        Some(Some(oauth)) => {
            p.plan = claude_plan(oauth.subscription_type.as_deref(), oauth.rate_limit_tier.as_deref());
            if oauth.expires_at.is_some_and(|ms| ms < now.timestamp_millis()) {
                Err((UsageStatus::Error, "Claude Code's sign-in token has expired; run `claude` once to refresh it".to_string()))
            } else {
                fetch_claude(hub, oauth).await.map_err(|e| (UsageStatus::Error, e))
            }
        }
    };
    match outcome {
        Ok(windows) => {
            for w in &windows {
                if let Some(u) = w.used_pct
                    && let Err(err) = hub.store.insert_usage_sample(name, &w.id, now, u, w.resets_at).await
                {
                    tracing::warn!("storing a usage sample: {err:#}");
                }
            }
            p.windows = windows;
            p.read_at = Some(now);
        }
        Err((status, error)) => {
            p.status = status;
            p.error = Some(error);
            // The last good reading still says where the account stands (and keeps the skip line in force).
            if let Some(last) = last {
                p.windows = last.windows.iter().cloned().map(|w| UsageWindow { rate_per_hour: None, projected_pct: None, ..w }).collect();
                p.read_at = last.read_at;
                p.plan = p.plan.take().or_else(|| last.plan.clone());
            }
        }
    }

    // Tokens: transcripts touched since midnight or since the 5-hour window opened, whichever is earlier.
    let today = local_midnight(now);
    let session = p.windows.iter().find(|w| w.id == "session");
    let window_start = match session.and_then(|w| w.resets_at) {
        Some(r) if r > now => r - chrono::Duration::minutes(300),
        Some(r) => r,
        None => now - chrono::Duration::minutes(300),
    };
    let since = today.min(window_start);
    let root = cfg.claude_projects.clone();
    let cache = hub.usage.cache.clone();
    let scans = tokio::task::spawn_blocking(move || {
        let mut cache = cache.lock().unwrap();
        let mut out = vec![];
        for (path, mtime) in files_under(&root, |n| n.ends_with(".jsonl")) {
            if DateTime::<Utc>::from(mtime) < since {
                break;
            }
            let folder = path.strip_prefix(&root).ok().and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().into_owned());
            if let Some(scan) = cache.get(&path, scan_claude) {
                out.push((path, scan, folder));
            }
        }
        out
    })
    .await
    .unwrap_or_default();
    let (tokens, shares) = aggregate(scans.iter().map(|(_, s, f)| (s.as_ref(), f.as_deref())), places, today, Some(window_start));
    p.tokens_today = Some(tokens);
    p.by_project = shares;
    p.by_project_since = Some(window_start);
    let mut seen: HashSet<PathBuf> = scans.into_iter().map(|(path, _, _)| path).collect();
    retain_cache(hub, &mut seen, &cfg.claude_projects);
    p
}

/// Drops cached scans under `root` that were not used this round (old files fall out of the window).
fn retain_cache(hub: &Hub, seen: &mut HashSet<PathBuf>, root: &Path) {
    let mut cache = hub.usage.cache.lock().unwrap();
    let others: Vec<PathBuf> = cache.files.keys().filter(|p| !p.starts_with(root)).cloned().collect();
    seen.extend(others);
    cache.keep_only(seen);
}

// ---- Codex

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CodexLimit {
    pub used_percent: f64,
    pub window_minutes: Option<u64>,
    /// Unix seconds.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CodexRateLimits {
    pub primary: Option<CodexLimit>,
    pub secondary: Option<CodexLimit>,
    pub plan_type: Option<String>,
}

#[derive(Deserialize)]
struct CodexLine {
    timestamp: Option<DateTime<Utc>>,
    #[serde(rename = "type")]
    kind: Option<String>,
    payload: Option<Value>,
}

#[derive(Deserialize, Default, Clone, Copy)]
struct CodexTokens {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    #[serde(default)]
    cache_write_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

impl CodexTokens {
    fn totals(self) -> TokenTotals {
        TokenTotals {
            input: self.input_tokens.saturating_sub(self.cached_input_tokens),
            output: self.output_tokens,
            cache_read: self.cached_input_tokens,
            cache_write: self.cache_write_input_tokens,
        }
    }
}

/// A Codex rollout log: its cwd, the tokens of each turn (differences of the running `total_token_usage`, so a
/// repeated event adds nothing) and every `rate_limits` snapshot.
pub fn scan_codex(reader: &mut dyn BufRead) -> FileScan {
    let mut scan = FileScan::default();
    let mut prev = TokenTotals::default();
    for line in lines(reader) {
        let meta = line.contains("\"session_meta\"");
        if !meta && !line.contains("\"token_count\"") {
            continue;
        }
        let Ok(l) = serde_json::from_str::<CodexLine>(&line) else { continue };
        let Some(payload) = l.payload else { continue };
        if l.kind.as_deref() == Some("session_meta") {
            scan.cwd = payload.get("cwd").and_then(Value::as_str).map(String::from);
            continue;
        }
        if payload.get("type").and_then(Value::as_str) != Some("token_count") {
            continue;
        }
        let Some(at) = l.timestamp else { continue };
        if let Some(rl) = payload.get("rate_limits").filter(|v| v.is_object()).and_then(|v| serde_json::from_value::<CodexRateLimits>(v.clone()).ok()) {
            scan.limits.push((at, rl));
        }
        let Some(total) = payload.pointer("/info/total_token_usage").and_then(|v| serde_json::from_value::<CodexTokens>(v.clone()).ok()) else { continue };
        let total = total.totals();
        let delta = if total.weight() + total.cache_read < prev.weight() + prev.cache_read {
            total // the running total restarted
        } else {
            TokenTotals {
                input: total.input.saturating_sub(prev.input),
                output: total.output.saturating_sub(prev.output),
                cache_read: total.cache_read.saturating_sub(prev.cache_read),
                cache_write: total.cache_write.saturating_sub(prev.cache_write),
            }
        };
        prev = total;
        if delta != TokenTotals::default() {
            scan.events.push(TokenEvent { at, cwd: None, tokens: delta });
        }
    }
    scan
}

/// "Pro Lite" from `prolite`.
pub fn codex_plan(plan: &str) -> String {
    match plan {
        "free" => "Free".into(),
        "plus" => "Plus".into(),
        "pro" => "Pro".into(),
        "prolite" => "Pro Lite".into(),
        "team" => "Team".into(),
        "business" => "Business".into(),
        "enterprise" => "Enterprise".into(),
        "edu" => "Edu".into(),
        other => other.to_string(),
    }
}

/// The windows of a snapshot taken at `at`. A window whose reset passed since then is shown as unknown.
pub fn codex_windows(rl: &CodexRateLimits, now: DateTime<Utc>) -> Vec<UsageWindow> {
    let mut out = vec![];
    for (slot, limit) in [("primary", &rl.primary), ("secondary", &rl.secondary)] {
        let Some(l) = limit else { continue };
        let minutes = l.window_minutes;
        let (label, short, kind) = match minutes {
            Some(300) => ("5-hour limit".to_string(), "5h limit".to_string(), "5-hour window".to_string()),
            Some(10080) => ("Weekly limit".into(), "7d limit".into(), "7-day window".into()),
            Some(43200) => ("Monthly limit".into(), "30d limit".into(), "30-day window".into()),
            Some(m) if m % 1440 == 0 => (format!("{}-day limit", m / 1440), format!("{}d limit", m / 1440), format!("{}-day window", m / 1440)),
            Some(m) if m % 60 == 0 => (format!("{}-hour limit", m / 60), format!("{}h limit", m / 60), format!("{}-hour window", m / 60)),
            Some(m) => (format!("{m}-minute limit"), format!("{m}m limit"), format!("{m}-minute window")),
            None => ("Limit".into(), "limit".into(), "plan window".into()),
        };
        let resets = l.resets_at.and_then(|s| Utc.timestamp_opt(s, 0).single());
        let id = format!("{slot}_{}", minutes.unwrap_or(0));
        let mut w = window(&id, &label, &short, &kind, minutes, Some(l.used_percent), resets);
        if let Some(r) = resets
            && r <= now
        {
            w.used_pct = None;
            w.note = Some(format!("reset at {} after the last codex run; no newer reading", r.with_timezone(&Local).format("%a %d %b %H:%M")));
        }
        out.push(w);
    }
    out
}

async fn read_codex(hub: &Hub, name: &str, _last: Option<&ProviderUsage>, places: &Places, now: DateTime<Utc>) -> ProviderUsage {
    let cfg = &hub.config.usage;
    let mut p = blank(name, Some(Reader::Codex), UsageStatus::Snapshot);
    let today = local_midnight(now);
    // Files of the last week and a day cover today and a weekly window; older ones are read only to find a snapshot.
    let recent = today.min(now - chrono::Duration::days(KEEP_SAMPLES_DAYS));
    let root = cfg.codex_sessions.clone();
    let cache = hub.usage.cache.clone();
    let scans = tokio::task::spawn_blocking(move || {
        let mut cache = cache.lock().unwrap();
        let mut out = vec![];
        let mut found = false;
        for (path, mtime) in files_under(&root, |n| n.starts_with("rollout-") && n.ends_with(".jsonl")) {
            let old = DateTime::<Utc>::from(mtime) < recent;
            if old && found {
                break;
            }
            if let Some(scan) = cache.get(&path, scan_codex) {
                found |= !scan.limits.is_empty();
                out.push((path, scan));
            }
        }
        out
    })
    .await
    .unwrap_or_default();

    let newest = scans.iter().flat_map(|(_, s)| s.limits.iter()).max_by_key(|(at, _)| *at).cloned();
    let signed_out = !cfg.codex_auth.exists();
    match &newest {
        Some((at, rl)) => {
            p.windows = codex_windows(rl, now);
            p.read_at = Some(*at);
            p.plan = rl.plan_type.as_deref().map(codex_plan);
            // Every snapshot of the last hour is a real reading: they give the pace while Codex is running.
            for (t, snap) in scans.iter().flat_map(|(_, s)| s.limits.iter()).filter(|(t, _)| *t >= now - chrono::Duration::hours(1)) {
                for w in codex_windows(snap, *t) {
                    if let Some(u) = w.used_pct
                        && let Err(err) = hub.store.insert_usage_sample(name, &w.id, *t, u, w.resets_at).await
                    {
                        tracing::warn!("storing a usage sample: {err:#}");
                    }
                }
            }
        }
        None => {
            p.status = UsageStatus::Error;
            p.error = Some(format!("no rate-limit snapshot in {} yet; it appears after Codex's first run", cfg.codex_sessions.display()));
        }
    }
    if signed_out {
        p.status = UsageStatus::SignedOut;
        let age = newest.as_ref().map_or(String::new(), |(at, _)| format!("; showing the last snapshot, from {}", at.with_timezone(&Local).format("%a %d %b %H:%M")));
        p.error = Some(format!("Codex is signed out on this machine (no {}){age}", cfg.codex_auth.display()));
    }

    // By project: since the shortest window opened (a window that already reset has had no use since).
    let shortest = p.windows.iter().filter(|w| w.resets_at.is_some() && w.window_minutes.is_some()).min_by_key(|w| w.window_minutes);
    let window_start = shortest.and_then(|w| {
        let r = w.resets_at?;
        Some(if r > now { r - chrono::Duration::minutes(w.window_minutes? as i64) } else { r })
    });
    let (tokens, shares) = aggregate(scans.iter().map(|(_, s)| (s.as_ref(), None)), places, today, window_start);
    p.tokens_today = Some(tokens);
    p.by_project = shares;
    p.by_project_since = window_start;
    let mut seen: HashSet<PathBuf> = scans.into_iter().map(|(path, _)| path).collect();
    retain_cache(hub, &mut seen, &cfg.codex_sessions);
    p
}

// ---- OpenRouter: the API key's own limit (`GET /api/v1/key`)

/// The key's plan and limits from a `GET /api/v1/key` body: its credit limit as a window when it has one (spent
/// dollars against the limit, resetting daily/weekly/monthly or never), and the free models' daily request cap.
pub fn parse_openrouter_key(v: &Value) -> (Option<String>, Vec<UsageWindow>) {
    let d = v.get("data").unwrap_or(v);
    let num = |k: &str| d.get(k).and_then(Value::as_f64);
    let usage = num("usage");
    let free = d.get("is_free_tier").and_then(Value::as_bool) == Some(true);
    let mut plan = if free { "free tier".to_string() } else { "credits".to_string() };
    let mut out = vec![];
    match num("limit").filter(|l| *l > 0.0) {
        Some(limit) => {
            let reset = d.get("limit_reset").and_then(Value::as_str).filter(|r| !r.is_empty() && *r != "never");
            let used = match num("limit_remaining") {
                Some(left) => (limit - left).max(0.0),
                None => match reset {
                    Some("daily") => num("usage_daily").unwrap_or(0.0),
                    Some("weekly") => num("usage_weekly").unwrap_or(0.0),
                    Some("monthly") => num("usage_monthly").unwrap_or(0.0),
                    _ => usage.unwrap_or(0.0),
                },
            };
            let (kind, minutes) = match reset {
                Some("daily") => ("daily key limit", Some(1440)),
                Some("weekly") => ("weekly key limit", Some(10080)),
                Some("monthly") => ("monthly key limit", None),
                _ => ("key limit", None),
            };
            let mut w = window("key_limit", "Key credit limit", "credits", kind, minutes, Some((used / limit * 100.0).clamp(0.0, 100.0)), None);
            w.used = Some(used);
            w.cap = Some(limit);
            w.note = Some(format!("${used:.2} of ${limit:.2}"));
            out.push(w);
        }
        None => {
            if let Some(u) = usage {
                plan = format!("{plan} · ${u:.2} used, no key limit");
            }
        }
    }
    if let Some(f) = d.get("free_model_daily_requests").filter(|f| f.is_object()) {
        let (used, cap) = (f.get("used").and_then(Value::as_f64), f.get("limit").and_then(Value::as_f64));
        if let (Some(used), Some(cap)) = (used, cap.filter(|c| *c > 0.0)) {
            let mut w = window("free_daily", "Free models · today", "free/day", "daily request cap", Some(1440), Some((used / cap * 100.0).clamp(0.0, 100.0)), None);
            w.used = Some(used);
            w.cap = Some(cap);
            out.push(w);
        }
    }
    (Some(plan), out)
}

/// The OpenRouter key's limits. The key is read from the environment for this one request to openrouter.ai and goes
/// nowhere else: never logged, stored or returned.
async fn read_openrouter(hub: &Hub, name: &str, last: Option<&ProviderUsage>, now: DateTime<Utc>) -> ProviderUsage {
    let mut p = blank(name, Some(Reader::OpenRouter), UsageStatus::Live);
    let spec = hub.config.providers.get(name);
    let var = spec.and_then(|s| s.api_key_env.clone()).unwrap_or_else(|| "OPENROUTER_API_KEY".into());
    let key = hub.config.env_value(&var).or_else(|| {
        let v = spec?.env.get(&var)?;
        match crate::config::env_ref(v) {
            Some(r) => hub.config.env_value(r),
            None => Some(v.clone()).filter(|v| !v.is_empty()),
        }
    });
    let Some(key) = key else {
        p.status = UsageStatus::SignedOut;
        p.error = Some(format!("{var} is not set for this server"));
        return p;
    };
    let fetched = async {
        let response = hub.http.get(OPENROUTER_KEY_URL).bearer_auth(&key).timeout(Duration::from_secs(15)).send().await.map_err(|e| {
            if e.is_timeout() { "openrouter.ai timed out".to_string() } else { "could not reach openrouter.ai".to_string() }
        })?;
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(format!("openrouter.ai refused {var} ({status})"));
        }
        if !status.is_success() {
            return Err(format!("openrouter.ai answered {status}"));
        }
        response.json::<Value>().await.map_err(|_| "openrouter.ai sent a body that is not JSON".to_string())
    }
    .await;
    match fetched {
        Ok(body) => {
            let (plan, windows) = parse_openrouter_key(&body);
            for w in &windows {
                if let Some(u) = w.used_pct
                    && let Err(err) = hub.store.insert_usage_sample(name, &w.id, now, u, w.resets_at).await
                {
                    tracing::warn!("storing a usage sample: {err:#}");
                }
            }
            p.plan = plan;
            p.windows = windows;
            p.read_at = Some(now);
        }
        Err(error) => {
            p.status = UsageStatus::Error;
            p.error = Some(error);
            if let Some(last) = last {
                p.windows = last.windows.iter().cloned().map(|w| UsageWindow { rate_per_hour: None, projected_pct: None, ..w }).collect();
                p.read_at = last.read_at;
                p.plan = last.plan.clone();
            }
        }
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TaskStatus;
    use crate::orchestrator::Orchestrator;
    use crate::store::NewTask;
    use crate::terminal::fake::FakeTerminal;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn claude_limits_list_becomes_windows() {
        let body: Value = serde_json::from_str(
            r#"{"five_hour":{"utilization":31.0,"resets_at":"2026-10-13T17:00:00.308579+00:00"},
                "seven_day":{"utilization":38.0,"resets_at":"2026-10-15T16:00:00.308608+00:00"},"seven_day_opus":null,
                "limits":[{"kind":"session","group":"session","percent":31,"severity":"normal","resets_at":"2026-10-13T17:00:00.308579+00:00"},
                          {"kind":"weekly_all","group":"weekly","percent":38,"severity":"normal","resets_at":"2026-10-15T16:00:00.308608+00:00"},
                          {"kind":"weekly_scoped","group":"weekly","percent":0,"resets_at":"2026-10-15T16:00:00+00:00","scope":{"model":{"id":null,"display_name":"Fable"}}}]}"#,
        )
        .unwrap();
        let w = parse_claude_usage(&body);
        let ids: Vec<&str> = w.iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, ["session", "weekly_all", "weekly_fable"]);
        assert_eq!(w[0].used_pct, Some(31.0));
        assert_eq!(w[0].window_minutes, Some(300));
        assert_eq!(w[2].label, "Weekly · Fable");
        assert_eq!(w[1].resets_at, Some(t("2026-10-15T16:00:00.308608Z")));
    }

    #[test]
    fn claude_old_fields_are_the_fallback() {
        let body: Value = serde_json::from_str(
            r#"{"five_hour":{"utilization":62.5,"resets_at":"2026-10-13T17:00:00+00:00"},"seven_day":null,
                "seven_day_opus":{"utilization":88,"resets_at":"2026-10-16T09:00:00+00:00"},"seven_day_sonnet":null}"#,
        )
        .unwrap();
        let w = parse_claude_usage(&body);
        assert_eq!(w.iter().map(|w| (w.id.as_str(), w.used_pct)).collect::<Vec<_>>(), [("session", Some(62.5)), ("weekly_opus", Some(88.0))]);
    }

    #[test]
    fn claude_plan_names() {
        assert_eq!(claude_plan(Some("max"), Some("default_claude_max_20x")).as_deref(), Some("Max 20x"));
        assert_eq!(claude_plan(Some("max"), Some("default_claude_max_5x")).as_deref(), Some("Max 5x"));
        assert_eq!(claude_plan(Some("pro"), None).as_deref(), Some("Pro"));
        assert_eq!(claude_plan(None, None), None);
    }

    #[test]
    fn claude_transcript_counts_each_message_once() {
        let text = [
            r#"{"type":"user","timestamp":"2026-10-13T12:00:00Z","cwd":"/w","message":{"role":"user","content":"hi"}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-13T12:00:05Z","cwd":"/w","message":{"id":"m1","usage":{"input_tokens":2,"output_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":50}}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-13T12:00:06Z","cwd":"/w","message":{"id":"m1","usage":{"input_tokens":2,"output_tokens":10,"cache_read_input_tokens":100,"cache_creation_input_tokens":50}}}"#,
            r#"{"type":"assistant","timestamp":"2026-10-13T12:01:00Z","cwd":"/w/sub","message":{"id":"m2","usage":{"input_tokens":3,"output_tokens":7}}}"#,
            "not json \"usage\" \"assistant\"",
        ]
        .join("\n");
        let scan = scan_claude(&mut text.as_bytes());
        assert_eq!(scan.events.len(), 2);
        assert_eq!(scan.events[0].tokens, TokenTotals { input: 2, output: 10, cache_read: 100, cache_write: 50 });
        assert_eq!(scan.events[1].cwd.as_deref(), Some("/w/sub"));
    }

    const CODEX: &str = r#"{"timestamp":"2026-10-13T10:00:00Z","type":"session_meta","payload":{"id":"x","cwd":"/data/projects/acme/workspace"}}
{"timestamp":"2026-10-13T10:00:01Z","type":"event_msg","payload":{"type":"token_count","info":null,"rate_limits":{"limit_id":"codex","primary":{"used_percent":5.0,"window_minutes":300,"resets_at":1791900000},"secondary":{"used_percent":40.0,"window_minutes":10080,"resets_at":1792300000},"plan_type":"plus"}}}
{"timestamp":"2026-10-13T10:05:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":600,"cache_write_input_tokens":0,"output_tokens":50}},"rate_limits":{"primary":{"used_percent":7.0,"window_minutes":300,"resets_at":1791900000},"secondary":null,"plan_type":"plus"}}}
{"timestamp":"2026-10-13T10:05:01Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":600,"cache_write_input_tokens":0,"output_tokens":50}},"rate_limits":null}}
{"timestamp":"2026-10-13T10:09:00Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1500,"cached_input_tokens":800,"cache_write_input_tokens":0,"output_tokens":80}}}}
{"timestamp":"2026-10-13T10:09:30Z","type":"response_item","payload":{"type":"message","content":"talking about token_count"}}"#;

    #[test]
    fn codex_rollout_gives_snapshots_and_turn_tokens() {
        let scan = scan_codex(&mut CODEX.as_bytes());
        assert_eq!(scan.cwd.as_deref(), Some("/data/projects/acme/workspace"));
        assert_eq!(scan.limits.len(), 2);
        assert_eq!(scan.limits[1].1.primary.as_ref().unwrap().used_percent, 7.0);
        // The repeated total adds nothing; the second turn is the difference.
        let tokens: Vec<TokenTotals> = scan.events.iter().map(|e| e.tokens).collect();
        assert_eq!(tokens, [TokenTotals { input: 400, output: 50, cache_read: 600, cache_write: 0 }, TokenTotals { input: 300, output: 30, cache_read: 200, cache_write: 0 }]);
    }

    #[test]
    fn codex_windows_label_by_length_and_hide_a_passed_reset() {
        let scan = scan_codex(&mut CODEX.as_bytes());
        let rl = &scan.limits[0].1;
        let before = Utc.timestamp_opt(1791890000, 0).unwrap();
        let w = codex_windows(rl, before);
        assert_eq!(w.iter().map(|w| w.label.as_str()).collect::<Vec<_>>(), ["5-hour limit", "Weekly limit"]);
        assert_eq!(w[0].used_pct, Some(5.0));
        let after = Utc.timestamp_opt(1791950000, 0).unwrap();
        let w = codex_windows(rl, after);
        assert_eq!(w[0].used_pct, None, "the 5-hour window reset after the snapshot");
        assert!(w[0].note.is_some());
        assert_eq!(w[1].used_pct, Some(40.0));
        assert_eq!(codex_plan("prolite"), "Pro Lite");
    }

    #[test]
    fn tokens_are_attributed_to_projects_workspace_and_other() {
        let codex = scan_codex(&mut CODEX.as_bytes());
        let claude = FileScan {
            cwd: None,
            limits: vec![],
            events: vec![
                TokenEvent { at: t("2026-10-13T11:00:00Z"), cwd: Some("/repo/src".into()), tokens: TokenTotals { input: 10, output: 0, cache_read: 999, cache_write: 0 } },
                TokenEvent { at: t("2026-10-13T11:00:00Z"), cwd: Some("/elsewhere".into()), tokens: TokenTotals { input: 5, ..Default::default() } },
                // Before the window: counts today, not in the shares.
                TokenEvent { at: t("2026-10-13T01:00:00Z"), cwd: Some("/repo".into()), tokens: TokenTotals { input: 1000, ..Default::default() } },
            ],
        };
        // A transcript line without a cwd falls back to Claude's folder name.
        let folder_only = FileScan { cwd: None, limits: vec![], events: vec![TokenEvent { at: t("2026-10-13T11:00:00Z"), cwd: None, tokens: TokenTotals { output: 7, ..Default::default() } }] };
        let places = Places::fixed(&[("acme", "/data/projects/acme")], "/repo");
        let (total, shares) = aggregate(
            [(&codex, None), (&claude, None), (&folder_only, Some("-data-projects-acme-workspace"))].into_iter(),
            &places,
            t("2026-10-13T00:00:00Z"),
            Some(t("2026-10-13T08:00:00Z")),
        );
        assert_eq!(total.input, 400 + 300 + 10 + 5 + 1000);
        let got: Vec<(ShareKind, &str, u64)> = shares.iter().map(|s| (s.kind, s.name.as_str(), s.tokens)).collect();
        assert_eq!(got, [(ShareKind::Project, "acme", 450 + 330 + 7), (ShareKind::Workspace, "workspace", 10), (ShareKind::Other, "other", 5)]);
        assert_eq!(claude_folder_name(Path::new("/tmp/claude-1000/-home-x/a.b")), "-tmp-claude-1000--home-x-a-b");
    }

    #[test]
    fn pace_needs_a_span_and_projects_to_reset() {
        let now = t("2026-10-13T12:00:00Z");
        assert_eq!(pace(&[(t("2026-10-13T11:55:00Z"), 10.0), (now, 11.0)]), None);
        let rate = pace(&[(t("2026-10-13T11:30:00Z"), 10.0), (t("2026-10-13T11:45:00Z"), 12.0), (now, 15.0)]).unwrap();
        assert!((rate - 10.0).abs() < 1e-9);
        assert!((projected(15.0, rate, t("2026-10-13T14:00:00Z"), now) - 35.0).abs() < 1e-9);
        assert_eq!(pace(&[(t("2026-10-13T11:00:00Z"), 20.0), (now, 18.0)]), Some(0.0));
    }

    fn win(used: f64, resets_at: Option<DateTime<Utc>>) -> UsageWindow {
        window("w", "W", "w", "k", Some(300), Some(used), resets_at)
    }

    #[test]
    fn skip_line_lasts_until_the_reset() {
        let now = t("2026-10-13T12:00:00Z");
        let reset = t("2026-10-13T14:00:00Z");
        let recheck = Duration::from_secs(300);
        assert_eq!(skip_until(&[win(89.9, Some(reset))], 90.0, now, recheck), None);
        assert_eq!(skip_until(&[win(40.0, None), win(90.0, Some(reset))], 90.0, now, recheck), Some(reset));
        assert_eq!(skip_until(&[win(95.0, Some(t("2026-10-13T11:00:00Z")))], 90.0, now, recheck), None, "already reset");
        assert_eq!(skip_until(&[win(95.0, None)], 90.0, now, recheck), Some(now + chrono::Duration::minutes(5)));
    }

    async fn hub_with(extra: &str) -> (Arc<Hub>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[orchestrator]\nprovider_order = [\"claude\", \"codex\"]\n[providers.claude]\nbinary = \"/bin/sh\"\n[providers.codex]\nbinary = \"/bin/sh\"\n{extra}",
            dir.path().display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        (Hub::new(config, Store::memory().await.unwrap(), Arc::new(FakeTerminal::default())), dir)
    }

    fn report_with(hub: &Hub, provider: &str, used: f64) -> UsageReport {
        let mut r = hub.usage.report();
        let p = r.providers.iter_mut().find(|p| p.provider == provider).unwrap();
        p.windows = vec![win(used, Some(Utc::now() + chrono::Duration::hours(2)))];
        r
    }

    #[tokio::test]
    async fn isolated_mode_reads_codex_and_claude_state_from_maps_own_folders() {
        let (hub, dir) = hub_with("[agents_home]\nmode = \"isolated\"\n").await;
        let ah = dir.path().join("data/agent-home");
        let now = Utc::now();
        let places = Places::fixed(&[], "/repo");
        let p = read_codex(&hub, "codex", None, &places, now).await;
        assert_eq!(p.status, UsageStatus::SignedOut);
        assert!(p.error.as_deref().unwrap().contains(&ah.join("codex/auth.json").display().to_string()), "{:?}", p.error);
        std::fs::create_dir_all(ah.join("codex")).unwrap();
        std::fs::write(ah.join("codex/auth.json"), "{}").unwrap();
        let p = read_codex(&hub, "codex", None, &places, now).await;
        assert!(p.error.as_deref().unwrap().contains(&ah.join("codex/sessions").display().to_string()), "{:?}", p.error);
        // Claude's sign-in for the usage call: MAP's copy, never ~/.claude.
        assert_eq!(hub.config.usage.claude_credentials, ah.join("claude/.credentials.json"));
    }

    #[tokio::test]
    async fn orchestrator_passes_over_a_provider_past_the_skip_line() {
        let (hub, _dir) = hub_with("").await;
        assert_eq!(reader_for(&hub.config, "claude"), Some(Reader::Claude));
        assert_eq!(reader_for(&hub.config, "gemini"), Some(Reader::None));
        hub.usage.set_report(report_with(&hub, "claude", 93.0));
        assert!(hub.cooldown_until("claude").is_some());
        let task = hub.store.create_task(NewTask { title: "a".into(), prompt: "do a".into(), depends_on: vec![], provider: None, model: None, project: None, workflow: None }).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let session = hub.store.task(task.id).await.unwrap().unwrap().session.unwrap();
        assert_eq!(hub.store.session(&session).await.unwrap().unwrap().provider, "codex");
    }

    #[tokio::test]
    async fn a_task_pinned_to_a_skipped_provider_waits() {
        let (hub, _dir) = hub_with("[usage]\nskip_pct = 80").await;
        hub.usage.set_report(report_with(&hub, "claude", 85.0));
        let task = hub.store.create_task(NewTask { title: "a".into(), prompt: "do a".into(), depends_on: vec![], provider: Some("claude".into()), model: None, project: None, workflow: None }).await.unwrap();
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        let task = hub.store.task(task.id).await.unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Pending);
        assert!(task.result.unwrap().starts_with("Waiting"));

        // Below the line again: it runs.
        hub.usage.set_report(report_with(&hub, "claude", 50.0));
        assert!(hub.cooldown_until("claude").is_none());
        Orchestrator::new(hub.clone()).tick().await.unwrap();
        assert_eq!(hub.store.task(task.id).await.unwrap().unwrap().status, TaskStatus::Running);
    }

    #[test]
    fn openrouter_key_limits_become_windows() {
        // GET /api/v1/key for a key with a monthly $20 limit and the free-model daily cap.
        let body = serde_json::json!({"data": {"label": "sk-or-v1-abc...", "limit": 20.0, "limit_remaining": 15.0, "limit_reset": "monthly", "usage": 42.5, "usage_monthly": 5.0, "is_free_tier": false, "free_model_daily_requests": {"used": 10, "limit": 1000, "remaining": 990}}});
        let (plan, windows) = parse_openrouter_key(&body);
        assert_eq!(plan.as_deref(), Some("credits"));
        assert_eq!(windows.len(), 2);
        let w = &windows[0];
        assert_eq!((w.id.as_str(), w.kind.as_str(), w.used_pct, w.used, w.cap), ("key_limit", "monthly key limit", Some(25.0), Some(5.0), Some(20.0)));
        assert_eq!(w.note.as_deref(), Some("$5.00 of $20.00"));
        assert_eq!((windows[1].id.as_str(), windows[1].used_pct), ("free_daily", Some(1.0)));
        // No limit on the key: nothing to fill up, the spend goes in the plan line.
        let (plan, windows) = parse_openrouter_key(&serde_json::json!({"data": {"limit": null, "usage": 3.21, "is_free_tier": true}}));
        assert_eq!(plan.as_deref(), Some("free tier · $3.21 used, no key limit"));
        assert!(windows.is_empty());
    }

    #[tokio::test]
    async fn openrouter_without_a_key_is_signed_out_and_never_calls_out() {
        let (hub, _dir) = hub_with("[providers.openrouter]\napi_key_env = \"MAP_TEST_NO_SUCH_KEY_9Q\"\nenv = {}\n").await;
        assert_eq!(reader_for(&hub.config, "openrouter"), Some(Reader::OpenRouter), "by name, not by its `claude` binary");
        let p = read_openrouter(&hub, "openrouter", None, Utc::now()).await;
        assert_eq!(p.status, UsageStatus::SignedOut);
        assert_eq!(p.error.as_deref(), Some("MAP_TEST_NO_SUCH_KEY_9Q is not set for this server"));
        assert!(Config::parse("[workspace]\ndir = \"/r\"\n[usage.providers.cursor]\nreader = \"openrouter\"\n", Path::new("/")).is_ok());
    }

    #[tokio::test]
    async fn samples_round_trip_and_disabled_readers_are_reported() {
        let (hub, _dir) = hub_with("[usage.providers.codex]\nenabled = false").await;
        let now = t("2026-10-13T12:00:00Z");
        hub.store.insert_usage_sample("claude", "session", now - chrono::Duration::minutes(30), 10.0, Some(now)).await.unwrap();
        hub.store.insert_usage_sample("claude", "session", now, 12.0, Some(now)).await.unwrap();
        hub.store.insert_usage_sample("claude", "session", now, 12.0, Some(now)).await.unwrap();
        let s = hub.store.usage_samples("claude", "session", now - chrono::Duration::hours(1)).await.unwrap();
        assert_eq!(s.len(), 2);
        assert_eq!(s[1], (now, 12.0, Some(now)));
        hub.store.prune_usage_samples(now).await.unwrap();
        assert_eq!(hub.store.usage_samples("claude", "session", now - chrono::Duration::hours(1)).await.unwrap().len(), 1);

        let statuses: Vec<(String, UsageStatus)> = hub.usage.report().providers.into_iter().map(|p| (p.provider, p.status)).collect();
        assert_eq!(
            statuses,
            [
                ("claude".into(), UsageStatus::Pending),
                ("codex".into(), UsageStatus::Disabled),
                ("copilot".into(), UsageStatus::NoReader),
                ("cursor".into(), UsageStatus::NoReader),
                ("gemini".into(), UsageStatus::NoReader),
                ("openrouter".into(), UsageStatus::Pending),
            ]
        );
    }
}
