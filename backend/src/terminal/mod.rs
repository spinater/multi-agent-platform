//! Where agents run. tmux in production; tests use [`fake::FakeTerminal`].

#[cfg(test)]
pub mod fake;
pub mod tmux;

use std::path::Path;

use anyhow::Result;
use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermSession {
    pub name: String,
    pub attached_clients: usize,
}

#[async_trait]
pub trait Terminal: Send + Sync {
    async fn list(&self) -> Result<Vec<TermSession>>;
    async fn exists(&self, name: &str) -> Result<bool>;
    /// Starts `script` (an executable file) in a detached session rooted at `cwd`.
    async fn spawn(&self, name: &str, cwd: &Path, script: &Path) -> Result<()>;
    /// Types `text` literally (no key-name interpretation), then presses Enter when `enter` is set.
    async fn send_text(&self, name: &str, text: &str, enter: bool) -> Result<()>;
    /// Sends one named key such as `C-c` or `Escape`.
    async fn send_key(&self, name: &str, key: &str) -> Result<()>;
    /// The last `lines` lines of the pane; `escapes` keeps SGR sequences (needed for [`crate::detect::pending_input`]).
    async fn capture(&self, name: &str, lines: usize, escapes: bool) -> Result<String>;
    /// Stops the session and every process under it, then confirms it is gone.
    async fn kill(&self, name: &str) -> Result<()>;
}
