use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use tokio::process::Command;

use super::{TermSession, Terminal};

/// Variables tmux reads to find "the current server" when it runs inside a tmux session. They would win over `-L`'s
/// absence (and point a MAP started inside tmux at that server), so every tmux command MAP runs drops them.
const TMUX_VARS: [&str; 2] = ["TMUX", "TMUX_PANE"];

pub struct Tmux {
    bin: String,
    /// `workspace.tmux_socket`: `-L <name>` on every command; `None`: tmux's default server.
    socket: Option<String>,
}

impl Tmux {
    pub fn new(socket: Option<String>) -> Self {
        let bin = ["/opt/homebrew/bin/tmux", "/usr/local/bin/tmux", "/usr/bin/tmux"]
            .into_iter()
            .find(|p| Path::new(p).exists())
            .unwrap_or("tmux")
            .to_string();
        Self { bin, socket: socket.filter(|s| !s.is_empty()) }
    }

    /// The tmux command for `args`: `-L <socket>` first when one is configured, and without `TMUX` / `TMUX_PANE`.
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(&self.bin);
        if let Some(socket) = &self.socket {
            cmd.arg("-L").arg(socket);
        }
        for var in TMUX_VARS {
            cmd.env_remove(var);
        }
        cmd.args(args).kill_on_drop(true);
        cmd
    }

    async fn run(&self, args: &[&str]) -> Result<std::process::Output> {
        let out = tokio::time::timeout(Duration::from_secs(5), self.command(args).output())
            .await
            .context("tmux timed out")?
            .context("running tmux")?;
        Ok(out)
    }

    async fn ok(&self, args: &[&str]) -> Result<String> {
        let out = self.run(args).await?;
        if !out.status.success() {
            bail!("tmux {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// `=name` makes tmux match the exact session, not a prefix.
    fn target(name: &str) -> String {
        format!("={name}")
    }

    /// Pane target: exact session, its active window and pane.
    fn pane(name: &str) -> String {
        format!("={name}:")
    }

    async fn descendants(roots: Vec<String>) -> Vec<String> {
        let mut all = Vec::new();
        let mut frontier = roots;
        while !frontier.is_empty() {
            let mut next = Vec::new();
            for pid in &frontier {
                if let Ok(out) = Command::new("pgrep").args(["-P", pid]).output().await {
                    next.extend(String::from_utf8_lossy(&out.stdout).split_whitespace().map(String::from));
                }
            }
            all.extend(next.iter().cloned());
            frontier = next;
        }
        all
    }
}


fn alive(pid: &str) -> bool {
    std::process::Command::new("kill").args(["-0", pid]).output().is_ok_and(|o| o.status.success())
}

#[async_trait]
impl Terminal for Tmux {
    async fn list(&self) -> Result<Vec<TermSession>> {
        let out = self.run(&["list-sessions", "-F", "#{session_name}\t#{session_attached}"]).await?;
        // No server running means no sessions, not an error.
        if !out.status.success() {
            return Ok(vec![]);
        }
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|line| {
                let (name, attached) = line.split_once('\t')?;
                Some(TermSession { name: name.to_string(), attached_clients: attached.parse().unwrap_or(0) })
            })
            .collect())
    }

    async fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.run(&["has-session", "-t", &Self::target(name)]).await?.status.success())
    }

    async fn spawn(&self, name: &str, cwd: &Path, script: &Path) -> Result<()> {
        let cwd = cwd.to_str().context("workspace path is not UTF-8")?;
        let script = shlex::try_quote(script.to_str().context("script path is not UTF-8")?)?.into_owned();
        self.ok(&["new-session", "-d", "-s", name, "-x", "220", "-y", "50", "-c", cwd, &script]).await?;
        tokio::time::sleep(Duration::from_millis(400)).await;
        if !self.exists(name).await? {
            bail!("the agent exited immediately; check its binary and flags");
        }
        Ok(())
    }

    async fn send_text(&self, name: &str, text: &str, enter: bool) -> Result<()> {
        self.ok(&["send-keys", "-t", &Self::pane(name), "-l", text]).await?;
        if enter {
            // TUIs may treat a key that arrives in the same burst as pasted text; give them a beat.
            tokio::time::sleep(Duration::from_millis(150)).await;
            self.ok(&["send-keys", "-t", &Self::pane(name), "Enter"]).await?;
        }
        Ok(())
    }

    async fn send_key(&self, name: &str, key: &str) -> Result<()> {
        self.ok(&["send-keys", "-t", &Self::pane(name), key]).await.map(|_| ())
    }

    async fn capture(&self, name: &str, lines: usize, escapes: bool) -> Result<String> {
        let start = format!("-{lines}");
        let mut args = vec!["capture-pane", "-p", "-J", "-S", &start];
        if escapes {
            args.push("-e");
        }
        let target = Self::pane(name);
        args.extend(["-t", &target]);
        self.ok(&args).await
    }

    async fn kill(&self, name: &str) -> Result<()> {
        let panes = self.ok(&["list-panes", "-s", "-t", &Self::target(name), "-F", "#{pane_pid}"]).await.unwrap_or_default();
        let roots: Vec<String> = panes.split_whitespace().map(String::from).collect();
        let mut pids = roots.clone();
        pids.extend(Self::descendants(roots).await);
        for pid in &pids {
            let _ = Command::new("kill").args(["-TERM", pid]).output().await;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        for pid in pids.iter().filter(|p| alive(p)) {
            let _ = Command::new("kill").args(["-KILL", pid]).output().await;
        }
        let _ = self.run(&["kill-session", "-t", &Self::target(name)]).await;
        tokio::time::sleep(Duration::from_millis(300)).await;
        if self.exists(name).await? {
            bail!("session {name} is still running after kill");
        }
        let survivors: Vec<&String> = pids.iter().filter(|p| alive(p)).collect();
        if !survivors.is_empty() {
            bail!("processes survived the kill: {survivors:?}");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parts(t: &Tmux, args: &[&str]) -> (Vec<String>, Vec<(String, Option<String>)>) {
        let cmd = t.command(args);
        let std = cmd.as_std();
        let argv = std.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        let env = std.get_envs().map(|(k, v)| (k.to_string_lossy().into_owned(), v.map(|v| v.to_string_lossy().into_owned()))).collect();
        (argv, env)
    }

    #[test]
    fn commands_use_the_private_socket_and_never_the_callers_tmux() {
        let private = Tmux::new(Some("map-test".into()));
        let (argv, env) = parts(&private, &["has-session", "-t", "=map-x"]);
        assert_eq!(argv, ["-L", "map-test", "has-session", "-t", "=map-x"]);
        // Removed (`None`), not just left unset: a MAP started inside tmux must not inherit them.
        for var in TMUX_VARS {
            assert!(env.contains(&(var.to_string(), None)), "{var} not removed: {env:?}");
        }
        let default = Tmux::new(None);
        let (argv, env) = parts(&default, &["list-sessions"]);
        assert_eq!(argv, ["list-sessions"]);
        assert!(env.contains(&("TMUX".to_string(), None)) && env.contains(&("TMUX_PANE".to_string(), None)));
        assert_eq!(parts(&Tmux::new(Some(String::new())), &["ls"]).0, ["ls"], "an empty name is the default server");
    }
}
