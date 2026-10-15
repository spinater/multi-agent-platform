//! A branch and git worktree per task (`branches.mode = worktree`): the task's agent, verify, audit and hooks run in
//! `<data_dir>/projects/<slug>/worktrees/task-<id>` on branch `map/task-<id>`, made from the project's base branch,
//! so several tasks of one project run at once without touching each other's files. When the task passes, MAP
//! commits what the agent left, then lands the branch: rebase onto the base and fast-forward it (one landing at a
//! time per project), push it and open a pull request, or keep it for the owner. Every git command uses MAP's
//! gitconfig and never prompts.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::git_access::{self, Run};
use crate::hub::Hub;
use crate::model::{BranchMode, LandMode, ProjectBranches, Task, TaskGit};

const LOCAL: Duration = Duration::from_secs(60);
const NET: Duration = Duration::from_secs(180);

pub fn branch_name(id: i64) -> String {
    format!("map/task-{id}")
}

pub fn worktree_path(hub: &Hub, slug: &str, id: i64) -> PathBuf {
    hub.project_dir(slug).join("worktrees").join(format!("task-{id}"))
}

/// The project's branch settings for `task`; `None` for the default workspace or a shared-folder project.
pub async fn settings(hub: &Hub, task: &Task) -> Option<ProjectBranches> {
    let slug = task.project.as_deref()?;
    let b = hub.store.project(slug).await.ok().flatten()?.settings.branches;
    (b.mode == BranchMode::Worktree).then_some(b)
}

/// Landings of one project run one at a time, so the base never moves between a rebase and its fast-forward.
pub async fn land_lock(slug: &str) -> tokio::sync::OwnedMutexGuard<()> {
    static LOCKS: LazyLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = LazyLock::new(Default::default);
    let lock = Arc::clone(LOCKS.lock().unwrap().entry(slug.to_string()).or_default());
    lock.lock_owned().await
}

/// A relative path inside the project, for `copy`.
pub fn validate_copy(path: &str) -> Result<()> {
    let p = Path::new(path);
    if path.is_empty() || p.is_absolute() || p.components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
        bail!("copy entries are paths inside the project folder, without `..`: `{path}`");
    }
    if p.components().next().is_some_and(|c| c.as_os_str() == ".git") {
        bail!("`.git` cannot be copied");
    }
    Ok(())
}

/// git in `dir` with MAP's gitconfig.
pub struct Git {
    pub dir: PathBuf,
    gitconfig: PathBuf,
}

impl Git {
    pub fn new(hub: &Hub, dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into(), gitconfig: hub.gitconfig() }
    }

    pub async fn run(&self, args: &[&str], timeout: Duration) -> Result<Run> {
        let mut cmd = git_access::git_command(&self.gitconfig);
        cmd.current_dir(&self.dir).args(args);
        git_access::run(cmd, timeout).await
    }

    /// The trimmed output of a command that succeeded with output.
    pub async fn text(&self, args: &[&str]) -> Option<String> {
        let run = self.run(args, LOCAL).await.ok()?;
        let out = run.output.trim().to_string();
        (run.ok && !out.is_empty()).then_some(out)
    }

    async fn ok(&self, args: &[&str]) -> Result<()> {
        let run = self.run(args, LOCAL).await?;
        if !run.ok {
            bail!("git {} failed: {}", args.join(" "), crate::decisions::last_line(&run.output));
        }
        Ok(())
    }
}

/// Why a worktree could not be made: the project cannot have worktrees at all (`Fatal`, a person must change the
/// repo or the setting) or this attempt failed (`Retry`, e.g. a setup command).
#[derive(Debug)]
pub enum PrepareError {
    Fatal(String),
    Retry(String),
}

/// Makes (or reuses) the task's worktree and records it in `task.git`; `Ok(false)` when the task runs in the
/// project folder. Runs `setup` and copies `copy` into a new worktree.
pub async fn prepare(hub: &Hub, task: &mut Task) -> Result<bool, PrepareError> {
    let Some(b) = settings(hub, task).await else { return Ok(false) };
    let slug = task.project.clone().unwrap_or_default();
    let fatal = |why: String| PrepareError::Fatal(format!("{why}; switch the project's branches to the shared folder or fix the repo"));
    let folder = hub.work_dir(Some(&slug)).await;
    let repo = Git::new(hub, &folder);
    if repo.text(&["rev-parse", "--verify", "-q", "HEAD"]).await.is_none() {
        return Err(fatal(format!("{} is not a git repo with a commit, so tasks cannot have worktrees", folder.display())));
    }
    let path = worktree_path(hub, &slug, task.id);
    let branch = branch_name(task.id);
    if let Some(git) = &task.git
        && path.join(".git").exists()
    {
        let mut git = git.clone();
        git.worktree = Some(path.display().to_string());
        task.git = Some(git);
        return Ok(true);
    }
    let base = match task.git.as_ref().map(|g| g.base.clone()).or(b.base.clone()) {
        Some(base) => base,
        None => repo.text(&["symbolic-ref", "--short", "-q", "HEAD"]).await.ok_or_else(|| fatal("the project folder has no branch checked out (detached HEAD) and no base branch is set".into()))?,
    };
    if repo.text(&["rev-parse", "--verify", "-q", &format!("refs/heads/{base}")]).await.is_none() {
        return Err(fatal(format!("the base branch `{base}` does not exist")));
    }
    std::fs::create_dir_all(path.parent().unwrap_or(&path)).map_err(|e| PrepareError::Retry(format!("{e:#}")))?;
    // A folder left by a crash is not a worktree git knows; prune its record and start clean.
    let _ = repo.run(&["worktree", "prune"], LOCAL).await;
    let _ = std::fs::remove_dir_all(&path);
    let p = path.to_str().ok_or_else(|| fatal("non-UTF-8 data path".into()))?;
    let exists = repo.text(&["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).await.is_some();
    let added = if exists { repo.ok(&["worktree", "add", p, &branch]).await } else { repo.ok(&["worktree", "add", "-b", &branch, p, &base]).await };
    added.map_err(|e| PrepareError::Retry(format!("{e:#}")))?;
    for entry in &b.copy {
        let from = folder.join(entry);
        let to = path.join(entry);
        if from.exists() && !to.exists() {
            copy_tree(&from, &to).map_err(|e| PrepareError::Retry(format!("copying `{entry}` into the worktree: {e:#}")))?;
        }
    }
    task.git = Some(TaskGit { branch, base, worktree: Some(path.display().to_string()), ..Default::default() });
    for command in &b.setup {
        let (ok, output) = crate::orchestrator::shell_for(hub, Some(&slug), &path, command, Duration::from_secs(30 * 60)).await.map_err(|e| PrepareError::Retry(format!("{e:#}")))?;
        if !ok {
            return Err(PrepareError::Retry(format!("worktree setup `{command}` failed:\n{output}")));
        }
    }
    Ok(true)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(from)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    if meta.is_dir() {
        std::fs::create_dir_all(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            copy_tree(&entry.path(), &to.join(entry.file_name()))?;
        }
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to)?;
    }
    Ok(())
}

/// How landing a task went.
#[derive(Debug, PartialEq)]
pub enum Landed {
    /// The base now points at this commit.
    Merged(String),
    /// Pushed; with the pull request's URL when one could be opened, else why not.
    Pushed { pr_url: Option<String>, note: String },
    /// `land = keep`: waiting for the owner.
    Keep,
    /// Something a person must sort out (changes in the project folder, no remote, a failed push).
    Blocked(String),
    /// The rebase onto the base hit conflicts: back to the agent.
    Conflict(String),
}

/// The task's worktree, branch and folders, for committing and landing.
pub struct Lander {
    pub wt: Git,
    pub repo: Git,
    pub branch: String,
    pub base: String,
    pub copy: Vec<String>,
    pub mode: LandMode,
}

impl Lander {
    /// `None` when the task has no worktree (it ran in the project folder).
    pub async fn of(hub: &Hub, task: &Task) -> Option<Lander> {
        let git = task.git.as_ref()?;
        let wt = PathBuf::from(git.worktree.as_deref()?);
        if !wt.join(".git").exists() {
            return None;
        }
        let b = hub.store.project(task.project.as_deref()?).await.ok().flatten().map(|p| p.settings.branches).unwrap_or_default();
        Some(Lander {
            wt: Git::new(hub, wt),
            repo: Git::new(hub, hub.work_dir(task.project.as_deref()).await),
            branch: git.branch.clone(),
            base: git.base.clone(),
            copy: b.copy,
            mode: git.land.unwrap_or(b.land),
        })
    }

    /// Commits whatever the agent left uncommitted (never the `copy` files); the new commit, if any.
    pub async fn commit(&self, task: &Task) -> Result<Option<String>> {
        let mut add = vec!["add".to_string(), "-A".into(), "--".into(), ".".into()];
        add.extend(self.copy.iter().map(|c| format!(":(exclude){c}")));
        let add: Vec<&str> = add.iter().map(String::as_str).collect();
        self.wt.ok(&add).await?;
        if self.wt.run(&["diff", "--cached", "--quiet"], LOCAL).await?.ok {
            return Ok(None);
        }
        let summary = task.result.as_deref().unwrap_or_default().trim();
        let message = format!("Task #{}: {}\n\n{}\n\nMAP-Task: {}", task.id, task.title, summary, task.id);
        let mut args: Vec<&str> = vec![];
        // A machine without a git identity still gets its tasks committed.
        let has_identity = self.wt.text(&["config", "user.email"]).await.is_some();
        if !has_identity {
            args.extend(["-c", "user.name=MAP", "-c", "user.email=map@localhost"]);
        }
        args.extend(["commit", "-q", "--no-verify", "-m", &message]);
        self.wt.ok(&args).await?;
        Ok(self.wt.text(&["rev-parse", "HEAD"]).await)
    }

    /// Commits on the branch that the base does not have.
    pub async fn ahead(&self) -> Result<u32> {
        let out = self.wt.text(&["rev-list", "--count", &format!("{}..HEAD", self.base)]).await.context("counting the branch's commits")?;
        Ok(out.parse().unwrap_or(0))
    }

    /// Rebases the branch onto the base when the base has moved on. `Ok(true)`: it was rebased (verify again);
    /// `Err(Landed::Conflict)` lists the conflicting files and leaves the branch as it was.
    pub async fn rebase(&self) -> Result<Result<bool, Landed>> {
        if self.wt.run(&["merge-base", "--is-ancestor", &self.base, "HEAD"], LOCAL).await?.ok {
            return Ok(Ok(false));
        }
        let run = self.wt.run(&["-c", "user.name=MAP", "-c", "user.email=map@localhost", "rebase", "--no-autostash", &self.base], LOCAL).await?;
        if run.ok {
            return Ok(Ok(true));
        }
        let files = self.wt.text(&["diff", "--name-only", "--diff-filter=U"]).await.unwrap_or_default();
        let _ = self.wt.run(&["rebase", "--abort"], LOCAL).await;
        let files = if files.is_empty() { crate::decisions::last_line(&run.output) } else { files.lines().collect::<Vec<_>>().join(", ") };
        Ok(Err(Landed::Conflict(format!(
            "Landing on `{base}` hit conflicts ({files}). `{base}` moved on while you worked: run `git rebase {base}` on your branch `{branch}`, \
             resolve the conflicts keeping both changes' intent, run the checks, commit, and report done again.",
            base = self.base,
            branch = self.branch
        ))))
    }

    /// Moves the base to the branch's tip: a fast-forward merge where the base is checked out (the project folder,
    /// usually), else a compare-and-swap of the ref.
    pub async fn fast_forward(&self) -> Result<Landed> {
        let tip = self.wt.text(&["rev-parse", "HEAD"]).await.context("reading the branch tip")?;
        let list = self.repo.text(&["worktree", "list", "--porcelain"]).await.unwrap_or_default();
        let target = format!("branch refs/heads/{}", self.base);
        let mut checked_out = None;
        let mut current = None;
        for line in list.lines() {
            if let Some(p) = line.strip_prefix("worktree ") {
                current = Some(PathBuf::from(p));
            } else if line == target {
                checked_out = current.clone();
            }
        }
        if let Some(dir) = checked_out {
            let there = Git { dir: dir.clone(), gitconfig: self.repo.gitconfig.clone() };
            let run = there.run(&["merge", "--ff-only", "-q", &tip], LOCAL).await?;
            if !run.ok {
                return Ok(Landed::Blocked(format!(
                    "`{}` is checked out in {} and git could not fast-forward it there ({}). Commit or stash the changes in that folder, then press Merge.",
                    self.base,
                    dir.display(),
                    crate::decisions::last_line(&run.output)
                )));
            }
        } else {
            let old = self.repo.text(&["rev-parse", &format!("refs/heads/{}", self.base)]).await.context("reading the base")?;
            self.repo.ok(&["update-ref", &format!("refs/heads/{}", self.base), &tip, &old]).await?;
        }
        Ok(Landed::Merged(tip))
    }

    /// Pushes the branch (force-with-lease: it is MAP's own) and opens a pull request with `gh` or `glab`.
    pub async fn push(&self, hub: &Hub, task: &Task) -> Result<Landed> {
        let remotes = self.repo.text(&["remote"]).await.unwrap_or_default();
        let Some(remote) = remotes.lines().find(|r| *r == "origin").or(remotes.lines().next()).map(String::from) else {
            return Ok(Landed::Blocked("the project has no git remote to push to: connect one on the project page".into()));
        };
        let run = self.wt.run(&["push", "--force-with-lease", "-u", &remote, &format!("HEAD:refs/heads/{}", self.branch)], NET).await?;
        if !run.ok {
            return Ok(Landed::Blocked(format!("pushing `{}` to {remote} failed: {}", self.branch, crate::decisions::last_line(&git_access::mask_secrets(&run.output)))));
        }
        let url = self.repo.text(&["remote", "get-url", &remote]).await.unwrap_or_default();
        let (pr_url, note) = open_pr(hub, &self.wt.dir, &url, &self.branch, &self.base, task).await;
        Ok(Landed::Pushed { pr_url, note })
    }

    /// Removes the worktree and, with `delete_branch`, the branch.
    pub async fn remove(&self, delete_branch: bool) {
        remove(&self.repo, &self.wt.dir, delete_branch.then_some(self.branch.as_str())).await;
    }
}

async fn remove(repo: &Git, wt: &Path, branch: Option<&str>) {
    if let Some(p) = wt.to_str() {
        let _ = repo.run(&["worktree", "remove", "--force", p], LOCAL).await;
    }
    let _ = std::fs::remove_dir_all(wt);
    let _ = repo.run(&["worktree", "prune"], LOCAL).await;
    if let Some(b) = branch {
        let _ = repo.run(&["branch", "-D", b], LOCAL).await;
    }
}

/// Removes the task's worktree (if any) and its branch unless it was pushed; for discarding or deleting a task.
pub async fn discard(hub: &Hub, task: &mut Task) {
    let Some(git) = task.git.as_mut() else { return };
    let repo = Git::new(hub, hub.work_dir(task.project.as_deref()).await);
    let wt = git.worktree.take().map(PathBuf::from).unwrap_or_else(|| worktree_path(hub, task.project.as_deref().unwrap_or_default(), task.id));
    remove(&repo, &wt, (!git.pushed).then_some(git.branch.as_str())).await;
}

/// Opens a pull request (GitHub, `gh`) or merge request (GitLab, `glab`) for `branch`; its URL, or a note on why not.
async fn open_pr(hub: &Hub, dir: &Path, remote_url: &str, branch: &str, base: &str, task: &Task) -> (Option<String>, String) {
    let title = format!("Task #{}: {}", task.id, task.title);
    let body = format!("{}\n\n_Opened by MAP for task #{}._", task.result.as_deref().unwrap_or_default().trim(), task.id);
    let (cli, args): (&str, Vec<&str>) = if remote_url.contains("github") {
        ("gh", vec!["pr", "create", "--head", branch, "--base", base, "--title", &title, "--body", &body])
    } else if remote_url.contains("gitlab") {
        ("glab", vec!["mr", "create", "--source-branch", branch, "--target-branch", base, "--title", &title, "--description", &body, "--yes"])
    } else {
        return (None, format!("pushed `{branch}`; open a pull request against `{base}` on the remote (MAP opens them on GitHub and GitLab only)"));
    };
    let mut cmd = tokio::process::Command::new(cli);
    cmd.args(&args).current_dir(dir).envs(hub.command_env()).env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
    let run = match git_access::run(cmd, NET).await {
        Ok(run) => run,
        Err(_) => return (None, format!("pushed `{branch}`; `{cli}` is not installed, so open the pull request against `{base}` yourself")),
    };
    let url = run.output.split_whitespace().rev().find(|w| w.starts_with("https://")).map(String::from);
    match url {
        Some(url) => (Some(url.clone()), format!("pushed `{branch}` and opened {url}")),
        None => (None, format!("pushed `{branch}`; `{cli}` could not open the pull request: {}", crate::decisions::last_line(&run.output))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_entries_stay_inside_the_project() {
        assert!(validate_copy(".env").is_ok());
        assert!(validate_copy("config/local.toml").is_ok());
        for bad in ["", "/etc/passwd", "../x", "a/../../x", ".git/config", "./x"] {
            assert!(validate_copy(bad).is_err(), "{bad}");
        }
    }
}
