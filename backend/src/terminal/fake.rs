//! In-memory terminal for tests: panes are strings the test sets, and sent text is recorded.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use anyhow::{Result, bail};
use async_trait::async_trait;

use super::{TermSession, Terminal};

#[derive(Default)]
pub struct FakeTerminal {
    pub sessions: Mutex<BTreeMap<String, FakeSession>>,
}

#[derive(Default, Clone)]
pub struct FakeSession {
    pub pane: String,
    pub attached: usize,
    pub sent: Vec<String>,
    pub script: String,
    /// The folder it was started in.
    pub cwd: std::path::PathBuf,
    /// Panes shown one after another as keys arrive (a TUI reacting to Down, Enter).
    pub after_keys: std::collections::VecDeque<String>,
}

impl FakeTerminal {
    pub fn set_pane(&self, name: &str, pane: &str) {
        if let Some(s) = self.sessions.lock().unwrap().get_mut(name) {
            s.pane = pane.to_string();
        }
    }

    /// Each later `send_key` shows the next of `panes`.
    pub fn on_keys(&self, name: &str, panes: &[&str]) {
        if let Some(s) = self.sessions.lock().unwrap().get_mut(name) {
            s.after_keys = panes.iter().map(|p| p.to_string()).collect();
        }
    }

    pub fn sent(&self, name: &str) -> Vec<String> {
        self.sessions.lock().unwrap().get(name).map(|s| s.sent.clone()).unwrap_or_default()
    }

    pub fn exit(&self, name: &str) {
        self.sessions.lock().unwrap().remove(name);
    }
}

#[async_trait]
impl Terminal for FakeTerminal {
    async fn list(&self) -> Result<Vec<TermSession>> {
        Ok(self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|(name, s)| TermSession { name: name.clone(), attached_clients: s.attached })
            .collect())
    }

    async fn exists(&self, name: &str) -> Result<bool> {
        Ok(self.sessions.lock().unwrap().contains_key(name))
    }

    async fn spawn(&self, name: &str, cwd: &Path, script: &Path) -> Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(name) {
            bail!("duplicate session: {name}");
        }
        let script = std::fs::read_to_string(script).unwrap_or_default();
        sessions.insert(name.to_string(), FakeSession { script, cwd: cwd.to_path_buf(), ..Default::default() });
        Ok(())
    }

    async fn send_text(&self, name: &str, text: &str, enter: bool) -> Result<()> {
        let mut sessions = self.sessions.lock().unwrap();
        let Some(s) = sessions.get_mut(name) else { bail!("no session {name}") };
        s.sent.push(if enter { format!("{text}⏎") } else { text.to_string() });
        Ok(())
    }

    async fn send_key(&self, name: &str, key: &str) -> Result<()> {
        self.send_text(name, &format!("<{key}>"), false).await?;
        if let Some(s) = self.sessions.lock().unwrap().get_mut(name)
            && let Some(next) = s.after_keys.pop_front()
        {
            s.pane = next;
        }
        Ok(())
    }

    async fn capture(&self, name: &str, _lines: usize, _escapes: bool) -> Result<String> {
        match self.sessions.lock().unwrap().get(name) {
            Some(s) => Ok(s.pane.clone()),
            None => bail!("no session {name}"),
        }
    }

    async fn kill(&self, name: &str) -> Result<()> {
        self.sessions.lock().unwrap().remove(name);
        Ok(())
    }
}
