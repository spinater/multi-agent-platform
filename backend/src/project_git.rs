//! Git in a project's folder, run by MAP on the user's behalf with MAP's gitconfig (`git_access.rs`): the folder's
//! status, fetch, fast-forward-only pull, push (never forced), connecting `origin` (initialising the repo first when
//! the folder is not one yet), cloning a repo into a new folder, and listing a remote's branches before a clone.
//! Every command has a timeout and never prompts.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, Utc};

use crate::git_access::{self, Run};
use crate::hub::Hub;
use crate::model::{Commit, GitOpResult, GitStatus, Level, SourceInspection, TaskStatus};

/// Local commands (status, init, remote) are quick; this bounds them all the same.
const LOCAL: Duration = Duration::from_secs(30);

/// A folder and how to run git in it.
pub struct Folder {
    pub dir: PathBuf,
    pub gitconfig: PathBuf,
    /// For commands that talk to a remote.
    pub timeout: Duration,
}

impl Folder {
    async fn git(&self, args: &[&str], net: bool) -> Result<Run> {
        let mut cmd = git_access::git_command(&self.gitconfig);
        cmd.current_dir(&self.dir).args(args);
        // Only the folder itself counts as a repo, never one further up (a projects root that is a repo).
        if let Some(parent) = self.dir.parent() {
            cmd.env("GIT_CEILING_DIRECTORIES", parent);
        }
        git_access::run(cmd, if net { self.timeout } else { LOCAL }).await
    }

    async fn text(&self, args: &[&str]) -> Option<String> {
        let run = self.git(args, false).await.ok()?;
        let out = run.output.trim().to_string();
        (run.ok && !out.is_empty()).then_some(out)
    }

    pub async fn status(&self) -> GitStatus {
        let mut s = GitStatus::default();
        let top = self.text(&["rev-parse", "--show-toplevel"]).await;
        let here = self.dir.canonicalize().unwrap_or_else(|_| self.dir.clone());
        if top.map(PathBuf::from).and_then(|t| t.canonicalize().ok()) != Some(here) {
            return s;
        }
        s.is_repo = true;
        s.branch = self.text(&["symbolic-ref", "--short", "-q", "HEAD"]).await;
        s.upstream = self.text(&["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"]).await;
        s.remote_url = self.text(&["remote", "get-url", "origin"]).await.map(|u| git_access::mask_secrets(&u));
        if s.upstream.is_some()
            && let Some(counts) = self.text(&["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]).await
        {
            let mut n = counts.split_whitespace().map(|x| x.parse::<u32>().ok());
            (s.ahead, s.behind) = (n.next().flatten(), n.next().flatten());
        }
        s.dirty = self.text(&["status", "--porcelain"]).await.map_or(0, |t| t.lines().count() as u32);
        if let Some(fetch_head) = self.text(&["rev-parse", "--git-path", "FETCH_HEAD"]).await {
            let path = self.dir.join(fetch_head);
            s.last_fetch = std::fs::metadata(path).and_then(|m| m.modified()).ok().map(DateTime::<Utc>::from);
        }
        s.last_commit = self.text(&["log", "-1", "--format=%H%x1f%an%x1f%ct%x1f%s"]).await.and_then(|line| {
            let mut f = line.split('\x1f');
            let (sha, author, at, subject) = (f.next()?, f.next()?, f.next()?, f.next().unwrap_or(""));
            Some(Commit { sha: sha.into(), author: author.into(), at: DateTime::<Utc>::from_timestamp(at.parse().ok()?, 0)?, subject: subject.into() })
        });
        s
    }
}

/// Why an operation did not run: refused (409, with a reason for the user) or a bad request.
#[derive(Debug)]
pub enum OpError {
    Refused(String),
    Invalid(anyhow::Error),
}

impl From<anyhow::Error> for OpError {
    fn from(err: anyhow::Error) -> Self {
        OpError::Invalid(err)
    }
}

fn refuse<T>(why: impl Into<String>) -> Result<T, OpError> {
    Err(OpError::Refused(why.into()))
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Op {
    Fetch,
    Pull,
    Push,
    Remote,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Fetch => "fetch",
            Op::Pull => "pull",
            Op::Push => "push",
            Op::Remote => "remote",
        }
    }
}

/// Collects the commands run and their output for the result.
#[derive(Default)]
struct Log {
    commands: Vec<String>,
    output: Vec<String>,
}

impl Log {
    async fn run(&mut self, f: &Folder, args: &[&str], net: bool) -> Result<bool> {
        self.commands.push(format!("git {}", args.join(" ")));
        let run = f.git(args, net).await?;
        if !run.output.is_empty() {
            self.output.push(run.output);
        }
        Ok(run.ok)
    }

    fn note(&mut self, text: impl Into<String>) {
        self.output.push(text.into());
    }
}

/// Runs one operation in the folder. `url` is the new `origin` for [`Op::Remote`].
pub async fn op(f: &Folder, op: Op, url: Option<&str>) -> Result<GitOpResult, OpError> {
    let before = f.status().await;
    let mut log = Log::default();
    let ok = match op {
        Op::Fetch => {
            if !before.is_repo {
                return refuse("the folder is not a git repo: connect a remote first");
            }
            if f.text(&["remote"]).await.is_none() {
                return refuse("the repo has no remote to fetch from: connect one first");
            }
            log.run(f, &["fetch", "--prune"], true).await?
        }
        Op::Pull => {
            let Some(branch) = before.branch.clone().filter(|_| before.is_repo) else {
                return refuse(if before.is_repo { "HEAD is detached: check out a branch first" } else { "the folder is not a git repo" });
            };
            if before.dirty > 0 {
                return refuse(format!("{} changed file(s) in the folder: commit or stash them first (pull only fast-forwards a clean folder)", before.dirty));
            }
            if before.upstream.is_none() {
                return refuse(format!("{branch} has no upstream yet: push it first, which sets one"));
            }
            if log.run(f, &["fetch", "--prune"], true).await? {
                let now = f.status().await;
                let (ahead, behind) = (now.ahead.unwrap_or(0), now.behind.unwrap_or(0));
                if ahead > 0 && behind > 0 {
                    return refuse(format!("{branch} and {} have diverged ({ahead} local, {behind} remote commits): merge or rebase by hand; MAP only fast-forwards", now.upstream.unwrap_or_default()));
                }
                if behind == 0 {
                    log.note("Already up to date.");
                    true
                } else {
                    log.run(f, &["merge", "--ff-only", "@{upstream}"], false).await?
                }
            } else {
                false
            }
        }
        Op::Push => {
            let Some(branch) = before.branch.clone().filter(|_| before.is_repo) else {
                return refuse(if before.is_repo { "HEAD is detached: check out a branch first" } else { "the folder is not a git repo" });
            };
            if before.last_commit.is_none() {
                return refuse(format!("{branch} has no commits yet: nothing to push"));
            }
            let remote = f.text(&["config", "--get", &format!("branch.{branch}.remote")]).await;
            let merge = f.text(&["config", "--get", &format!("branch.{branch}.merge")]).await;
            let src = format!("refs/heads/{branch}");
            match (remote, merge) {
                // An explicit refspec without `+`: config push refspecs or push.default never make it a force push.
                (Some(remote), Some(merge)) if before.upstream.is_some() => log.run(f, &["push", &remote, &format!("{src}:{merge}")], true).await?,
                _ => {
                    if before.remote_url.is_none() {
                        return refuse("the repo has no origin: connect a remote first");
                    }
                    log.run(f, &["push", "-u", "origin", &format!("{src}:{src}")], true).await?
                }
            }
        }
        Op::Remote => {
            let url = url.map(str::trim).unwrap_or_default();
            git_access::validate_url(url)?;
            if !before.is_repo && !log.run(f, &["init", "-b", "main"], false).await? {
                return Ok(result(f, log, false).await);
            }
            if f.text(&["remote", "get-url", "origin"]).await.is_some() {
                log.run(f, &["remote", "set-url", "origin", url], false).await?
            } else {
                log.run(f, &["remote", "add", "origin", url], false).await?
            }
        }
    };
    Ok(result(f, log, ok).await)
}

async fn result(f: &Folder, log: Log, ok: bool) -> GitOpResult {
    GitOpResult { ok, command: log.commands.join("\n"), output: log.output.join("\n"), status: f.status().await }
}

/// The repo's name from its URL: `git@github.com:me/app.git` → `app`.
pub fn repo_name(url: &str) -> String {
    let path = url.trim().trim_end_matches('/');
    let last = path.rsplit(['/', ':']).next().unwrap_or("");
    let name: String = last.trim_end_matches(".git").chars().filter(|c| c.is_ascii_alphanumeric() || "._-".contains(*c)).collect();
    let name = name.trim_start_matches(['.', '-']).to_string();
    if name.is_empty() { "repo".into() } else { name }
}

/// `git ls-remote` of a URL before cloning it: the default branch and the branches, or the error with a hint.
pub async fn inspect_remote(gitconfig: &Path, url: &str, branch: Option<&str>, timeout: Duration, has_key: bool) -> SourceInspection {
    let failed = |error: String, hint: Option<String>| SourceInspection { error: Some(error), hint, ..Default::default() };
    if let Err(err) = git_access::validate_url(url) {
        return failed(format!("{err:#}"), None);
    }
    let mut cmd = git_access::git_command(gitconfig);
    cmd.args(["ls-remote", "--symref", "--", url.trim()]);
    let run = match git_access::run(cmd, timeout).await {
        Ok(run) => run,
        Err(err) => return failed(format!("{err:#}"), None),
    };
    if !run.ok {
        // The whole (masked) output: the last line alone often hides why ("Permission denied" comes before it).
        let error = if run.output.trim().is_empty() { "git ls-remote failed".to_string() } else { run.output.trim().to_string() };
        return failed(error, git_access::auth_hint(&run.output, has_key));
    }
    let mut out = SourceInspection { ok: true, ..Default::default() };
    for line in run.output.lines() {
        if let Some(target) = line.strip_prefix("ref: ").and_then(|l| l.strip_suffix("\tHEAD")) {
            out.default_branch = target.strip_prefix("refs/heads/").map(String::from);
        } else if let Some((_, name)) = line.split_once('\t')
            && let Some(b) = name.strip_prefix("refs/heads/")
        {
            out.branches.push(b.to_string());
        }
    }
    if let Some(b) = branch.filter(|b| !b.is_empty())
        && !out.branches.iter().any(|x| x == b)
    {
        return failed(format!("the remote has no branch `{b}`"), None);
    }
    out
}

/// `git clone` (not bare) of `url` into `folder`, which must be missing or empty. Returns git's output.
pub async fn clone(gitconfig: &Path, url: &str, branch: Option<&str>, folder: &Path, timeout: Duration, has_key: bool) -> Result<String> {
    git_access::validate_url(url)?;
    crate::project_settings::validate_branch(branch)?;
    let mut cmd = git_access::git_command(gitconfig);
    cmd.arg("clone");
    if let Some(b) = branch {
        cmd.args(["--branch", b]);
    }
    cmd.arg("--").arg(url.trim()).arg(folder);
    let run = git_access::run(cmd, timeout).await?;
    if !run.ok {
        let hint = git_access::auth_hint(&run.output, has_key).map(|h| format!("\n{h}")).unwrap_or_default();
        bail!("git clone failed: {}{hint}", run.output.trim());
    }
    Ok(run.output)
}

// ---- with the hub: a project's folder, the running-task guard, events

pub async fn folder(hub: &Hub, slug: &str) -> Result<Folder> {
    let project = hub.store.project(slug).await?.ok_or_else(|| anyhow!("unknown project `{slug}`"))?;
    let dir = hub.workspace_of(&project);
    if !dir.is_dir() {
        bail!("the project's folder {} does not exist", dir.display());
    }
    Ok(Folder { dir, gitconfig: hub.gitconfig(), timeout: Duration::from_secs(hub.config.projects.git_timeout_secs) })
}

/// Tasks of the project that are running in its folder (like [`crate::projects::delete`] checks).
pub async fn busy(hub: &Hub, slug: &str) -> Result<Option<String>> {
    let live: Vec<String> = hub
        .store
        .tasks()
        .await?
        .into_iter()
        .filter(|t| t.project.as_deref() == Some(slug) && !t.status.is_terminal() && t.status != TaskStatus::Pending)
        .map(|t| format!("#{}", t.id))
        .collect();
    Ok((!live.is_empty()).then(|| format!("task{} {} running in this folder; wait for {} or cancel {}", if live.len() == 1 { "" } else { "s" }, live.join(", "), if live.len() == 1 { "it" } else { "them" }, if live.len() == 1 { "it" } else { "them" })))
}

pub async fn status(hub: &Hub, slug: &str) -> Result<GitStatus> {
    let f = folder(hub, slug).await?;
    let mut s = f.status().await;
    s.busy = busy(hub, slug).await?;
    Ok(s)
}

/// Runs `op` on the project's folder, refused while one of its tasks runs (except fetch), and records an event.
/// Connecting a remote also keeps its URL as the project's `source.remote`.
pub async fn run_op(hub: &Hub, slug: &str, op_kind: Op, url: Option<&str>) -> Result<GitOpResult, OpError> {
    let f = folder(hub, slug).await?;
    let blocked = busy(hub, slug).await?;
    if op_kind != Op::Fetch
        && let Some(why) = blocked.clone()
    {
        return refuse(why);
    }
    let mut result = op(&f, op_kind, url).await?;
    result.status.busy = blocked;
    if op_kind == Op::Remote
        && result.ok
        && let Some(mut p) = hub.store.project(slug).await?
    {
        p.settings.source.remote = url.map(|u| u.trim().to_string());
        hub.put_project(&p).await?;
    }
    let first = result.output.lines().rfind(|l| !l.trim().is_empty()).unwrap_or("").chars().take(160).collect::<String>();
    let (level, word) = if result.ok { (Level::Info, "ok") } else { (Level::Warning, "failed") };
    hub.event(level, &format!("project.git.{}", op_kind.name()), None, None, format!("{slug}: git {} {word}{}", op_kind.name(), if first.is_empty() { String::new() } else { format!(" · {first}") }))
        .await;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plain git for setting up fixtures, with a fixed identity and no global config.
    fn sh(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn commit(dir: &Path, file: &str, text: &str) {
        std::fs::write(dir.join(file), text).unwrap();
        sh(dir, &["add", "."]);
        sh(dir, &["commit", "-q", "-m", file]);
    }

    struct Fixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        bare: PathBuf,
        url: String,
        gitconfig: PathBuf,
    }

    /// A bare remote with one commit on `main`, and an empty MAP gitconfig with an identity.
    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let bare = root.join("remote.git");
        sh(&root, &["init", "-q", "--bare", "-b", "main", bare.to_str().unwrap()]);
        let seed = root.join("seed");
        std::fs::create_dir(&seed).unwrap();
        sh(&seed, &["init", "-q", "-b", "main"]);
        commit(&seed, "README.md", "hello\n");
        sh(&seed, &["push", "-q", bare.to_str().unwrap(), "main"]);
        let gitconfig = root.join("gitconfig");
        std::fs::write(&gitconfig, "[user]\n\tname = MAP test\n\temail = map@test\n").unwrap();
        let url = format!("file://{}", bare.display());
        Fixture { _tmp: tmp, root, bare, url, gitconfig }
    }

    fn folder_at(fx: &Fixture, dir: &Path) -> Folder {
        Folder { dir: dir.to_path_buf(), gitconfig: fx.gitconfig.clone(), timeout: Duration::from_secs(30) }
    }

    /// A second clone that pushes a commit to the remote, so the first one is behind.
    fn push_from_elsewhere(fx: &Fixture, file: &str) {
        let other = fx.root.join(format!("other-{file}"));
        sh(&fx.root, &["clone", "-q", &fx.url, other.to_str().unwrap()]);
        commit(&other, file, "theirs\n");
        sh(&other, &["push", "-q", "origin", "main"]);
    }

    #[test]
    fn repo_names() {
        assert_eq!(repo_name("git@github.com:me/app.git"), "app");
        assert_eq!(repo_name("https://gitlab.com/g/sub/my-lib/"), "my-lib");
        assert_eq!(repo_name("file:///tmp/remote.git"), "remote");
        assert_eq!(repo_name("git@host:x"), "x");
        assert_eq!(repo_name("https://h/.."), "repo");
    }

    #[tokio::test]
    async fn clones_inspects_and_reports_status() {
        let fx = fixture();
        let seen = inspect_remote(&fx.gitconfig, &fx.url, None, Duration::from_secs(30), false).await;
        assert!(seen.ok, "{seen:?}");
        assert_eq!((seen.default_branch.as_deref(), seen.branches.clone()), (Some("main"), vec!["main".to_string()]));
        assert!(!inspect_remote(&fx.gitconfig, &fx.url, Some("nope"), Duration::from_secs(30), false).await.ok);
        let missing = inspect_remote(&fx.gitconfig, "file:///nonexistent/r.git", None, Duration::from_secs(30), false).await;
        assert!(!missing.ok && missing.error.is_some());
        assert!(!inspect_remote(&fx.gitconfig, "ext::sh -c x", None, Duration::from_secs(5), false).await.ok);

        let dir = fx.root.join("work");
        std::fs::create_dir(&dir).unwrap();
        clone(&fx.gitconfig, &fx.url, Some("main"), &dir, Duration::from_secs(30), false).await.unwrap();
        assert!(dir.join("README.md").is_file() && dir.join(".git").is_dir(), "a working clone, not bare");
        assert!(clone(&fx.gitconfig, &fx.url, None, &dir, Duration::from_secs(30), false).await.is_err(), "not into a folder with files");

        let f = folder_at(&fx, &dir);
        let s = f.status().await;
        assert!(s.is_repo);
        assert_eq!((s.branch.as_deref(), s.upstream.as_deref(), s.ahead, s.behind, s.dirty), (Some("main"), Some("origin/main"), Some(0), Some(0), 0));
        assert_eq!(s.remote_url.as_deref(), Some(fx.url.as_str()));
        assert_eq!(s.last_commit.as_ref().unwrap().subject, "README.md");

        // Ahead by one local commit, behind by one after a fetch, and dirty with an untracked file.
        commit(&dir, "mine.txt", "mine\n");
        push_from_elsewhere(&fx, "theirs.txt");
        let r = op(&f, Op::Fetch, None).await.unwrap();
        assert!(r.ok, "{}", r.output);
        assert_eq!((r.status.ahead, r.status.behind), (Some(1), Some(1)));
        assert!(r.status.last_fetch.is_some());
        std::fs::write(dir.join("scratch.txt"), "x").unwrap();
        assert_eq!(f.status().await.dirty, 1);

        // Pull refuses a dirty folder, then a diverged branch, and never merges.
        let Err(OpError::Refused(why)) = op(&f, Op::Pull, None).await else { panic!("dirty pull ran") };
        assert!(why.contains("changed file"), "{why}");
        std::fs::remove_file(dir.join("scratch.txt")).unwrap();
        let Err(OpError::Refused(why)) = op(&f, Op::Pull, None).await else { panic!("diverged pull ran") };
        assert!(why.contains("diverged"), "{why}");
        assert_eq!(sh(&dir, &["log", "-1", "--format=%s"]), "mine.txt", "nothing merged");
    }

    #[tokio::test]
    async fn pulls_fast_forward_and_pushes_with_upstream() {
        let fx = fixture();
        let dir = fx.root.join("work");
        clone(&fx.gitconfig, &fx.url, None, &dir, Duration::from_secs(30), false).await.unwrap();
        let f = folder_at(&fx, &dir);
        push_from_elsewhere(&fx, "theirs.txt");
        let r = op(&f, Op::Pull, None).await.unwrap();
        assert!(r.ok && dir.join("theirs.txt").is_file(), "{}", r.output);
        assert_eq!(r.status.behind, Some(0));
        let again = op(&f, Op::Pull, None).await.unwrap();
        assert!(again.output.contains("Already up to date"), "{}", again.output);

        // A new branch: the first push sets its upstream; the identity comes from MAP's gitconfig.
        sh(&dir, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("f.txt"), "f").unwrap();
        let mut c = git_access::git_command(&fx.gitconfig);
        c.current_dir(&dir).args(["commit", "-q", "-am", "x"]);
        sh(&dir, &["add", "f.txt"]);
        let committed = git_access::run(c, LOCAL).await.unwrap();
        assert!(committed.ok, "{}", committed.output);
        assert_eq!(sh(&dir, &["log", "-1", "--format=%an"]), "MAP test");
        let r = op(&f, Op::Push, None).await.unwrap();
        assert!(r.ok, "{}", r.output);
        assert!(r.command.contains("push -u origin refs/heads/feature:refs/heads/feature"), "{}", r.command);
        assert_eq!(r.status.upstream.as_deref(), Some("origin/feature"));
        assert_eq!(sh(&fx.bare, &["log", "-1", "--format=%s", "feature"]), "x");

        // Behind the remote: the push is rejected rather than forced.
        let other = fx.root.join("other-feature");
        sh(&fx.root, &["clone", "-q", "-b", "feature", &fx.url, other.to_str().unwrap()]);
        commit(&other, "o.txt", "o");
        sh(&other, &["push", "-q", "origin", "feature"]);
        commit(&dir, "l.txt", "l");
        let r = op(&f, Op::Push, None).await.unwrap();
        assert!(!r.ok, "{}", r.output);
        assert!(r.command.contains("push origin refs/heads/feature:refs/heads/feature") && !r.command.contains('+'), "{}", r.command);
        assert_eq!(sh(&fx.bare, &["log", "-1", "--format=%s", "feature"]), "o.txt", "the remote keeps its commit");
    }

    #[tokio::test]
    async fn connects_a_remote_to_a_plain_folder() {
        let fx = fixture();
        let dir = fx.root.join("plain");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), "n").unwrap();
        let f = folder_at(&fx, &dir);
        assert!(!f.status().await.is_repo);
        assert!(matches!(op(&f, Op::Push, None).await, Err(OpError::Refused(_))));
        assert!(matches!(op(&f, Op::Remote, Some("--upload-pack=x")).await, Err(OpError::Invalid(_))));

        let r = op(&f, Op::Remote, Some(&fx.url)).await.unwrap();
        assert!(r.ok, "{}", r.output);
        assert_eq!(r.command, format!("git init -b main\ngit remote add origin {}", fx.url));
        assert!(r.status.is_repo && r.status.remote_url.as_deref() == Some(fx.url.as_str()));
        assert_eq!(r.status.branch.as_deref(), Some("main"));
        let other = fx.root.join("elsewhere.git");
        sh(&fx.root, &["init", "-q", "--bare", other.to_str().unwrap()]);
        let url2 = format!("file://{}", other.display());
        let r = op(&f, Op::Remote, Some(&url2)).await.unwrap();
        assert!(r.ok && r.command == format!("git remote set-url origin {url2}"), "{}", r.command);

        // A folder inside another repo is not that repo.
        let nested = fx.root.join("seed").join("sub");
        std::fs::create_dir(&nested).unwrap();
        assert!(!folder_at(&fx, &nested).status().await.is_repo);
    }
}
