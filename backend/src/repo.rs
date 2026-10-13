//! A project's bare `corpus.git` (cloned and pushed over `/git`), and read-only looks at a project's folder. The
//! platform writes to the corpus with plumbing (a temporary index and `commit-tree`), so it never needs a working tree
//! there and never races a user's push unnoticed.

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::model::{Commit, RepoFile};

pub const BRANCH: &str = "refs/heads/main";

/// Corpus folder whose contents Reflex never loads: archived domains live in `_archive/<domain>/…`.
pub const ARCHIVE_DIR: &str = "_archive";

/// `git` with a fixed identity, so platform commits never depend on the server user's git config.
fn git_cmd() -> Command {
    let mut cmd = Command::new("git");
    cmd.env("GIT_AUTHOR_NAME", "multi-agent-platform")
        .env("GIT_AUTHOR_EMAIL", "platform@localhost")
        .env("GIT_COMMITTER_NAME", "multi-agent-platform")
        .env("GIT_COMMITTER_EMAIL", "platform@localhost")
        .env("GIT_TERMINAL_PROMPT", "0");
    cmd
}

async fn run(mut cmd: Command, stdin: Option<&[u8]>, what: &str) -> Result<Vec<u8>> {
    cmd.stdin(if stdin.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().with_context(|| format!("starting {what}"))?;
    if let Some(data) = stdin {
        let mut pipe = child.stdin.take().context("stdin")?;
        pipe.write_all(data).await?;
    }
    let out = child.wait_with_output().await?;
    if !out.status.success() {
        bail!("{what}: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(out.stdout)
}

async fn git(repo: &Path, args: &[&str], index: Option<&Path>, stdin: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut cmd = git_cmd();
    cmd.arg("--git-dir").arg(repo).args(args);
    if let Some(index) = index {
        cmd.env("GIT_INDEX_FILE", index);
    }
    run(cmd, stdin, &format!("git {}", args.first().unwrap_or(&""))).await
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).trim().to_string()
}

pub async fn init_bare(repo: &Path) -> Result<()> {
    let mut cmd = git_cmd();
    cmd.args(["init", "--bare", "-q", "-b", "main"]).arg(repo);
    run(cmd, None, "git init").await?;
    // `git http-backend` refuses pushes without an authenticated REMOTE_USER unless this is set; /git does its own auth.
    git(repo, &["config", "http.receivepack", "true"], None, None).await?;
    Ok(())
}

/// What is in a folder on this machine: its top-level entries and, when it is a git repo, the branch checked out,
/// the local branches and the commit count. Read-only.
pub async fn inspect_folder(dir: &Path) -> crate::model::SourceInspection {
    let mut out = crate::model::SourceInspection { ok: true, ..Default::default() };
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut files: Vec<String> = entries.filter_map(|e| e.ok()?.file_name().into_string().ok()).filter(|n| n != ".git").collect();
        files.sort();
        files.truncate(60);
        out.files = files;
    }
    let work = |args: &'static [&'static str]| {
        let mut cmd = git_cmd();
        cmd.arg("-C").arg(dir).args(args);
        run(cmd, None, "git")
    };
    if text(work(&["rev-parse", "--is-inside-work-tree"]).await.unwrap_or_default()) != "true" {
        return out;
    }
    out.default_branch = Some(text(work(&["rev-parse", "--abbrev-ref", "HEAD"]).await.unwrap_or_default())).filter(|b| !b.is_empty());
    out.branches = text(work(&["branch", "--format=%(refname:short)"]).await.unwrap_or_default()).lines().map(String::from).collect();
    out.commits = text(work(&["rev-list", "--count", "HEAD"]).await.unwrap_or_default()).parse().ok();
    out
}

pub async fn head(repo: &Path) -> Option<String> {
    git(repo, &["rev-parse", "--verify", "-q", BRANCH], None, None).await.ok().map(text)
}

/// Commits the changes onto `main` and returns the new commit. `None` content deletes the path.
pub async fn commit(repo: &Path, files: &[(String, Option<String>)], message: &str) -> Result<String> {
    let index = repo.join(format!("platform-index-{}", std::process::id()));
    let parent = head(repo).await;
    let result = async {
        match &parent {
            Some(p) => git(repo, &["read-tree", p], Some(&index), None).await?,
            None => git(repo, &["read-tree", "--empty"], Some(&index), None).await?,
        };
        for (path, content) in files {
            match content {
                Some(c) => {
                    let sha = text(git(repo, &["hash-object", "-w", "--stdin"], Some(&index), Some(c.as_bytes())).await?);
                    let info = format!("100644,{sha},{path}");
                    git(repo, &["update-index", "--add", "--cacheinfo", &info], Some(&index), None).await?;
                }
                None => {
                    // Mode 0 removes the entry; `--force-remove` would need a work tree, which a bare repo lacks.
                    let line = format!("0 0000000000000000000000000000000000000000\t{path}\n");
                    git(repo, &["update-index", "--index-info"], Some(&index), Some(line.as_bytes())).await?;
                }
            }
        }
        let tree = text(git(repo, &["write-tree"], Some(&index), None).await?);
        let mut args = vec!["commit-tree", tree.as_str()];
        if let Some(p) = &parent {
            args.extend(["-p", p.as_str()]);
        }
        args.extend(["-m", message]);
        let commit = text(git(repo, &args, Some(&index), None).await?);
        // Compare-and-swap on the parent: a push that landed meanwhile makes this fail instead of being overwritten.
        let old = parent.clone().unwrap_or_default();
        git(repo, &["update-ref", BRANCH, &commit, &old], None, None).await?;
        Ok(commit)
    }
    .await;
    let _ = tokio::fs::remove_file(&index).await;
    result
}

pub async fn files(repo: &Path) -> Result<Vec<RepoFile>> {
    if head(repo).await.is_none() {
        return Ok(vec![]);
    }
    let out = text(git(repo, &["ls-tree", "-r", "--long", BRANCH], None, None).await?);
    Ok(out
        .lines()
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            Some(RepoFile { path: path.to_string(), size: meta.split_whitespace().nth(3)?.parse().ok()? })
        })
        .collect())
}

pub async fn read(repo: &Path, path: &str) -> Option<String> {
    let spec = format!("{BRANCH}:{path}");
    git(repo, &["show", &spec], None, None).await.ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

pub async fn log(repo: &Path, limit: usize) -> Result<Vec<Commit>> {
    if head(repo).await.is_none() {
        return Ok(vec![]);
    }
    let n = format!("-n{limit}");
    let out = text(git(repo, &["log", BRANCH, &n, "--format=%H%x1f%an%x1f%at%x1f%s"], None, None).await?);
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut f = line.split('\x1f');
            let (sha, author, at, subject) = (f.next()?, f.next()?, f.next()?, f.next().unwrap_or(""));
            Some(Commit { sha: sha.into(), author: author.into(), at: DateTime::<Utc>::from_timestamp(at.parse().ok()?, 0)?, subject: subject.into() })
        })
        .collect())
}

/// Replaces `dest` with the tree of `main` and returns the Reflex domains in it. Only top-level folders and `.md`
/// files are kept, so a README, a dotfile or a stray script never becomes a domain.
pub async fn export_corpus(repo: &Path, dest: &Path) -> Result<Vec<String>> {
    let _ = tokio::fs::remove_dir_all(dest).await;
    tokio::fs::create_dir_all(dest).await?;
    if head(repo).await.is_none() {
        return Ok(vec![]);
    }
    let tar = git(repo, &["archive", "--format=tar", BRANCH], None, None).await?;
    let mut untar = Command::new("tar");
    untar.arg("-x").arg("-C").arg(dest);
    run(untar, Some(&tar), "tar").await?;

    let mut domains = vec![];
    let mut entries = tokio::fs::read_dir(dest).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().await?.is_dir();
        if !name.starts_with('.') && name != ARCHIVE_DIR && !name.eq_ignore_ascii_case("readme.md") && (is_dir || name.ends_with(".md")) {
            domains.push(name.trim_end_matches(".md").to_string());
        } else if is_dir {
            tokio::fs::remove_dir_all(entry.path()).await?;
        } else {
            tokio::fs::remove_file(entry.path()).await?;
        }
    }
    domains.sort();
    Ok(domains)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn commits_lists_and_exports() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("corpus.git");
        init_bare(&repo).await.unwrap();
        assert!(head(&repo).await.is_none());

        commit(&repo, &[("refund/a.md".into(), Some("refund text".into())), ("README.md".into(), Some("x".into()))], "first").await.unwrap();
        commit(&repo, &[("shipping.md".into(), Some("ship".into()))], "second").await.unwrap();
        let paths: Vec<String> = files(&repo).await.unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["README.md", "refund/a.md", "shipping.md"]);
        assert_eq!(read(&repo, "refund/a.md").await.as_deref(), Some("refund text"));
        let history = log(&repo, 10).await.unwrap();
        assert_eq!(history.iter().map(|c| c.subject.as_str()).collect::<Vec<_>>(), vec!["second", "first"]);

        let live = dir.path().join("live");
        assert_eq!(export_corpus(&repo, &live).await.unwrap(), vec!["refund", "shipping"]);
        assert!(!live.join("README.md").exists());

        commit(&repo, &[("shipping.md".into(), None), ("_archive/old/a.md".into(), Some("archived".into()))], "drop").await.unwrap();
        assert_eq!(export_corpus(&repo, &live).await.unwrap(), vec!["refund"]);
        assert!(!live.join(ARCHIVE_DIR).exists());

    }

    #[tokio::test]
    async fn inspects_a_folder_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let plain = dir.path().join("plain");
        std::fs::create_dir_all(plain.join("src")).unwrap();
        std::fs::write(plain.join("package.json"), "{}").unwrap();
        let seen = inspect_folder(&plain).await;
        assert!(seen.ok);
        assert_eq!(seen.files, vec!["package.json", "src"]);
        assert_eq!((seen.default_branch, seen.commits), (None, None), "not a git repo");

        let repo = dir.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("Cargo.toml"), "[package]").unwrap();
        let sh = |args: &[&str]| {
            let ok = std::process::Command::new("git").arg("-C").arg(&repo).args(args).env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t").status().unwrap().success();
            assert!(ok, "git {args:?}");
        };
        sh(&["init", "-q", "-b", "develop"]);
        sh(&["add", "."]);
        sh(&["commit", "-q", "-m", "one"]);
        let seen = inspect_folder(&repo).await;
        assert_eq!(seen.default_branch.as_deref(), Some("develop"));
        assert_eq!(seen.branches, vec!["develop"]);
        assert_eq!(seen.commits, Some(1));
        assert_eq!(seen.files, vec!["Cargo.toml"], "without .git");
    }
}
