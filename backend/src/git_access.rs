//! MAP's own git access, as a default for every project: an SSH key of its own (`<data_dir>/git/id_ed25519`, made
//! with `ssh-keygen`; only the public half ever leaves the server), a `known_hosts` that pins GitHub's and GitLab's
//! published host keys (other hosts are trusted on first use), a git identity, and a gitconfig that puts them
//! together. Agents get that gitconfig as `GIT_CONFIG_GLOBAL`, and MAP uses it for every git command it runs in a
//! project folder. The gitconfig includes the user's own `~/.gitconfig` after MAP's defaults, so their settings win;
//! a repo's own config wins over both. Nothing here reads or writes `~/.ssh`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use regex::Regex;
use tokio::process::Command;

use crate::model::{GitIdentity, GitKey, GitTestResult, KnownHost};

/// GitHub's and GitLab's SSH host keys, as published on 2026-10-14 at https://api.github.com/meta (`ssh_keys`) and
/// https://docs.gitlab.com/user/gitlab_com/#ssh-host-keys-fingerprints. Fingerprints, checked against both pages:
/// github.com ED25519 SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU, ECDSA SHA256:p2QAMXNIC1TJYWeIOttrVc98/R1BUFWu3/LiyKgUfQM,
/// RSA SHA256:uNiVztksCsDhcc0u9e8BujQXVUpKZIDTMczCvj3tD2s; gitlab.com ED25519 SHA256:eUXGGm1YGsMAS7vkcx6JOJdOGHPem5gQp4taiCfCLB8,
/// ECDSA SHA256:HbW3g8zUjNSksFbqTiUWPWg2Bq1x8xdGUrliXFzSnUw, RSA SHA256:ROQFvPThGrW4RuWLoL9tq9I9zJ42fK4XywyRtbOz/EQ.
/// If a host rotates its keys, update this list: ssh refuses a pinned host whose key changed.
pub const PINNED_KNOWN_HOSTS: &str = "\
github.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl
github.com ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBEmKSENjQEezOmxkZMy7opKgwFB9nkt5YRrYMjNuG5N87uRgg6CLrbo5wAdT/y6v0mKV0U2w0WZ2YB/++Tpockg=
github.com ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABgQCj7ndNxQowgcQnjshcLrqPEiiphnt+VTTvDP6mHBL9j1aNUkY4Ue1gvwnGLVlOhGeYrnZaMgRK6+PKCUXaDbC7qtbW8gIkhL7aGCsOr/C56SJMy/BCZfxd1nWzAOxSDPgVsmerOBYfNqltV9/hWCqBywINIR+5dIg6JTJ72pcEpEjcYgXkE2YEFXV1JHnsKgbLWNlhScqb2UmyRkQyytRLtL+38TGxkxCflmO+5Z8CSSNY7GidjMIZ7Q4zMjA2n1nGrlTDkzwDCsw+wqFPGQA179cnfGWOWRVruj16z6XyvxvjJwbz0wQZ75XK5tKSb7FNyeIEs4TT4jk+S4dhPeAUC5y+bDYirYgM4GC7uEnztnZyaVWQ7B381AK4Qdrwt51ZqExKbQpTUNn+EjqoTwvqNj4kqx5QUCI0ThS/YkOxJCXmPUWZbhjpCg56i+2aB6CmK2JGhn57K5mj0MNdBXA4/WnwH6XoPWJzK5Nyu2zB3nAZp+S5hpQs+p1vN1/wsjk=
gitlab.com ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAfuCHKVTjquxvt6CM6tdG4SLp1Btn/nOeHHE5UOzRdf
gitlab.com ecdsa-sha2-nistp256 AAAAE2VjZHNhLXNoYTItbmlzdHAyNTYAAAAIbmlzdHAyNTYAAABBBFSMqzJeV9rUzU4kWitGjeR4PWSa29SPqJ1fVkhtj3Hw9xjLVXVYrU9QlYWrOLXBpQ6KWjbjTDTdDkoohFzgbEY=
gitlab.com ssh-rsa AAAAB3NzaC1yc2EAAAADAQABAAABAQCsj2bNKTBSpIYDEGk9KxsGh3mySTRgMtXL583qmBpzeQ+jqCMRgBqB98u3z++J1sKlXHWfM9dyhSevkMwSbhoR8XIq/U0tCNyokEi/ueaBMCvbcTHhO7FcwzY92WK4Yt0aGROY5qX2UKSeOvuP4D6TPqKF1onrSzH9bx9XUf2lEdWT/ia1NEKjunUqu1xOB/StKDHMoX4/OKyIzuS0q/T1zOATthvasJFoPrAjkohTyaDUz2LN5JoH839hViyEG82yB+MjcFV5MU3N1l1QL3cVUCh93xSaua1N85qivl+siMkPGbO5xR/En4iEY6K2XPASUEMaieWVNTRCtJ4S8H+9
";

const PINNED_HOSTS: [&str; 2] = ["github.com", "gitlab.com"];

/// Transports MAP's own git commands may use; `ext::` and `fd::` (which run commands) are never among them.
const ALLOWED_PROTOCOLS: &str = "https:http:ssh:git:file";

/// The files under `<data_dir>/git/`.
#[derive(Debug, Clone)]
pub struct GitFiles {
    pub dir: PathBuf,
}

impl GitFiles {
    /// Absolute even for a relative `data_dir`: agents and git use these paths from other working directories.
    pub fn new(data_dir: &Path) -> Self {
        let data = std::path::absolute(data_dir).unwrap_or_else(|_| data_dir.to_path_buf());
        Self { dir: data.join("git") }
    }
    pub fn key(&self) -> PathBuf {
        self.dir.join("id_ed25519")
    }
    pub fn public_key(&self) -> PathBuf {
        self.dir.join("id_ed25519.pub")
    }
    pub fn known_hosts(&self) -> PathBuf {
        self.dir.join("known_hosts")
    }
    pub fn gitconfig(&self) -> PathBuf {
        self.dir.join("gitconfig")
    }
    fn identity_file(&self) -> PathBuf {
        self.dir.join("identity.json")
    }
}

// ---- validation

static SCP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([A-Za-z0-9._-]+)@([A-Za-z0-9.-]+):(.+)$").unwrap());
static HOST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(?:([A-Za-z0-9._-]+)@)?([A-Za-z0-9.-]+)(?::([0-9]{1,5}))?$").unwrap());

/// A git URL MAP may clone, fetch or connect: `https://`, `http://`, `ssh://`, `git://`, `file://` or
/// `user@host:path`. Never an option (`--upload-pack=…`), a transport that runs commands (`ext::`, `fd::`), a host
/// that starts with `-`, or an https password or token (the dashboard never takes secrets).
pub fn validate_url(url: &str) -> Result<()> {
    let url = url.trim();
    if url.is_empty() {
        bail!("a git source needs a URL");
    }
    if url.len() > 2000 || url.chars().any(|c| c.is_whitespace() || c.is_control()) || url.starts_with('-') {
        bail!("use an https://, ssh://, git:// or file:// URL, or user@host:path");
    }
    let authority = |rest: &str| rest.split('/').next().unwrap_or("").to_string();
    let host = if let Some(rest) = ["https://", "http://", "ssh://", "git://"].iter().find_map(|p| url.strip_prefix(p)) {
        let auth = authority(rest);
        let (user, hostport) = auth.rsplit_once('@').map_or((None, auth.as_str()), |(u, h)| (Some(u), h));
        if user.is_some_and(|u| u.contains(':')) {
            bail!("the URL carries a password or token; MAP never takes secrets: use the ssh URL with MAP's key");
        }
        let host = if let Some(v6) = hostport.strip_prefix('[') { v6.split(']').next().unwrap_or("") } else { hostport.split(':').next().unwrap_or("") };
        host.to_string()
    } else if url.starts_with("file://") {
        return Ok(());
    } else if let Some(c) = SCP.captures(url) {
        if c[3].starts_with('-') {
            bail!("the repo path cannot start with -");
        }
        c[2].to_string()
    } else {
        bail!("use an https://, ssh://, git:// or file:// URL, or user@host:path");
    };
    if host.is_empty() || host.starts_with('-') || host.starts_with('.') || !host.chars().all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c)) {
        bail!("`{host}` is not a host name");
    }
    Ok(())
}

/// A host for `ssh -T`: `github.com`, `git@gitlab.example.com`, `git.example.com:2222`.
#[derive(Debug, Clone, PartialEq)]
pub struct SshHost {
    pub user: String,
    pub host: String,
    pub port: Option<u16>,
}

impl SshHost {
    /// How `known_hosts` names it: `host`, or `[host]:port` for a port other than 22.
    pub fn known_hosts_name(&self) -> String {
        match self.port {
            Some(p) if p != 22 => format!("[{}]:{p}", self.host),
            _ => self.host.clone(),
        }
    }
}

pub fn parse_host(text: &str) -> Result<SshHost> {
    let text = text.trim();
    let bad = || anyhow::anyhow!("give a host name like github.com, gitlab.example.com:2222 or git@host (letters, digits, . and -)");
    let c = HOST.captures(text).ok_or_else(bad)?;
    let user = c.get(1).map_or("git", |m| m.as_str()).to_string();
    let host = c[2].to_string();
    if text.len() > 253 || host.starts_with(['-', '.']) || host.ends_with('.') || user.starts_with('-') || !host.contains(|ch: char| ch.is_ascii_alphabetic() || ch == '.') {
        return Err(bad());
    }
    let port = match c.get(3) {
        Some(p) => Some(p.as_str().parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(bad)?),
        None => None,
    };
    Ok(SshHost { user, host, port })
}

pub fn validate_identity(id: &GitIdentity) -> Result<()> {
    let name = id.name.trim();
    let email = id.email.trim();
    let clean = |s: &str| !s.chars().any(|c| c.is_control() || "<>\"\\".contains(c));
    if name.is_empty() != email.is_empty() {
        bail!("give both a name and an email, or neither");
    }
    if name.len() > 100 || !clean(name) {
        bail!("the name is at most 100 characters, without <, >, \", \\ or line breaks");
    }
    let at = email.split_once('@');
    if !email.is_empty() && (email.len() > 254 || !clean(email) || email.contains(char::is_whitespace) || !at.is_some_and(|(u, d)| !u.is_empty() && !d.is_empty())) {
        bail!("`{email}` is not an email address");
    }
    Ok(())
}

// ---- identity, gitconfig, known_hosts

pub fn read_identity(files: &GitFiles) -> GitIdentity {
    std::fs::read_to_string(files.identity_file()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

pub fn write_identity(files: &GitFiles, id: &GitIdentity) -> Result<GitIdentity> {
    validate_identity(id)?;
    let id = GitIdentity { name: id.name.trim().into(), email: id.email.trim().into() };
    make_dir(&files.dir)?;
    std::fs::write(files.identity_file(), serde_json::to_string_pretty(&id)?)?;
    ensure(files)?;
    Ok(id)
}

/// The user's own global git config files that exist: `~/.gitconfig`, then `$XDG_CONFIG_HOME/git/config` (or
/// `~/.config/git/config`). `GIT_CONFIG_GLOBAL` replaces both, so MAP's gitconfig includes them.
pub fn user_includes() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return vec![] };
    let xdg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".config"));
    [home.join(".gitconfig"), xdg.join("git/config")].into_iter().filter(|p| p.is_file()).collect()
}

/// A gitconfig value in double quotes, so `#`, `;` and spaces survive.
fn config_value(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// One word for the shell `core.sshCommand` runs through: as is when plain, else in single quotes.
fn shell_word(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_./=:@%+,-".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The ssh command MAP's key is used with. `BatchMode` so nothing ever waits on a prompt; first-use hosts are
/// recorded unhashed in MAP's own `known_hosts`, where pinned hosts are already listed.
pub fn ssh_options(key: &Path, known_hosts: &Path) -> Vec<String> {
    vec![
        "-i".into(),
        key.display().to_string(),
        "-o".into(),
        "IdentitiesOnly=yes".into(),
        "-o".into(),
        format!("UserKnownHostsFile={}", known_hosts.display()),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "HashKnownHosts=no".into(),
        "-o".into(),
        "BatchMode=yes".into(),
    ]
}

/// MAP's gitconfig: its defaults first (`[user]` when an identity is set, `[core] sshCommand` when the key exists),
/// then `[include]` of the user's own global files, so theirs win. A repo's own config wins over both.
pub fn render_gitconfig(identity: &GitIdentity, key: Option<&Path>, known_hosts: &Path, includes: &[PathBuf]) -> String {
    let mut out = String::from(
        "# Written by MAP (multi-agent-platform) and regenerated on every change: edit the Git access panel instead.\n\
         # Agents get this file as GIT_CONFIG_GLOBAL. Your own global config is included below, so its settings win.\n",
    );
    if !identity.name.is_empty() {
        out.push_str(&format!("[user]\n\tname = {}\n\temail = {}\n", config_value(&identity.name), config_value(&identity.email)));
    }
    if let Some(key) = key {
        let opts = ssh_options(key, known_hosts).iter().map(|o| shell_word(o)).collect::<Vec<_>>().join(" ");
        out.push_str(&format!("[core]\n\tsshCommand = {}\n", config_value(&format!("ssh {opts}"))));
    }
    for path in includes {
        out.push_str(&format!("[include]\n\tpath = {}\n", config_value(&path.display().to_string())));
    }
    out
}

fn make_dir(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn write_if_changed(path: &Path, text: &str) -> Result<()> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// The hosts a `known_hosts` line names (`github.com,140.82.112.3`, `[h]:2222`); empty for comments and markers.
fn line_hosts(line: &str) -> Vec<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with('@') {
        return vec![];
    }
    line.split_whitespace().next().map(|h| h.split(',').collect()).unwrap_or_default()
}

/// MAP's `known_hosts`: the pinned keys first, then every first-use entry for other hosts (entries naming a pinned
/// host are dropped, so only the published keys count for it).
pub fn render_known_hosts(existing: &str) -> String {
    let mut out = String::from(PINNED_KNOWN_HOSTS);
    for line in existing.lines() {
        let hosts = line_hosts(line);
        if !hosts.is_empty() && !hosts.iter().any(|h| PINNED_HOSTS.contains(h)) {
            out.push_str(line.trim());
            out.push('\n');
        }
    }
    out
}

/// Creates `<data_dir>/git/` (0700) and brings `known_hosts` and `gitconfig` up to date; returns the gitconfig's
/// path. Cheap when nothing changed, so it runs before every launch and every git command MAP runs.
pub fn ensure(files: &GitFiles) -> Result<PathBuf> {
    make_dir(&files.dir)?;
    let kh = files.known_hosts();
    write_if_changed(&kh, &render_known_hosts(&std::fs::read_to_string(&kh).unwrap_or_default()))?;
    let key = files.key();
    let text = render_gitconfig(&read_identity(files), key.is_file().then_some(key.as_path()), &kh, &user_includes());
    let path = files.gitconfig();
    write_if_changed(&path, &text)?;
    Ok(path)
}

// ---- running git and ssh

/// Output of a command MAP ran: success, and stdout then stderr, with any URL password masked.
#[derive(Debug, Clone, Default)]
pub struct Run {
    pub ok: bool,
    pub output: String,
}

static URL_PASSWORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"://([^/@:\s]+):([^/@\s]+)@").unwrap());

/// `https://user:secret@host` → `https://user:***@host`.
pub fn mask_secrets(text: &str) -> String {
    URL_PASSWORD.replace_all(text, "://$1:***@").into_owned()
}

/// `git` for a project folder (or a clone) with MAP's gitconfig: no prompts (`GIT_TERMINAL_PROMPT=0`, no askpass,
/// and no controlling terminal for ssh), only the allowed transports, English messages. Callers add a timeout
/// through [`run`].
pub fn git_command(gitconfig: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_CONFIG_GLOBAL", gitconfig)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ALLOW_PROTOCOL", ALLOWED_PROTOCOLS)
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .env("LC_ALL", "C");
    cmd
}

/// Runs `cmd` without stdin, in a session of its own, and kills its whole process group (ssh included) after
/// `timeout`.
pub async fn run(mut cmd: Command, timeout: Duration) -> Result<Run> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    #[cfg(unix)]
    // SAFETY: setsid is async-signal-safe, and nothing else runs between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let child = cmd.spawn().context("starting git (is it installed?)")?;
    let pid = child.id();
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(out) => {
            let out = out?;
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            let err = String::from_utf8_lossy(&out.stderr);
            if !err.trim().is_empty() {
                if !text.is_empty() && !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push_str(&err);
            }
            // ssh ends lines with \r\n, and progress meters redraw with \r.
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            Ok(Run { ok: out.status.success(), output: tail(&mask_secrets(text.trim_end()), 20_000) })
        }
        Err(_) => {
            #[cfg(unix)]
            if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
                // SAFETY: a plain kill(2) of the process group the child leads.
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            Ok(Run { ok: false, output: format!("timed out after {}s", timeout.as_secs()) })
        }
    }
}

fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

pub fn on_path(binary: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(binary).is_file()))
}

// ---- the key

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname NUL-terminates or truncates within it.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let name = if ok { String::from_utf8_lossy(&buf[..buf.iter().position(|b| *b == 0).unwrap_or(buf.len())]).into_owned() } else { String::new() };
    let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || ".-".contains(*c)).collect();
    if name.is_empty() { "localhost".into() } else { name }
}

async fn fingerprints(file: &Path) -> Vec<(String, String, String)> {
    let mut cmd = Command::new("ssh-keygen");
    cmd.arg("-l").arg("-f").arg(file);
    let Ok(run) = run(cmd, Duration::from_secs(10)).await else { return vec![] };
    if !run.ok {
        return vec![];
    }
    // `256 SHA256:… github.com (ED25519)`, one per key.
    run.output
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (_bits, fp) = (f.next()?, f.next()?);
            let rest: Vec<&str> = f.collect();
            let (kind, names) = rest.split_last()?;
            Some((names.join(" "), fp.to_string(), kind.trim_matches(['(', ')']).to_string()))
        })
        .collect()
}

/// MAP's public key, read from the `.pub` file only: the private key is never read.
pub async fn key(files: &GitFiles) -> Option<GitKey> {
    let public = std::fs::read_to_string(files.public_key()).ok()?;
    let public = public.trim().to_string();
    if !public.starts_with("ssh-ed25519 ") || public.contains("PRIVATE") || !files.key().is_file() {
        return None;
    }
    let comment = public.split_whitespace().nth(2).unwrap_or("").to_string();
    let fingerprint = fingerprints(&files.public_key()).await.into_iter().next().map(|(_, fp, _)| fp).unwrap_or_default();
    let created_at = std::fs::metadata(files.key()).and_then(|m| m.modified()).map(DateTime::<Utc>::from).unwrap_or_else(|_| Utc::now());
    Some(GitKey { public_key: public, fingerprint, created_at, comment })
}

/// Makes MAP's ed25519 key (no passphrase, so agents never wait on one). An existing key is refused unless
/// `replace`, which moves it aside to `id_ed25519.old-<time>` (and `.pub`) rather than deleting it.
pub async fn create_key(files: &GitFiles, replace: bool) -> Result<GitKey> {
    if !on_path("ssh-keygen") {
        bail!("ssh-keygen is not installed on this machine (package openssh-client)");
    }
    make_dir(&files.dir)?;
    let (private, public) = (files.key(), files.public_key());
    if private.exists() || public.exists() {
        if !replace {
            bail!("MAP already has a key; replace it explicitly (the old one is kept as id_ed25519.old-…)");
        }
        let stamp = Utc::now().format("%Y%m%d-%H%M%S");
        for (from, to) in [(&private, format!("id_ed25519.old-{stamp}")), (&public, format!("id_ed25519.old-{stamp}.pub"))] {
            if from.exists() {
                std::fs::rename(from, files.dir.join(to)).with_context(|| format!("moving {} aside", from.display()))?;
            }
        }
    }
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-q", "-t", "ed25519", "-N", "", "-C", &format!("map@{}", hostname()), "-f"]).arg(&private);
    let out = run(cmd, Duration::from_secs(30)).await?;
    if !out.ok {
        bail!("ssh-keygen failed: {}", out.output);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o600))?;
    }
    ensure(files)?;
    key(files).await.context("the new key could not be read back")
}

/// Every key in MAP's `known_hosts`, with whether it is one of the pinned, published ones.
pub async fn known_hosts(files: &GitFiles) -> Vec<KnownHost> {
    let kh = files.known_hosts();
    if !kh.is_file() {
        return vec![];
    }
    fingerprints(&kh)
        .await
        .into_iter()
        .map(|(host, fingerprint, key_type)| {
            let pinned = host.split(',').any(|h| PINNED_HOSTS.contains(&h));
            KnownHost { host, key_type, fingerprint, pinned }
        })
        .collect()
}

/// The host greeted the key: GitHub (exit 1) "successfully authenticated", GitLab "Welcome to GitLab", Gitea and
/// Bitbucket's own wording.
pub fn greeted(output: &str, exit_ok: bool) -> bool {
    let o = output.to_lowercase();
    exit_ok || o.contains("successfully authenticated") || o.contains("welcome to gitlab") || o.contains("authenticated via ssh key") || o.contains("logged in as")
}

/// `ssh -T git@<host>` with MAP's key alone (`-F /dev/null`: the user's own ssh config and keys play no part), so it
/// answers whether the host accepts MAP's key.
pub async fn test(files: &GitFiles, host: &SshHost, timeout: Duration) -> Result<GitTestResult> {
    if files.key().is_file() {
        ensure(files)?;
    } else {
        bail!("MAP has no SSH key yet: create one first");
    }
    let mut args: Vec<String> = vec!["-F".into(), "/dev/null".into()];
    args.extend(ssh_options(&files.key(), &files.known_hosts()));
    args.extend(["-o".into(), format!("ConnectTimeout={}", timeout.as_secs().clamp(1, 20)), "-T".into()]);
    if let Some(p) = host.port {
        args.extend(["-p".into(), p.to_string()]);
    }
    args.push(format!("{}@{}", host.user, host.host));
    let mut cmd = Command::new("ssh");
    cmd.args(&args).env("LC_ALL", "C").env("SSH_ASKPASS_REQUIRE", "never");
    let out = run(cmd, timeout).await?;
    let shown = format!("ssh -T {}{}@{}", host.port.map(|p| format!("-p {p} ")).unwrap_or_default(), host.user, host.host);
    let name = host.known_hosts_name();
    let host_keys = known_hosts(files).await.into_iter().filter(|k| k.host.split(',').any(|h| h == name)).collect();
    Ok(GitTestResult { ok: greeted(&out.output, out.ok), host: name, command: shown, output: out.output, host_keys })
}

/// A friendly next step for a failed clone, ls-remote, fetch or push.
pub fn auth_hint(output: &str, has_key: bool) -> Option<String> {
    let o = output.to_lowercase();
    if o.contains("host key verification failed") || o.contains("remote host identification has changed") {
        return Some("The host's key could not be confirmed or has changed: check its fingerprint under Git access before trusting it.".into());
    }
    if o.contains("permission denied (publickey") || o.contains("could not read from remote repository") {
        return Some(if has_key {
            "The host refused MAP's key: add MAP's public key (Projects → Git access) to your GitHub or GitLab account, or as a deploy key of the repo (with write access to push), then try again.".into()
        } else {
            "Create MAP's SSH key under Projects → Git access, add its public key to your GitHub or GitLab account (or as a deploy key of the repo), then try again.".into()
        });
    }
    if o.contains("could not read username") || o.contains("terminal prompts disabled") || o.contains("authentication failed") {
        return Some("This https URL needs a sign-in, and MAP never takes tokens or passwords: use the repo's ssh URL (git@host:owner/repo.git) with MAP's key.".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        for ok in ["https://github.com/a/b.git", "git@github.com:a/b.git", "ssh://git@gitlab.example.com:2222/g/r.git", "file:///tmp/r.git", "git://h.example/r", "http://[::1]:8080/r.git"] {
            assert!(validate_url(ok).is_ok(), "{ok}");
        }
        for bad in [
            "--upload-pack=touch /tmp/x",
            "ext::sh -c touch% /tmp/x",
            "fd::3",
            "https://user:ghp_secret@github.com/a/b.git",
            "git@-oProxyCommand=x:a/b",
            "git@github.com:-u/x",
            "ssh://-oProxyCommand=x/r",
            "/tmp/plain/path",
            "github.com/a/b",
            "https://github.com/a b",
            "",
        ] {
            assert!(validate_url(bad).is_err(), "{bad}");
        }
        assert!(validate_url("https://git@github.com/a/b.git").is_ok(), "a user name alone is not a secret");
    }

    #[test]
    fn hosts() {
        assert_eq!(parse_host("github.com").unwrap(), SshHost { user: "git".into(), host: "github.com".into(), port: None });
        let h = parse_host("me@git.example.com:2222").unwrap();
        assert_eq!((h.user.as_str(), h.port, h.known_hosts_name()), ("me", Some(2222), "[git.example.com]:2222".to_string()));
        for bad in ["-oProxyCommand=x", "git@-x", "host;rm", "a b", "host:0", "host:99999", "", ".x", "x."] {
            assert!(parse_host(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn identities() {
        assert!(validate_identity(&GitIdentity { name: "Ada L".into(), email: "ada@example.com".into() }).is_ok());
        assert!(validate_identity(&GitIdentity::default()).is_ok(), "unset");
        assert!(validate_identity(&GitIdentity { name: "Ada".into(), email: String::new() }).is_err());
        assert!(validate_identity(&GitIdentity { name: "Ada\n[core]".into(), email: "a@b".into() }).is_err());
        assert!(validate_identity(&GitIdentity { name: "Ada".into(), email: "not an email".into() }).is_err());
    }

    #[test]
    fn gitconfig_puts_map_defaults_before_the_users_includes() {
        let kh = Path::new("/data/git/known_hosts");
        let bare = render_gitconfig(&GitIdentity::default(), None, kh, &[]);
        assert!(!bare.contains("[user]") && !bare.contains("sshCommand") && !bare.contains("[include]"), "{bare}");

        let id = GitIdentity { name: "Ada \"L\"".into(), email: "ada@example.com".into() };
        let includes = [PathBuf::from("/home/u/.gitconfig"), PathBuf::from("/home/u/.config/git/config")];
        let text = render_gitconfig(&id, Some(Path::new("/data/my git/id_ed25519")), kh, &includes);
        assert!(text.contains("\tname = \"Ada \\\"L\\\"\"\n\temail = \"ada@example.com\""), "{text}");
        assert!(text.contains("sshCommand = \"ssh -i '/data/my git/id_ed25519' -o IdentitiesOnly=yes -o UserKnownHostsFile=/data/git/known_hosts -o StrictHostKeyChecking=accept-new"), "{text}");
        let (user, core, inc) = (text.find("[user]").unwrap(), text.find("[core]").unwrap(), text.find("[include]").unwrap());
        assert!(user < inc && core < inc, "the user's own settings come last, so they win");
        assert_eq!(text.matches("[include]").count(), 2);

        // git itself reads it: the user's include overrides MAP's identity.
        let dir = tempfile::tempdir().unwrap();
        let mine = dir.path().join("mine");
        std::fs::write(&mine, "[user]\n\tname = Mine\n").unwrap();
        let cfg = dir.path().join("gitconfig");
        std::fs::write(&cfg, render_gitconfig(&id, Some(Path::new("/k")), kh, &[mine])).unwrap();
        let get = |key: &str| {
            let out = std::process::Command::new("git").args(["config", "--global", "--includes", "--get", key]).env("GIT_CONFIG_GLOBAL", &cfg).output().unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        assert_eq!(get("user.name"), "Mine");
        assert_eq!(get("user.email"), "ada@example.com");
        assert!(get("core.sshCommand").starts_with("ssh -i /k -o IdentitiesOnly=yes"));
    }

    #[test]
    fn known_hosts_keep_pins_and_first_use_entries() {
        let existing = "github.com ssh-ed25519 AAAAstale\ngit.example.com ssh-ed25519 AAAAtofu\n# comment\n";
        let text = render_known_hosts(existing);
        assert!(text.starts_with(PINNED_KNOWN_HOSTS));
        assert!(!text.contains("AAAAstale"), "only the published keys count for a pinned host");
        assert!(text.ends_with("git.example.com ssh-ed25519 AAAAtofu\n"));
        assert_eq!(render_known_hosts(&text), text, "stable");
    }

    #[test]
    fn masks_and_hints() {
        assert_eq!(mask_secrets("fatal: https://me:tok3n@github.com/a/b"), "fatal: https://me:***@github.com/a/b");
        assert!(greeted("Hi ada! You've successfully authenticated, but GitHub does not provide shell access.", false));
        assert!(greeted("Welcome to GitLab, @ada!", false));
        assert!(!greeted("git@github.com: Permission denied (publickey).", false));
        assert!(auth_hint("git@github.com: Permission denied (publickey).", true).unwrap().contains("deploy key"));
        assert!(auth_hint("fatal: could not read Username for 'https://github.com': terminal prompts disabled", false).unwrap().contains("ssh URL"));
        assert!(auth_hint("fatal: repository not found", true).is_none());
    }

    #[tokio::test]
    async fn creates_a_key_refuses_a_second_and_replaces_it_aside() {
        if !on_path("ssh-keygen") {
            eprintln!("skipped: ssh-keygen is not installed");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let files = GitFiles::new(dir.path());
        assert!(key(&files).await.is_none());
        let made = create_key(&files, false).await.unwrap();
        assert!(made.public_key.starts_with("ssh-ed25519 AAAA") && made.public_key.ends_with(&made.comment));
        assert!(made.comment.starts_with("map@"));
        assert!(made.fingerprint.starts_with("SHA256:"));
        let json = serde_json::to_string(&made).unwrap();
        assert!(!json.contains("PRIVATE") && !json.contains("BEGIN OPENSSH"), "never the private key: {json}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(files.key()).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&files.dir).unwrap().permissions().mode() & 0o777, 0o700);
        }
        let config = std::fs::read_to_string(files.gitconfig()).unwrap();
        assert!(config.contains("sshCommand") && config.contains(&files.key().display().to_string()), "{config}");
        assert!(std::fs::read_to_string(files.known_hosts()).unwrap().starts_with(PINNED_KNOWN_HOSTS));
        let pinned = known_hosts(&files).await;
        assert_eq!(pinned.iter().filter(|k| k.pinned).count(), 6, "{pinned:?}");
        assert!(pinned.iter().any(|k| k.host == "github.com" && k.fingerprint == "SHA256:+DiY3wvvV6TuJJhbpZisF/zLDA0zPMSvHdkr4UvCOqU"));

        let err = create_key(&files, false).await.unwrap_err().to_string();
        assert!(err.contains("already has a key"), "{err}");
        assert_eq!(key(&files).await.unwrap().public_key, made.public_key, "unchanged");

        let again = create_key(&files, true).await.unwrap();
        assert_ne!(again.public_key, made.public_key);
        let old: Vec<String> = std::fs::read_dir(&files.dir).unwrap().filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n.starts_with("id_ed25519.old-")).collect();
        assert_eq!(old.len(), 2, "the old key and its .pub are kept: {old:?}");
        let old_pub = old.iter().find(|n| n.ends_with(".pub")).unwrap();
        assert_eq!(std::fs::read_to_string(files.dir.join(old_pub)).unwrap().trim(), made.public_key);
    }

    #[tokio::test]
    async fn identity_round_trip_regenerates_the_gitconfig() {
        let dir = tempfile::tempdir().unwrap();
        let files = GitFiles::new(dir.path());
        ensure(&files).unwrap();
        assert!(!std::fs::read_to_string(files.gitconfig()).unwrap().contains("[user]"));
        assert!(write_identity(&files, &GitIdentity { name: "x".into(), email: "bad".into() }).is_err());
        write_identity(&files, &GitIdentity { name: " Ada ".into(), email: "ada@example.com".into() }).unwrap();
        assert_eq!(read_identity(&files).name, "Ada");
        assert!(std::fs::read_to_string(files.gitconfig()).unwrap().contains("name = \"Ada\""));
        assert!(test(&files, &parse_host("github.com").unwrap(), Duration::from_secs(1)).await.is_err(), "no key: no ssh at all");
    }
}
