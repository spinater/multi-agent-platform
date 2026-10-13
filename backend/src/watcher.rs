//! The watcher: every few seconds, read each managed pane and decide its state. When a provider reports a
//! rate limit, wait for the reset and type the wake prompt (the Anti-Gravity watcher's job, for every agent).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Local, Utc};

use crate::detect::{self, PaneSignals};
use crate::hub::Hub;
use crate::model::{Level, Role, Session, SessionState};

#[derive(Default)]
struct Memory {
    hash: u64,
    changed_at: Option<DateTime<Utc>>,
    /// Rate-limit lines already acted on; see [`PaneSignals::rate_limit_count`].
    handled_limits: usize,
    last_wake: Option<DateTime<Utc>>,
}

pub struct Watcher {
    hub: Arc<Hub>,
    memory: HashMap<String, Memory>,
    /// The provider list last pushed to dashboards, as JSON, so a sign-in or a cooldown shows without a reload.
    providers: Option<String>,
}

pub async fn run(hub: Arc<Hub>) {
    let interval = Duration::from_secs(hub.config.watcher.interval_secs.max(1));
    let mut watcher = Watcher::new(hub);
    loop {
        if let Err(err) = watcher.tick(Utc::now()).await {
            tracing::error!("watcher tick failed: {err:#}");
        }
        tokio::time::sleep(interval).await;
    }
}

fn fingerprint(pane: &str) -> u64 {
    let mut h = DefaultHasher::new();
    detect::strip_ansi(pane).hash(&mut h);
    h.finish()
}

fn short(d: chrono::Duration) -> String {
    let s = d.num_seconds().max(0);
    if s >= 3600 { format!("{}h {}m", s / 3600, s % 3600 / 60) } else if s >= 60 { format!("{}m {}s", s / 60, s % 60) } else { format!("{s}s") }
}

impl Watcher {
    pub fn new(hub: Arc<Hub>) -> Self {
        Self { hub, memory: HashMap::new(), providers: None }
    }

    pub async fn tick(&mut self, now: DateTime<Utc>) -> anyhow::Result<()> {
        let live: HashMap<String, usize> = self.hub.term.list().await?.into_iter().map(|s| (s.name, s.attached_clients)).collect();
        for session in self.hub.store.sessions().await? {
            if !session.state.is_live() {
                continue;
            }
            let name = session.name.clone();
            if let Err(err) = self.check(session, live.get(&name).copied(), now).await {
                tracing::warn!(session = name, "watch failed: {err:#}");
            }
        }
        self.memory.retain(|name, _| live.contains_key(name));
        self.push_providers();
        Ok(())
    }

    /// Pushes the provider list when it differs from the last one pushed (the first tick only records it).
    fn push_providers(&mut self) {
        let providers = self.hub.providers();
        let Ok(json) = serde_json::to_string(&providers) else { return };
        if self.providers.as_ref() == Some(&json) {
            return;
        }
        let first = self.providers.replace(json).is_none();
        if !first {
            self.hub.announce_providers(providers);
        }
    }

    async fn check(&mut self, mut s: Session, attached: Option<usize>, now: DateTime<Utc>) -> anyhow::Result<()> {
        let hub = Arc::clone(&self.hub);
        let cfg = &hub.config.watcher;
        let before = (s.state, s.detail.clone(), s.last_line.clone(), s.attached, s.rate_limit_reset_at);

        let Some(attached) = attached else {
            s.state = SessionState::Exited;
            s.detail = Some("tmux session ended".into());
            s.attached = false;
            hub.put_session(&s).await?;
            hub.event(Level::Info, "session.exited", Some(&s.name), s.task_id, "Session ended").await;
            return Ok(());
        };
        s.attached = attached > 0;

        let pane = hub.term.capture(&s.name, 200, true).await?;
        let spec = hub.spec(&s.provider)?.clone();
        let sig = detect::scan(&spec, &pane, cfg.scan_lines, now.with_timezone(&Local));
        let mem = self.memory.entry(s.name.clone()).or_default();
        let hash = fingerprint(&pane);
        if hash != mem.hash {
            mem.hash = hash;
            mem.changed_at = Some(now);
            s.last_activity_at = now;
        }
        let changed_recently = mem.changed_at.is_some_and(|t| now - t <= chrono::Duration::seconds(2 * cfg.interval_secs as i64 + 1));
        if sig.rate_limit_count < mem.handled_limits {
            // Old limit lines scrolled out of the scanned tail.
            mem.handled_limits = sig.rate_limit_count;
        }
        s.last_line = sig.last_line.clone();

        let new_limit = sig.rate_limit_count > mem.handled_limits;
        if new_limit && s.state != SessionState::RateLimited {
            let (line, parsed) = sig.rate_limit.clone().unwrap_or_default();
            let reset_at = parsed.unwrap_or(now + chrono::Duration::seconds(cfg.default_rate_limit_wait_secs as i64));
            s.state = SessionState::RateLimited;
            s.rate_limit_reset_at = Some(reset_at);
            s.detail = Some(line.chars().take(200).collect());
            hub.cool_down(&s.provider, reset_at);
            let when = reset_at.with_timezone(&Local).format("%H:%M:%S");
            let how = if parsed.is_some() { "" } else { " (no time in the message; using the default wait)" };
            hub.event(Level::Warning, "session.rate_limited", Some(&s.name), s.task_id, format!("Rate limited; waking at {when}{how}")).await;
        }

        if s.state == SessionState::RateLimited {
            let reset_at = s.rate_limit_reset_at.unwrap_or(now);
            if now < reset_at {
                s.detail = Some(format!("Waking in {}", short(reset_at - now)));
            } else if sig.pending_input {
                s.detail = Some("Limit reset; holding the wake prompt while a person is typing".into());
            } else if mem.last_wake.is_some_and(|t| now - t < chrono::Duration::seconds(cfg.wake_cooldown_secs as i64)) {
                s.detail = Some("Limit reset; wake cooldown".into());
            } else {
                match hub.term.send_text(&s.name, &cfg.wake_prompt, true).await {
                    Ok(()) => {
                        mem.last_wake = Some(now);
                        mem.handled_limits = sig.rate_limit_count;
                        s.wakeups += 1;
                        s.state = SessionState::Working;
                        s.rate_limit_reset_at = None;
                        s.detail = None;
                        hub.event(Level::Success, "session.woken", Some(&s.name), s.task_id, "Limit reset; sent the wake prompt").await;
                    }
                    Err(err) => {
                        s.detail = Some(format!("Wake failed: {err:#}"));
                        hub.event(Level::Error, "session.wake_failed", Some(&s.name), s.task_id, format!("Wake failed: {err:#}")).await;
                    }
                }
            }
        } else {
            let stalled_for = mem.changed_at.map(|t| now - t).unwrap_or_default();
            let (state, detail) = Self::classify(&s, &sig, changed_recently, stalled_for, cfg.stall_minutes, spec.startup_secs, now);
            // The sign-in helper session shows the login screen by design: that is not the provider being signed out.
            let signing_in = s.name == crate::login::session_name(&hub, &s.provider);
            if state == SessionState::NeedsAuth && s.state != SessionState::NeedsAuth && !signing_in {
                hub.cool_down(&s.provider, now + chrono::Duration::minutes(hub.config.orchestrator.provider_cooldown_minutes as i64));
                hub.event(Level::Error, "session.needs_auth", Some(&s.name), s.task_id, format!("{} needs you to sign in again", s.provider)).await;
            }
            if state == SessionState::Stalled && s.state != SessionState::Stalled {
                hub.event(Level::Warning, "session.stalled", Some(&s.name), s.task_id, format!("No pane change for {}", short(stalled_for))).await;
            }
            s.state = state;
            s.detail = detail;
        }

        if before != (s.state, s.detail.clone(), s.last_line.clone(), s.attached, s.rate_limit_reset_at) {
            hub.put_session(&s).await?;
        }
        Ok(())
    }

    fn classify(
        s: &Session,
        sig: &PaneSignals,
        changed_recently: bool,
        unchanged_for: chrono::Duration,
        stall_minutes: u64,
        startup_secs: u64,
        now: DateTime<Utc>,
    ) -> (SessionState, Option<String>) {
        if let Some(marker) = &sig.needs_auth {
            return (SessionState::NeedsAuth, Some(marker.clone()));
        }
        if sig.pending_input {
            return (SessionState::WaitingInput, Some("A person is typing on the input line".into()));
        }
        if changed_recently || sig.working_marker {
            let starting = s.state == SessionState::Starting && now - s.created_at < chrono::Duration::seconds(startup_secs as i64 * 2);
            return (if starting { SessionState::Starting } else { SessionState::Working }, None);
        }
        if sig.idle_marker {
            return (SessionState::Idle, None);
        }
        if s.role == Role::Worker && unchanged_for >= chrono::Duration::minutes(stall_minutes as i64) {
            return (SessionState::Stalled, Some(format!("No pane change for {}", short(unchanged_for))));
        }
        if let Some(err) = &sig.error {
            return (SessionState::Error, Some(err.clone()));
        }
        (SessionState::Idle, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::hub::LaunchRequest;
    use crate::model::ServerMessage;
    use crate::store::Store;
    use crate::terminal::fake::FakeTerminal;
    use std::path::Path;

    async fn setup() -> (Arc<Hub>, Arc<FakeTerminal>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!("[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n", dir.path().display());
        let config = Config::parse(&text, Path::new("/")).unwrap();
        let term = Arc::new(FakeTerminal::default());
        let hub = Hub::new(config, Store::memory().await.unwrap(), term.clone());
        (hub, term, dir)
    }

    async fn launch(hub: &Arc<Hub>) -> String {
        let req = LaunchRequest { name: Some("a".into()), provider: "claude".into(), model: None, role: Role::Manual, task_id: None, project: None, prompt: None };
        hub.launch(req).await.unwrap().name
    }

    #[tokio::test]
    async fn rate_limit_waits_for_reset_then_wakes_once() {
        let (hub, term, _dir) = setup().await;
        let name = launch(&hub).await;
        let mut w = Watcher::new(hub.clone());
        let t0 = Utc::now();
        term.set_pane(&name, "working...\nAPI Error: Rate limit reached. Try again in 10 minutes\n");
        w.tick(t0).await.unwrap();
        let s = hub.store.session(&name).await.unwrap().unwrap();
        assert_eq!(s.state, SessionState::RateLimited);
        assert!(hub.cooldown_until("claude").is_some());

        w.tick(t0 + chrono::Duration::minutes(5)).await.unwrap();
        assert!(term.sent(&name).is_empty(), "woke before the reset");

        w.tick(t0 + chrono::Duration::minutes(11)).await.unwrap();
        assert_eq!(term.sent(&name).len(), 1);
        let s = hub.store.session(&name).await.unwrap().unwrap();
        assert_eq!((s.state, s.wakeups), (SessionState::Working, 1));

        // The same old line still on screen is not a new hit.
        w.tick(t0 + chrono::Duration::minutes(12)).await.unwrap();
        assert_eq!(hub.store.session(&name).await.unwrap().unwrap().state, SessionState::Idle);

        // A second identical message is.
        term.set_pane(&name, "API Error: Rate limit reached. Try again in 10 minutes\nretrying\nAPI Error: Rate limit reached. Try again in 10 minutes\n");
        w.tick(t0 + chrono::Duration::minutes(13)).await.unwrap();
        assert_eq!(hub.store.session(&name).await.unwrap().unwrap().state, SessionState::RateLimited);
    }

    #[tokio::test]
    async fn wake_is_held_while_a_person_types() {
        let (hub, term, _dir) = setup().await;
        let name = launch(&hub).await;
        let mut w = Watcher::new(hub.clone());
        let t0 = Utc::now();
        term.set_pane(&name, "HTTP 429 Too Many Requests, retry after 30s\n");
        w.tick(t0).await.unwrap();
        term.set_pane(&name, "HTTP 429 Too Many Requests, retry after 30s\n❯ hold on");
        w.tick(t0 + chrono::Duration::minutes(1)).await.unwrap();
        assert!(term.sent(&name).is_empty());
        assert_eq!(hub.store.session(&name).await.unwrap().unwrap().state, SessionState::RateLimited);
    }

    #[tokio::test]
    async fn the_sign_in_helper_does_not_cool_the_provider_down() {
        let (hub, term, _dir) = setup().await;
        let req = LaunchRequest { name: Some("map-login-claude".into()), provider: "claude".into(), model: None, role: Role::Manual, task_id: None, project: None, prompt: None };
        let name = hub.launch(req).await.unwrap().name;
        assert_eq!(name, crate::login::session_name(&hub, "claude"));
        let mut w = Watcher::new(hub.clone());
        term.set_pane(&name, "Login expired · Please run /login\n");
        w.tick(Utc::now()).await.unwrap();
        assert!(hub.cooldown_until("claude").is_none());
    }

    #[tokio::test]
    async fn a_sign_in_or_a_cooldown_is_pushed_to_dashboards() {
        let dir = tempfile::tempdir().unwrap();
        let cred = dir.path().join("auth.json");
        let text = format!(
            "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[providers.codex]\ncredentials = [\"{1}\"]\napi_key_env = \"MAP_TEST_UNSET_KEY\"\n",
            dir.path().display(),
            cred.display()
        );
        let hub = Hub::new(Config::parse(&text, Path::new("/")).unwrap(), Store::memory().await.unwrap(), Arc::new(FakeTerminal::default()));
        let mut rx = hub.subscribe();
        let mut w = Watcher::new(hub.clone());
        let codex = |msg: ServerMessage| match msg {
            ServerMessage::Providers { providers } => providers.into_iter().find(|p| p.name == "codex"),
            _ => None,
        };

        w.tick(Utc::now()).await.unwrap();
        assert!(rx.try_recv().is_err(), "the first tick only records the list");

        std::fs::write(&cred, r#"{"tokens": {}}"#).unwrap();
        w.tick(Utc::now()).await.unwrap();
        assert_eq!(codex(rx.try_recv().unwrap()).unwrap().signed_in, Some(true));

        w.tick(Utc::now()).await.unwrap();
        assert!(rx.try_recv().is_err(), "nothing changed, nothing pushed");

        hub.cool_down("codex", Utc::now() + chrono::Duration::minutes(5));
        w.tick(Utc::now()).await.unwrap();
        assert!(codex(rx.try_recv().unwrap()).unwrap().cooldown_until.is_some());
    }

    #[tokio::test]
    async fn auth_prompt_and_exit_are_reported() {
        let (hub, term, _dir) = setup().await;
        let name = launch(&hub).await;
        let mut w = Watcher::new(hub.clone());
        term.set_pane(&name, "Login expired · Please run /login\n");
        w.tick(Utc::now()).await.unwrap();
        assert_eq!(hub.store.session(&name).await.unwrap().unwrap().state, SessionState::NeedsAuth);
        term.exit(&name);
        w.tick(Utc::now()).await.unwrap();
        assert_eq!(hub.store.session(&name).await.unwrap().unwrap().state, SessionState::Exited);
    }
}
