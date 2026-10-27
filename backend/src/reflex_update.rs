//! Reflex updates. Reflex is a row of the update report like a provider CLI (`provider: "reflex"`): its installed
//! version (`reflex --version`) against its latest release (`[reflex.update] latest_url`, GitHub's releases API by
//! default), announced once per release by `updates.rs`. Updating runs `[reflex.update] install` (the release
//! installer, which checks the archive's SHA-256) in the helper session `<prefix>update-reflex`, with `{dir}` the folder
//! of `reflex.binary`. When it ends with a new version installed, every Reflex restarts on it: the projects', the
//! gates' and the chat's. `auto = true` starts the update as soon as a check sees a release.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde::Deserialize;

use crate::hub::{Hub, LaunchRequest};
use crate::model::{Level, Role, Session, UpdateInfo};
use crate::updates::{self, StartError};

/// The update report's row name for Reflex (no provider is called that).
pub const NAME: &str = "reflex";

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct UpdateConfig {
    /// A GitHub "latest release" URL; the version is its `tag_name`. Empty: Reflex is not checked.
    pub latest_url: String,
    /// The installer, `{dir}` substituted with the folder of `reflex.binary`. Empty: no update from here.
    pub install: Vec<String>,
    /// Install a new release unasked, as soon as a check sees it.
    pub auto: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        let script = "curl -fsSL https://raw.githubusercontent.com/gist-rs/reflex/main/install.sh | REFLEX_INSTALL_DIR=\"$1\" sh";
        Self {
            latest_url: "https://api.github.com/repos/gist-rs/reflex/releases/latest".into(),
            install: ["/bin/sh", "-c", script, "sh", "{dir}"].map(String::from).to_vec(),
            auto: false,
        }
    }
}

fn install_command(install: &[String], dir: &Path) -> Vec<String> {
    install.iter().map(|a| a.replace("{dir}", &dir.to_string_lossy())).collect()
}

/// Why an update from here cannot work: no binary, a binary the installer does not write (it installs `reflex`), or
/// a folder this user cannot write.
fn blocked(binary: Option<&PathBuf>) -> Option<String> {
    match binary {
        None => Some("reflex.binary is not installed".into()),
        Some(b) if b.file_name().is_none_or(|n| n != NAME) => Some(format!("the installer writes `reflex`, not `{}`", b.display())),
        Some(b) => updates::update_blocked(b),
    }
}

/// Reflex's row, checked now.
pub async fn check(hub: &Hub) -> UpdateInfo {
    let cfg = &hub.config.reflex;
    let binary = hub.binary_path(&cfg.binary);
    let session = updates::session_name(&hub.config, NAME);
    let dir = binary.as_ref().and_then(|b| b.parent()).map(Path::to_path_buf).unwrap_or_default();
    let mut info = UpdateInfo {
        provider: NAME.into(),
        installed: None,
        latest: None,
        available: false,
        checked_at: Some(Utc::now()),
        error: None,
        version_command: vec![cfg.binary.clone(), "--version".into()],
        latest_url: Some(cfg.update.latest_url.clone()).filter(|u| !u.is_empty()),
        update_command: if cfg.update.install.is_empty() { vec![] } else { install_command(&cfg.update.install, &dir) },
        update_blocked: if cfg.update.install.is_empty() { None } else { blocked(binary.as_ref()) },
        updating: hub.term.exists(&session).await.unwrap_or(false),
        session,
    };
    let mut errors = vec![];
    match &binary {
        Some(b) => {
            let b = b.to_string_lossy().to_string();
            let read = tokio::task::spawn_blocking(move || crate::reflex::read_version(&b)).await.ok().flatten();
            info.installed = read.as_deref().and_then(updates::parse_version);
            if info.installed.is_none() {
                errors.push(format!("no version from `{} --version`", cfg.binary));
            }
        }
        None => errors.push(format!("`{}` is not installed", cfg.binary)),
    }
    if let Some(url) = &info.latest_url {
        match updates::fetch_latest(&hub.http, url, Some("tag_name"), Duration::from_secs(hub.config.updates.timeout_secs)).await {
            Ok(v) => info.latest = Some(v),
            Err(e) => errors.push(e),
        }
    }
    info.available = matches!((&info.installed, &info.latest), (Some(i), Some(l)) if updates::newer(l, i));
    info.error = (!errors.is_empty()).then(|| errors.join("; "));
    info
}

/// The provider rows plus Reflex's (when a latest URL is set); with `auto`, starts an available update.
pub async fn with_reflex(hub: &Arc<Hub>, mut rows: Vec<UpdateInfo>) -> Vec<UpdateInfo> {
    if hub.config.reflex.update.latest_url.is_empty() {
        return rows;
    }
    let row = check(hub).await;
    let start_now = hub.config.reflex.update.auto && row.available && row.update_blocked.is_none() && !row.updating && !row.update_command.is_empty();
    rows.push(row);
    if start_now && let Err(err) = start(hub, NAME).await {
        let why = match err {
            StartError::Conflict(why) => why,
            StartError::Invalid(err) => format!("{err:#}"),
        };
        tracing::warn!("reflex auto-update: {why}");
    }
    rows
}

/// `POST /api/providers/{name}/update`: Reflex's installer for `reflex`, else the provider's update (`updates.rs`).
pub async fn start(hub: &Arc<Hub>, name: &str) -> Result<Session, StartError> {
    if name != NAME {
        return updates::start(hub, name).await;
    }
    let row = check(hub).await;
    if row.update_command.is_empty() {
        return Err(StartError::Invalid(anyhow::anyhow!("no installer; set reflex.update.install in map.toml")));
    }
    if let Some(why) = row.update_blocked {
        return Err(StartError::Conflict(why));
    }
    if row.updating {
        return Err(StartError::Conflict(format!("an update of Reflex is already running in `{}`", row.session)));
    }
    // The helper session needs a provider for its environment; the installer itself uses none.
    let provider = crate::usage::ordered_providers(&hub.config).into_iter().next().ok_or_else(|| StartError::Invalid(anyhow::anyhow!("no provider to run the session")))?;
    let req = LaunchRequest { name: Some(row.session.clone()), provider, model: None, role: Role::Manual, task_id: None, project: None, prompt: None };
    let session = hub.launch_command(req, Some(&updates::wrapped(&row.update_command))).await.map_err(StartError::Invalid)?;
    hub.event(Level::Info, "provider.update", Some(&row.session), None, format!("Update of Reflex started: {}", row.update_command.join(" "))).await;
    let mut shown = row.clone();
    shown.updating = true;
    hub.announce_updates(hub.updates.set_provider(shown));
    tokio::spawn(watch(Arc::clone(hub), row.session, row.installed));
    Ok(session)
}

/// Waits for the update session to end, checks again, and restarts every Reflex on a new version.
async fn watch(hub: Arc<Hub>, session: String, before: Option<String>) {
    let started = tokio::time::Instant::now();
    while hub.term.exists(&session).await.unwrap_or(false) {
        if started.elapsed() > Duration::from_secs(3600) {
            return;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let row = check(&hub).await;
    hub.announce_updates(hub.updates.set_provider(row.clone()));
    if row.installed.is_none() || row.installed == before {
        hub.event(Level::Warning, "reflex.update", None, None, format!("Reflex update ended without a new version (still {})", before.as_deref().unwrap_or("unknown"))).await;
        return;
    }
    let now = row.installed.as_deref().unwrap_or("?");
    hub.event(Level::Info, "reflex.update", None, None, format!("Reflex {now} installed; restarting every Reflex")).await;
    restart_all(&hub).await;
}

async fn restart_all(hub: &Arc<Hub>) {
    // `reflex --version` is cached for 10 seconds: wait it out so the dashboard shows the new version.
    tokio::time::sleep(Duration::from_secs(11)).await;
    if let Err(err) = hub.restart_gates().await {
        tracing::warn!("gates reflex: {err:#}");
    }
    if let Err(err) = crate::chat::restart(hub).await {
        tracing::warn!("chat reflex: {err:#}");
    }
    for project in hub.store.projects().await.unwrap_or_default() {
        if let Err(err) = hub.restart_reflex(&project.slug).await {
            tracing::warn!("reflex for {}: {err:#}", project.slug);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installer_gets_the_binary_folder() {
        let cmd = install_command(&UpdateConfig::default().install, Path::new("/home/u/.cargo/bin"));
        assert_eq!(cmd.last().unwrap(), "/home/u/.cargo/bin");
        assert!(cmd[2].contains("REFLEX_INSTALL_DIR=\"$1\""));
    }

    #[test]
    fn only_a_binary_named_reflex_is_updated() {
        assert!(blocked(None).is_some());
        assert!(blocked(Some(&PathBuf::from("/opt/x/riir-reflex"))).unwrap().contains("writes `reflex`"));
    }
}
