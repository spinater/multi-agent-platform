//! Update notifications for the agent CLIs: each provider's `version` command (run with the workspace env, no stdin,
//! a timeout) against the version its `latest` source publishes (fetched without credentials). Every release is
//! announced once as an `update.available` event; the newest announced version per provider is kept in
//! `<data_dir>/updates.json` so a restart does not announce it again.
//!
//! An update runs the provider's `update` command in a helper tmux session `<session_prefix>update-<name>`, like a
//! sign-in. When that session ends, its last pane is saved for the console and the version is checked again. A CLI
//! installed where this user cannot write (the read-only mounts of the Docker deploy) is not updated from here.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use chrono::Utc;
use regex::Regex;
use serde_json::Value;

use crate::config::{ChannelSource, Config, LatestSource, ProviderSpec};
use crate::hub::{Hub, LaunchRequest};
use crate::model::{Level, Role, Session, UpdateInfo, UpdatesReport};

/// "Check now" within this long of the last check returns that check.
pub const DEBOUNCE: Duration = Duration::from_secs(5);
/// A `latest` response larger than this is refused.
const MAX_BODY: usize = 64 * 1024;
/// How often a running update session's pane is read.
const WATCH_EVERY: Duration = Duration::from_secs(1);
/// An update session left open longer than this is no longer watched.
const WATCH_FOR: Duration = Duration::from_secs(6 * 3600);
/// Printed by the wrapper around the update command, so the exit status survives in the pane.
const EXIT_MARK: &str = "[map: update exited with status ";

// ---- versions

static VERSION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"v?(\d+(?:\.\d+){1,3})(?:-([0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*))?(?:\+[0-9A-Za-z.-]+)?").unwrap());

/// The first semver-looking token: `2.1.299 (Claude Code)` → `2.1.299`, `codex-cli 0.159.2` → `0.159.2`,
/// `1.3.4` → `1.3.4`, `v1.0.0-beta.2` → `1.0.0-beta.2`. A number glued to a word (`x86_64`, `abc1.2`) is skipped.
pub fn parse_version(text: &str) -> Option<String> {
    VERSION.captures_iter(text).find_map(|c| {
        let all = c.get(0)?;
        let before = text[..all.start()].chars().next_back();
        let after = text[all.end()..].chars().next();
        if before.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '_') || after.is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_') {
            return None;
        }
        let mut v = c[1].to_string();
        if let Some(pre) = c.get(2) {
            v.push('-');
            v.push_str(pre.as_str());
        }
        Some(v)
    })
}

/// Semver order: numeric parts compared as numbers (missing parts are 0), a release above its prereleases,
/// prerelease identifiers numeric below alphanumeric. `None` when either is not a version.
pub fn compare(a: &str, b: &str) -> Option<Ordering> {
    fn split(v: &str) -> Option<(Vec<u64>, Option<&str>)> {
        let v = v.trim().trim_start_matches('v');
        let v = v.split('+').next()?;
        let (nums, pre) = match v.split_once('-') {
            Some((n, p)) => (n, Some(p)),
            None => (v, None),
        };
        let nums = nums.split('.').map(|n| n.parse().ok()).collect::<Option<Vec<u64>>>()?;
        Some((nums, pre))
    }
    let (an, ap) = split(a)?;
    let (bn, bp) = split(b)?;
    for i in 0..an.len().max(bn.len()) {
        let (x, y) = (an.get(i).copied().unwrap_or(0), bn.get(i).copied().unwrap_or(0));
        if x != y {
            return Some(x.cmp(&y));
        }
    }
    Some(match (ap, bp) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(x), Some(y)) => {
            let (xs, ys): (Vec<&str>, Vec<&str>) = (x.split('.').collect(), y.split('.').collect());
            for (p, q) in xs.iter().zip(&ys) {
                let ord = match (p.parse::<u64>(), q.parse::<u64>()) {
                    (Ok(p), Ok(q)) => p.cmp(&q),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => p.cmp(q),
                };
                if ord != Ordering::Equal {
                    return Some(ord);
                }
            }
            xs.len().cmp(&ys.len())
        }
    })
}

/// `latest` is a newer version than `installed`.
pub fn newer(latest: &str, installed: &str) -> bool {
    compare(latest, installed) == Some(Ordering::Greater)
}

/// `a.b.0.c`: object keys, or array indexes for arrays. An empty path is the value itself.
pub fn json_path<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').filter(|k| !k.is_empty()).try_fold(v, |v, key| match v {
        Value::Array(items) => items.get(key.parse::<usize>().ok()?),
        Value::Object(map) => map.get(key),
        _ => None,
    })
}

/// The version in a `latest` response: the whole body as text, or the value at `json`.
pub fn latest_from_body(body: &str, json: Option<&str>) -> Result<String, String> {
    let Some(path) = json else {
        return parse_version(body.trim()).ok_or_else(|| "no version in the response".to_string());
    };
    let v: Value = serde_json::from_str(body).map_err(|_| "the response is not JSON".to_string())?;
    let field = json_path(&v, path).ok_or_else(|| format!("no `{path}` in the response"))?;
    let text = match field {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    parse_version(&text).ok_or_else(|| format!("`{path}` in the response is not a version"))
}

// ---- where the latest version is published

/// Go-style names, as release manifests use them: linux, darwin, windows.
pub fn os_name(os: &str) -> &str {
    match os {
        "macos" => "darwin",
        other => other,
    }
}

/// amd64, arm64.
pub fn arch_name(arch: &str) -> &str {
    match arch {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

pub fn expand_url(url: &str, os: &str, arch: &str, channel: &str) -> String {
    url.replace("{os}", os_name(os)).replace("{arch}", arch_name(arch)).replace("{channel}", channel)
}

/// The channel named in the CLI's settings file, or the default. Only a short plain name is used, so nothing odd
/// ends up in a URL.
pub fn read_channel(src: &ChannelSource) -> String {
    let path = crate::login::expand_home(&src.file);
    let named = std::fs::metadata(&path)
        .ok()
        .filter(|m| m.is_file() && m.len() <= 1 << 20)
        .and_then(|_| std::fs::read(&path).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| json_path(&v, &src.key).and_then(Value::as_str).map(str::to_string));
    match named {
        Some(c) if !c.is_empty() && c.len() <= 32 && c.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')) => c,
        _ => src.default.clone(),
    }
}

pub fn latest_url(src: &LatestSource) -> String {
    let channel = src.channel.as_ref().map(read_channel).unwrap_or_default();
    expand_url(&src.url, std::env::consts::OS, std::env::consts::ARCH, &channel)
}

/// GETs `url` with no credentials, cookies or custom headers besides a User-Agent, and reads the version from it.
pub async fn fetch_latest(http: &reqwest::Client, url: &str, json: Option<&str>, timeout: Duration) -> Result<String, String> {
    let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)).unwrap_or_else(|| url.to_string());
    let mut resp = http
        .get(url)
        .header(reqwest::header::USER_AGENT, concat!("map/", env!("CARGO_PKG_VERSION")))
        .timeout(timeout)
        .send()
        .await
        .map_err(|e| if e.is_timeout() { format!("{host} timed out") } else { format!("could not reach {host}") })?;
    if !resp.status().is_success() {
        return Err(format!("{host} answered {}", resp.status()));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|_| format!("reading the answer from {host} failed"))? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BODY {
            return Err(format!("the answer from {host} is too large"));
        }
    }
    latest_from_body(&String::from_utf8_lossy(&body), json)
}

// ---- the installed version

fn substitute(command: &[String], binary: &str) -> Vec<String> {
    command.iter().map(|a| a.replace("{binary}", binary)).collect()
}

/// Runs the `version` command with the resolved binary, the workspace env, no stdin and a timeout.
async fn installed_version(hub: &Hub, name: &str, spec: &ProviderSpec, binary: &Path, timeout: Duration) -> Result<String, String> {
    let shown = substitute(&spec.version, &spec.binary).join(" ");
    let argv = substitute(&spec.version, &binary.to_string_lossy());
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // Isolated mode: the CLI reads its own config (an update channel, say) from MAP's folder for it, as in a launch.
    cmd.envs(hub.config.provider_env(name)).envs(&hub.config.workspace.env).env("NO_COLOR", "1");
    if hub.config.workspace.dir.is_dir() {
        cmd.current_dir(&hub.config.workspace.dir);
    }
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| format!("`{shown}` did not finish within {}s", timeout.as_secs()))?
        .map_err(|e| format!("could not run `{shown}`: {e}"))?;
    let text = crate::detect::strip_ansi(&format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)));
    match parse_version(&text) {
        Some(v) if out.status.success() => Ok(v),
        _ if !out.status.success() => Err(format!("`{shown}` failed ({})", out.status)),
        _ => Err(format!("no version in the output of `{shown}`")),
    }
}

// ---- can an update run from here

#[derive(Debug, PartialEq, Eq)]
enum NotWritable {
    ReadOnlyFs,
    Denied,
}

#[cfg(unix)]
fn writable(dir: &Path) -> Result<(), NotWritable> {
    use std::os::unix::ffi::OsStrExt;
    let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return Err(NotWritable::Denied) };
    // SAFETY: `path` is a valid NUL-terminated string that outlives the call; access(2) only reads it.
    if unsafe { libc::access(path.as_ptr(), libc::W_OK) } == 0 {
        return Ok(());
    }
    match std::io::Error::last_os_error().raw_os_error() {
        Some(libc::EROFS) => Err(NotWritable::ReadOnlyFs),
        _ => Err(NotWritable::Denied),
    }
}

#[cfg(not(unix))]
fn writable(dir: &Path) -> Result<(), NotWritable> {
    match std::fs::metadata(dir) {
        Ok(m) if !m.permissions().readonly() => Ok(()),
        _ => Err(NotWritable::Denied),
    }
}

/// Why an update started here would fail, judged from where the binary is: the directory of the command on PATH and
/// of the file it resolves to must both be writable by this user.
pub fn update_blocked(binary: &Path) -> Option<String> {
    let real = std::fs::canonicalize(binary).unwrap_or_else(|_| binary.to_path_buf());
    let mut dirs: Vec<&Path> = vec![];
    for dir in [binary.parent(), real.parent()].into_iter().flatten() {
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs.into_iter().find_map(|dir| match writable(dir) {
        Ok(()) => None,
        Err(NotWritable::ReadOnlyFs) => Some(format!("{} is installed read-only ({} is a read-only mount, as in the Docker deploy); update it on the host", binary.display(), dir.display())),
        Err(NotWritable::Denied) => Some(format!("{} is not writable by this user; update {} from a shell that can write there", dir.display(), binary.display())),
    })
}

// ---- state

pub fn session_name(config: &Config, provider: &str) -> String {
    format!("{}update-{provider}", config.workspace.session_prefix)
}

/// What a provider's row says before (or without) a check: its commands and URL, nothing measured.
fn blank(config: &Config, name: &str, spec: &ProviderSpec) -> UpdateInfo {
    UpdateInfo {
        provider: name.to_string(),
        installed: None,
        latest: None,
        available: false,
        checked_at: None,
        error: None,
        version_command: substitute(&spec.version, &spec.binary),
        latest_url: spec.latest.as_ref().map(latest_url),
        update_command: substitute(&spec.update, &spec.binary),
        update_blocked: None,
        updating: false,
        session: session_name(config, name),
    }
}

/// The latest report, the lock that serializes checks, and the newest version announced per provider.
pub struct UpdatesState {
    report: Mutex<UpdatesReport>,
    refresh: tokio::sync::Mutex<()>,
    announced: Mutex<BTreeMap<String, String>>,
    file: PathBuf,
}

impl UpdatesState {
    pub fn new(config: &Config) -> Self {
        let providers = crate::usage::ordered_providers(config).iter().filter_map(|n| Some(blank(config, n, config.providers.get(n)?))).collect();
        let report = UpdatesReport { enabled: config.updates.enabled, interval_secs: config.updates.interval_secs, checked_at: None, providers };
        let file = config.server.data_dir.join("updates.json");
        let announced = std::fs::read(&file)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|v| serde_json::from_value::<BTreeMap<String, String>>(v.get("announced")?.clone()).ok())
            .unwrap_or_default();
        Self { report: Mutex::new(report), refresh: tokio::sync::Mutex::new(()), announced: Mutex::new(announced), file }
    }

    pub fn report(&self) -> UpdatesReport {
        self.report.lock().unwrap().clone()
    }

    fn set_report(&self, report: UpdatesReport) {
        *self.report.lock().unwrap() = report;
    }

    fn set_provider(&self, info: UpdateInfo) -> UpdatesReport {
        let mut report = self.report.lock().unwrap();
        match report.providers.iter_mut().find(|p| p.provider == info.provider) {
            Some(p) => *p = info,
            None => report.providers.push(info),
        }
        report.clone()
    }

    /// Records and returns the available updates not announced before.
    fn take_new(&self, infos: &[UpdateInfo]) -> Vec<(String, String, String)> {
        let mut announced = self.announced.lock().unwrap();
        let fresh = newly_available(&mut announced, infos);
        if !fresh.is_empty() {
            let text = serde_json::json!({ "announced": *announced }).to_string();
            let tmp = self.file.with_extension("json.tmp");
            if let Err(err) = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, &self.file)) {
                tracing::warn!("saving {}: {err}", self.file.display());
            }
        }
        fresh
    }
}

/// (provider, installed, latest) for every available update whose `latest` is newer than the last one announced for
/// that provider; `announced` is updated. A release is announced once, however often it is seen.
pub fn newly_available(announced: &mut BTreeMap<String, String>, infos: &[UpdateInfo]) -> Vec<(String, String, String)> {
    let mut out = vec![];
    for info in infos.iter().filter(|i| i.available) {
        let (Some(latest), Some(installed)) = (&info.latest, &info.installed) else { continue };
        let seen = announced.get(&info.provider);
        if seen.is_some_and(|s| s == latest || compare(latest, s) != Some(Ordering::Greater)) {
            continue;
        }
        announced.insert(info.provider.clone(), latest.clone());
        out.push((info.provider.clone(), installed.clone(), latest.clone()));
    }
    out
}

/// Checks one provider now.
pub async fn check(hub: &Hub, name: &str) -> UpdateInfo {
    let Ok(spec) = hub.spec(name) else {
        let mut info = blank(&hub.config, name, &empty_spec());
        info.error = Some(format!("unknown provider `{name}`"));
        return info;
    };
    let mut info = blank(&hub.config, name, spec);
    let timeout = Duration::from_secs(hub.config.updates.timeout_secs);
    let binary = hub.binary_path(&spec.binary);
    let installed = async {
        match &binary {
            _ if spec.version.is_empty() => Ok(None),
            None => Err(format!("`{}` is not on the workspace PATH", spec.binary)),
            Some(b) => installed_version(hub, name, spec, b, timeout).await.map(Some),
        }
    };
    let latest = async {
        match (&spec.latest, &info.latest_url) {
            (Some(src), Some(url)) => fetch_latest(&hub.http, url, src.json.as_deref(), timeout).await.map(Some),
            _ => Ok(None),
        }
    };
    let (installed, latest) = tokio::join!(installed, latest);
    let mut errors = vec![];
    match installed {
        Ok(v) => info.installed = v,
        Err(e) => errors.push(e),
    }
    match latest {
        Ok(v) => info.latest = v,
        Err(e) => errors.push(e),
    }
    info.available = matches!((&info.installed, &info.latest), (Some(i), Some(l)) if newer(l, i));
    info.error = (!errors.is_empty()).then(|| errors.join("; "));
    info.update_blocked = match &binary {
        _ if spec.update.is_empty() => None,
        None => Some(format!("`{}` is not on the workspace PATH", spec.binary)),
        Some(b) => update_blocked(b),
    };
    info.updating = hub.term.exists(&info.session).await.unwrap_or(false);
    info.checked_at = Some(Utc::now());
    info
}

/// A spec with nothing configured, for a row about a provider that is not in `map.toml`.
fn empty_spec() -> ProviderSpec {
    ProviderSpec {
        binary: String::new(),
        models: vec![],
        default_model: String::new(),
        interactive: vec![],
        headless: vec![],
        extra_args: vec![],
        prompt_mode: crate::config::PromptMode::Send,
        startup_secs: 0,
        idle_markers: vec![],
        working_markers: vec![],
        auth_markers: vec![],
        trust_markers: vec![],
        trust_accept: vec![],
        trust: None,
        login: vec![],
        login_send: None,
        login_warning: None,
        credentials: vec![],
        api_key_env: None,
        version: vec![],
        latest: None,
        update: vec![],
        home_env: None,
        default_home: None,
        home_subdir: None,
        isolated_dir: None,
        home_keep: vec![],
    }
}

async fn announce_new(hub: &Hub, infos: &[UpdateInfo]) {
    for (provider, installed, latest) in hub.updates.take_new(infos) {
        hub.event(Level::Info, "update.available", None, None, format!("{provider} {latest} is available (installed: {installed})")).await;
    }
}

/// Polls every `updates.interval_secs`.
pub async fn run(hub: Arc<Hub>) {
    if !hub.config.updates.enabled {
        return;
    }
    let interval = Duration::from_secs(hub.config.updates.interval_secs);
    loop {
        refresh(&hub, Duration::ZERO).await;
        tokio::time::sleep(interval).await;
    }
}

/// Checks every provider (unless the last check is younger than `min_age`), stores the report, announces new
/// releases and pushes the report to the dashboard.
pub async fn refresh(hub: &Arc<Hub>, min_age: Duration) -> UpdatesReport {
    let _one_at_a_time = hub.updates.refresh.lock().await;
    let prev = hub.updates.report();
    let now = Utc::now();
    if let Some(at) = prev.checked_at
        && (now - at).to_std().unwrap_or_default() < min_age
    {
        return prev;
    }
    let names = crate::usage::ordered_providers(&hub.config);
    let providers = futures::future::join_all(names.iter().map(|n| check(hub, n))).await;
    let report = UpdatesReport { enabled: hub.config.updates.enabled, interval_secs: hub.config.updates.interval_secs, checked_at: Some(Utc::now()), providers };
    hub.updates.set_report(report.clone());
    announce_new(hub, &report.providers).await;
    hub.announce_updates(report.clone());
    report
}

/// Checks one provider again and pushes the report.
pub async fn recheck(hub: &Hub, provider: &str) -> UpdateInfo {
    let info = check(hub, provider).await;
    let report = hub.updates.set_provider(info.clone());
    announce_new(hub, std::slice::from_ref(&info)).await;
    hub.announce_updates(report);
    info
}

// ---- running an update

#[derive(Debug)]
pub enum StartError {
    /// An update is running already, or one from here would fail (read-only install).
    Conflict(String),
    Invalid(anyhow::Error),
}

/// The update command, wrapped so its exit status is printed and the pane stays a moment for the final capture.
fn wrapped(update: &[String]) -> Vec<String> {
    let script = format!("\"$@\"; s=$?; printf '\\n{EXIT_MARK}%s]\\n' \"$s\"; sleep 2; exit \"$s\"");
    ["/bin/sh".to_string(), "-c".into(), script, "sh".into()].into_iter().chain(update.iter().cloned()).collect()
}

/// The exit status the wrapper printed last in `pane`.
pub fn exit_status(pane: &str) -> Option<i32> {
    let at = pane.rfind(EXIT_MARK)?;
    pane[at + EXIT_MARK.len()..].split(']').next()?.trim().parse().ok()
}

/// Starts the provider's `update` command in its helper session and watches it; refused while one runs.
pub async fn start(hub: &Arc<Hub>, provider: &str) -> Result<Session, StartError> {
    let spec = hub.spec(provider).map_err(StartError::Invalid)?.clone();
    if spec.update.is_empty() {
        return Err(StartError::Invalid(anyhow::anyhow!("provider `{provider}` has no update command; set providers.{provider}.update in map.toml")));
    }
    let Some(binary) = hub.binary_path(&spec.binary) else {
        return Err(StartError::Invalid(anyhow::anyhow!("`{}` is not on the workspace PATH", spec.binary)));
    };
    if let Some(why) = update_blocked(&binary) {
        return Err(StartError::Conflict(why));
    }
    let name = session_name(&hub.config, provider);
    if hub.term.exists(&name).await.unwrap_or(false) {
        return Err(StartError::Conflict(format!("an update of {provider} is already running in `{name}`")));
    }
    let req = LaunchRequest { name: Some(name.clone()), provider: provider.into(), model: None, role: Role::Manual, task_id: None, project: None, prompt: None };
    let session = hub.launch_command(req, Some(&wrapped(&spec.update))).await.map_err(StartError::Invalid)?;
    let shown = substitute(&spec.update, &spec.binary).join(" ");
    hub.event(Level::Info, "provider.update", Some(&name), None, format!("Update of {provider} started: {shown}")).await;
    let mut row = hub.updates.report().providers.into_iter().find(|p| p.provider == provider).unwrap_or_else(|| blank(&hub.config, provider, &spec));
    row.updating = true;
    hub.announce_updates(hub.updates.set_provider(row));
    tokio::spawn(watch(Arc::clone(hub), provider.to_string(), name));
    Ok(session)
}

/// Follows the update session until it ends, saves its last pane, then checks the version again.
async fn watch(hub: Arc<Hub>, provider: String, name: String) {
    let before = hub.updates.report().providers.iter().find(|p| p.provider == provider).and_then(|p| p.installed.clone());
    let started = tokio::time::Instant::now();
    let mut last = String::new();
    loop {
        tokio::time::sleep(WATCH_EVERY).await;
        match hub.term.exists(&name).await {
            Ok(false) => break,
            Ok(true) if started.elapsed() > WATCH_FOR => return,
            _ => {}
        }
        if let Ok(text) = hub.term.capture(&name, 2000, false).await {
            last = crate::detect::strip_ansi(&text);
        }
    }
    if !last.trim().is_empty() {
        let dir = hub.run_dir(&name);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("final-pane.txt"), &last);
    }
    let info = recheck(&hub, &provider).await;
    let now = info.installed.as_deref().unwrap_or("unknown");
    let (level, message) = match (exit_status(&last), before.as_deref()) {
        (Some(code), _) if code != 0 => (Level::Warning, format!("Update of {provider} failed (exit {code}); its output is in the console of {name}")),
        (_, Some(old)) if info.installed.as_deref().is_some_and(|new| newer(new, old)) => (Level::Success, format!("{provider} updated: {old} → {now}")),
        (Some(_), _) => (Level::Info, format!("Update of {provider} finished; the version is {now}")),
        (None, _) => (Level::Warning, format!("Update session of {provider} ended without an exit status; the version is {now}")),
    };
    hub.event(level, "provider.update", Some(&name), None, message).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::fake::FakeTerminal;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn parses_the_three_version_outputs() {
        assert_eq!(parse_version("2.1.299 (Claude Code)\n").as_deref(), Some("2.1.299"));
        assert_eq!(parse_version("codex-cli 0.159.2\n").as_deref(), Some("0.159.2"));
        assert_eq!(parse_version("1.3.4").as_deref(), Some("1.3.4"));
        assert_eq!(parse_version("tool v1.0.0-beta.2 (build 7)").as_deref(), Some("1.0.0-beta.2"));
        assert_eq!(parse_version("x86_64 build abc1.2 then 3.4.5").as_deref(), Some("3.4.5"));
        assert_eq!(parse_version("no version here"), None);
        // Pane colours are stripped before parsing, as the version command does.
        assert_eq!(parse_version(&crate::detect::strip_ansi("\u{1b}[1m0.162.1\u{1b}[0m")).as_deref(), Some("0.162.1"));
    }

    #[test]
    fn versions_compare_numerically_with_prereleases_below_releases() {
        assert!(newer("1.10.0", "1.9.9"));
        assert!(newer("0.162.1", "0.159.2"));
        assert!(newer("2.1.300", "2.1.299"));
        assert!(!newer("1.3.4", "1.3.4"));
        assert!(!newer("1.3.4", "1.3.5"));
        assert_eq!(compare("1.2", "1.2.0"), Some(Ordering::Equal));
        assert!(newer("1.0.0", "1.0.0-rc.1"));
        assert!(!newer("1.0.0-rc.1", "1.0.0"));
        assert!(newer("1.0.0-rc.2", "1.0.0-rc.1"));
        assert!(newer("1.0.0-rc.10", "1.0.0-rc.9"));
        assert!(newer("1.0.0-beta", "1.0.0-alpha.5"));
        assert!(newer("1.0.0-alpha.1", "1.0.0-alpha"));
        assert!(newer("1.0.1-alpha", "1.0.0"));
        assert_eq!(compare("latest", "1.0.0"), None);
        assert!(!newer("garbage", "1.0.0"));
    }

    #[test]
    fn json_paths_and_bodies() {
        let v: Value = serde_json::from_str(r#"{"version":"1.3.5","dist-tags":{"latest":"0.162.1"},"list":[{"v":"9.0.1"}]}"#).unwrap();
        assert_eq!(json_path(&v, "version"), Some(&Value::from("1.3.5")));
        assert_eq!(json_path(&v, "dist-tags.latest"), Some(&Value::from("0.162.1")));
        assert_eq!(json_path(&v, "list.0.v"), Some(&Value::from("9.0.1")));
        assert_eq!(json_path(&v, "list.x"), None);
        assert_eq!(json_path(&v, "missing.key"), None);
        assert_eq!(latest_from_body("2.1.300\n", None).as_deref(), Ok("2.1.300"));
        assert_eq!(latest_from_body(r#"{"name":"@openai/codex","version":"0.162.1"}"#, Some("version")).as_deref(), Ok("0.162.1"));
        assert!(latest_from_body("<html>oops</html>", Some("version")).is_err());
        assert!(latest_from_body(r#"{"version": "soon"}"#, Some("version")).is_err());
        assert!(latest_from_body(r#"{"other": 1}"#, Some("version")).unwrap_err().contains("no `version`"));
    }

    #[test]
    fn url_placeholders() {
        let url = "https://x.run.app/manifests/{os}_{arch}.json";
        assert_eq!(expand_url(url, "linux", "x86_64", ""), "https://x.run.app/manifests/linux_amd64.json");
        assert_eq!(expand_url(url, "macos", "aarch64", ""), "https://x.run.app/manifests/darwin_arm64.json");
        assert_eq!(expand_url(url, "windows", "x86_64", ""), "https://x.run.app/manifests/windows_amd64.json");
        assert_eq!(expand_url("https://d/releases/{channel}", "linux", "x86_64", "stable"), "https://d/releases/stable");
    }

    #[test]
    fn channel_comes_from_the_settings_file_or_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        let src = ChannelSource { file: file.display().to_string(), key: "autoUpdatesChannel".into(), default: "latest".into() };
        assert_eq!(read_channel(&src), "latest");
        std::fs::write(&file, r#"{"autoUpdatesChannel": "stable", "other": 1}"#).unwrap();
        assert_eq!(read_channel(&src), "stable");
        std::fs::write(&file, r#"{"autoUpdatesChannel": "../../evil?x="}"#).unwrap();
        assert_eq!(read_channel(&src), "latest");
    }

    fn info(provider: &str, installed: &str, latest: &str) -> UpdateInfo {
        let mut i = blank(&Config::parse("[workspace]\ndir = \"/r\"\n", Path::new("/")).unwrap(), provider, &empty_spec());
        i.installed = Some(installed.into());
        i.latest = Some(latest.into());
        i.available = newer(latest, installed);
        i
    }

    #[test]
    fn each_release_is_announced_once() {
        let mut seen = BTreeMap::new();
        let first = newly_available(&mut seen, &[info("codex", "0.159.2", "0.162.1"), info("claude", "2.1.299", "2.1.299")]);
        assert_eq!(first, vec![("codex".to_string(), "0.159.2".to_string(), "0.162.1".to_string())]);
        assert!(newly_available(&mut seen, &[info("codex", "0.159.2", "0.162.1")]).is_empty());
        // An older latest (a channel switch) is not news either; a newer one is.
        assert!(newly_available(&mut seen, &[info("codex", "0.159.2", "0.160.0")]).is_empty());
        assert_eq!(newly_available(&mut seen, &[info("codex", "0.159.2", "0.163.0")]).len(), 1);
        assert_eq!(seen["codex"], "0.163.0");
    }

    #[cfg(unix)]
    #[test]
    fn a_read_only_install_blocks_the_update() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let cli = bin.join("cli");
        std::fs::write(&cli, "#!/bin/sh\n").unwrap();
        assert_eq!(update_blocked(&cli), None);
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o555)).unwrap();
        // SAFETY: geteuid has no preconditions. root ignores the mode bits, so the check cannot see them there.
        let root = unsafe { libc::geteuid() } == 0;
        let why = update_blocked(&cli);
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !root {
            assert!(why.unwrap().contains("not writable"));
        }
        // A link in a writable directory to a file in a read-only one is blocked too.
        let real = dir.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::write(real.join("cli"), "#!/bin/sh\n").unwrap();
        let link = bin.join("linked");
        std::os::unix::fs::symlink(real.join("cli"), &link).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o555)).unwrap();
        let why = update_blocked(&link);
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !root {
            assert!(why.unwrap().contains(&real.display().to_string()));
        }
    }

    #[test]
    fn exit_status_from_the_wrapper() {
        assert_eq!(exit_status("Updating…\n\n[map: update exited with status 0]\n"), Some(0));
        assert_eq!(exit_status("[map: update exited with status 1]\nagain\n[map: update exited with status 127]"), Some(127));
        assert_eq!(exit_status("no mark"), None);
        let w = wrapped(&["{binary}".into(), "update".into()]);
        assert_eq!(&w[..2], &["/bin/sh", "-c"]);
        assert_eq!(&w[3..], &["sh", "{binary}", "update"]);
    }

    /// A local HTTP server whose body can be changed; records each request's head.
    async fn server(body: Arc<Mutex<String>>, seen: Arc<Mutex<Vec<String>>>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 4096];
                let n = s.read(&mut buf).await.unwrap_or(0);
                seen.lock().unwrap().push(String::from_utf8_lossy(&buf[..n]).into_owned());
                let body = body.lock().unwrap().clone();
                let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = s.write_all(resp.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    fn fake_cli(dir: &Path, output: &str) -> PathBuf {
        let path = dir.join("fakecodex");
        let tmp = dir.join("fakecodex.tmp");
        std::fs::write(&tmp, format!("#!/bin/sh\necho '{output}'\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::rename(&tmp, &path).unwrap();
        path
    }

    async fn hub_with(dir: &Path, base: &str) -> (Arc<Hub>, Arc<FakeTerminal>) {
        let cli = fake_cli(dir, "codex-cli 0.159.2");
        let text = format!(
            "[workspace]\ndir = \"{0}\"\nsession_prefix = \"agp-\"\n[server]\ndata_dir = \"{0}/data\"\n[updates]\ntimeout_secs = 5\n\
             [providers.codex]\nbinary = \"{1}\"\nlatest = {{ url = \"{base}/codex/latest\", json = \"version\" }}\n\
             [providers.claude]\nbinary = \"/nonexistent/claude\"\nlatest = {{ url = \"\" }}\n\
             [providers.gemini]\nversion = []\nlatest = {{ url = \"\" }}\nupdate = []\n",
            dir.display(),
            cli.display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        std::fs::create_dir_all(&config.server.data_dir).unwrap();
        let term = Arc::new(FakeTerminal::default());
        (Hub::new(config, crate::store::Store::memory().await.unwrap(), term.clone()), term)
    }

    async fn update_events(hub: &Hub, kind: &str) -> Vec<String> {
        hub.store.events(200, None).await.unwrap().into_iter().filter(|e| e.kind == kind).map(|e| e.message).collect()
    }

    #[tokio::test]
    async fn checks_against_a_fake_cli_and_server_and_announces_once() {
        let dir = tempfile::tempdir().unwrap();
        let body = Arc::new(Mutex::new(r#"{"name":"@openai/codex","version":"0.162.1"}"#.to_string()));
        let seen = Arc::new(Mutex::new(vec![]));
        let base = server(body.clone(), seen.clone()).await;
        let (hub, _term) = hub_with(dir.path(), &base).await;

        let report = refresh(&hub, Duration::ZERO).await;
        let codex = report.providers.iter().find(|p| p.provider == "codex").unwrap();
        assert_eq!(codex.installed.as_deref(), Some("0.159.2"));
        assert_eq!(codex.latest.as_deref(), Some("0.162.1"));
        assert!(codex.available && codex.error.is_none(), "{codex:?}");
        assert_eq!(codex.update_command.last().map(String::as_str), Some("update"));
        assert_eq!(codex.update_blocked, None);
        assert_eq!(codex.session, "agp-update-codex");
        let claude = report.providers.iter().find(|p| p.provider == "claude").unwrap();
        assert!(claude.error.as_deref().unwrap().contains("not on the workspace PATH"));
        assert!(!claude.available && claude.latest.is_none());
        let gemini = report.providers.iter().find(|p| p.provider == "gemini").unwrap();
        assert!(gemini.installed.is_none() && gemini.latest.is_none() && gemini.error.is_none() && gemini.version_command.is_empty());

        // Nothing but a plain GET leaves the server: no credentials, no cookies.
        let head = seen.lock().unwrap()[0].to_lowercase();
        assert!(head.starts_with("get /codex/latest "), "{head}");
        assert!(!head.contains("authorization") && !head.contains("cookie"), "{head}");

        refresh(&hub, Duration::ZERO).await;
        assert_eq!(update_events(&hub, "update.available").await.len(), 1);
        // A restart remembers what was announced.
        let again = UpdatesState::new(&hub.config);
        assert_eq!(again.announced.lock().unwrap().get("codex").map(String::as_str), Some("0.162.1"));

        *body.lock().unwrap() = r#"{"version":"0.163.0"}"#.into();
        refresh(&hub, Duration::ZERO).await;
        let events = update_events(&hub, "update.available").await;
        assert_eq!(events.len(), 2);
        assert!(events[0].contains("0.163.0"), "{events:?}");
        // "Check now" right after a check returns that check.
        let at = hub.updates.report().checked_at;
        assert_eq!(refresh(&hub, DEBOUNCE).await.checked_at, at);
    }

    #[tokio::test]
    async fn update_runs_in_a_helper_session_and_rechecks_when_it_ends() {
        let dir = tempfile::tempdir().unwrap();
        let body = Arc::new(Mutex::new(r#"{"version":"0.162.1"}"#.to_string()));
        let base = server(body, Arc::default()).await;
        let (hub, term) = hub_with(dir.path(), &base).await;
        refresh(&hub, Duration::ZERO).await;

        let session = start(&hub, "codex").await.unwrap();
        assert_eq!(session.name, "agp-update-codex");
        let script = term.sessions.lock().unwrap()["agp-update-codex"].script.clone();
        assert!(script.contains("fakecodex update\n"), "{script}");
        assert!(hub.updates.report().providers.iter().any(|p| p.provider == "codex" && p.updating));
        assert!(matches!(start(&hub, "codex").await, Err(StartError::Conflict(_))), "a second update is refused");
        assert!(matches!(start(&hub, "gemini").await, Err(StartError::Invalid(_))), "no update command");
        assert!(matches!(start(&hub, "claude").await, Err(StartError::Invalid(_))), "binary missing");

        // The "update" installs 0.162.1, prints its status, and the session ends.
        term.set_pane("agp-update-codex", "installing…\n\n[map: update exited with status 0]\n");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        fake_cli(dir.path(), "codex-cli 0.162.1");
        term.exit("agp-update-codex");
        for _ in 0..50 {
            if !update_events(&hub, "provider.update").await.iter().any(|m| m.contains("updated")) {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        let events = update_events(&hub, "provider.update").await;
        assert!(events.iter().any(|m| m == "codex updated: 0.159.2 → 0.162.1"), "{events:?}");
        let codex = hub.updates.report().providers.into_iter().find(|p| p.provider == "codex").unwrap();
        assert_eq!(codex.installed.as_deref(), Some("0.162.1"));
        assert!(!codex.available && !codex.updating);
        let pane = std::fs::read_to_string(hub.run_dir("agp-update-codex").join("final-pane.txt")).unwrap();
        assert!(pane.contains("installing"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_read_only_install_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (hub, term) = hub_with(dir.path(), "http://127.0.0.1:9").await;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
        let result = start(&hub, "codex").await;
        let info = check(&hub, "codex").await;
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(result, Err(StartError::Conflict(ref why)) if why.contains("not writable")), "{result:?}");
        assert!(info.update_blocked.is_some());
        assert!(info.error.as_deref().unwrap().contains("127.0.0.1"), "the unreachable source is reported: {info:?}");
        assert!(term.sessions.lock().unwrap().is_empty());
    }
}
