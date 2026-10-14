//! The Settings screen's read-only facts (`GET /api/settings`): the server's settings from `map.toml`, how MAP was
//! started (a container, a systemd or launchd service, by hand), where each agent CLI's binary is, and whether `gh`
//! and `glab` are on the agents' PATH with their token variables set. Secrets never leave: `api_token` and every
//! variable are reported as set or not, never by value. `map.toml` is not editable from here.

use std::path::Path;

use crate::hub::Hub;
use crate::model::{AgentBinary, AgentsHome, AgentsHomeMode, EnvVarState, GitCliTool, RunMode, ServerSettings, SettingsInfo};

/// Files a container runtime leaves at the root: Docker's `/.dockerenv`, Podman's `/run/.containerenv`.
const CONTAINER_MARKERS: [&str; 2] = ["/.dockerenv", "/run/.containerenv"];

/// MAP runs in a container when any of `markers` exists.
pub fn in_container_at(markers: &[&Path]) -> bool {
    markers.iter().any(|m| m.exists())
}

pub fn in_container() -> bool {
    in_container_at(&CONTAINER_MARKERS.map(Path::new))
}

/// How MAP was started: a container first, then systemd (it sets `INVOCATION_ID` for every unit it starts), then
/// launchd (`XPC_SERVICE_NAME`, which is `0` in a login shell), else by hand.
pub fn run_mode(in_container: bool, env: impl Fn(&str) -> Option<String>) -> RunMode {
    let set = |k: &str| env(k).is_some_and(|v| !v.is_empty() && v != "0");
    if in_container {
        RunMode::Docker
    } else if set("INVOCATION_ID") {
        RunMode::Systemd
    } else if set("XPC_SERVICE_NAME") {
        RunMode::Launchd
    } else {
        RunMode::Manual
    }
}

/// The command that restarts MAP after editing `map.toml` (by hand: the command to start it again after Ctrl-C).
pub fn restart_hint(mode: RunMode, config_path: Option<&str>) -> String {
    match mode {
        RunMode::Docker => "docker compose restart map".into(),
        RunMode::Systemd => "systemctl --user restart map".into(),
        RunMode::Launchd => "launchctl kickstart -k gui/$(id -u)/com.multi-agent-platform.map".into(),
        RunMode::Manual => format!("map serve -c {}", config_path.unwrap_or("map.toml")),
    }
}

/// The command that attaches to a session by hand, `<session>` standing for its name.
pub fn attach_command(socket: Option<&str>) -> String {
    match socket {
        Some(s) => format!("tmux -L {s} attach -t '=<session>'"),
        None => "tmux attach -t '=<session>'".into(),
    }
}

/// `name` is set (non-empty) in `[workspace.env]` and/or MAP's own environment. Never the value.
fn var(hub: &Hub, name: &str, purpose: &str) -> EnvVarState {
    EnvVarState {
        name: name.into(),
        purpose: purpose.into(),
        in_workspace_env: hub.config.workspace.env.get(name).is_some_and(|v| !v.is_empty()),
        in_server_env: std::env::var_os(name).is_some_and(|v| !v.is_empty()),
    }
}

pub fn info(hub: &Hub) -> SettingsInfo {
    let c = &hub.config;
    let config_path = c.path.as_ref().map(|p| p.display().to_string());
    let mode = run_mode(in_container(), |k| std::env::var(k).ok());
    let path = |binary: &str| hub.binary_path(binary).map(|p| p.display().to_string());
    // Relative paths in a config given as a relative file resolve against MAP's working directory.
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).display().to_string();
    let server = ServerSettings {
        version: env!("CARGO_PKG_VERSION").into(),
        restart_hint: restart_hint(mode, config_path.as_deref()),
        config_path,
        data_dir: abs(&c.server.data_dir),
        bind: c.server.bind.clone(),
        allowed_hosts: c.server.allowed_hosts.clone(),
        api_token_set: c.server.api_token.is_some(),
        config_dir: c.path.as_ref().and_then(|p| p.parent()).map(|p| p.display().to_string()),
        projects_root: abs(&c.projects.default_root().path),
        project_roots: crate::roots::list_roots(c.projects.roots(), &c.projects.default_root().name),
        workspace_dir: abs(&c.workspace.dir),
        session_prefix: c.workspace.session_prefix.clone(),
        static_dir: abs(&c.server.static_dir),
        static_dir_ok: c.server.static_dir.join("index.html").is_file(),
        reflex_binary: c.reflex.binary.clone(),
        reflex_version: hub.reflex.version(&c.reflex.binary),
        run_mode: mode,
        install_mode: c.server.mode,
        tmux_socket: c.workspace.tmux_socket.clone(),
        attach_command: attach_command(c.workspace.tmux_socket.as_deref()),
    };
    let git_cli = vec![
        GitCliTool {
            name: "gh".into(),
            title: "GitHub CLI".into(),
            path: path("gh"),
            vars: vec![var(hub, "GH_TOKEN", "token for github.com (GITHUB_TOKEN also works)"), var(hub, "GITHUB_TOKEN", "older name gh also reads")],
        },
        GitCliTool {
            name: "glab".into(),
            title: "GitLab CLI".into(),
            path: path("glab"),
            vars: vec![var(hub, "GITLAB_TOKEN", "token for GitLab"), var(hub, "GITLAB_HOST", "self-hosted GitLab's address (not secret); unset: gitlab.com")],
        },
    ];
    let agents = c
        .providers
        .iter()
        .map(|(name, spec)| AgentBinary {
            provider: name.clone(),
            binary: spec.binary.clone(),
            path: path(&spec.binary),
            home: hub.provider_home(name).map(|p| p.display().to_string()),
            home_env: hub.config.provider_home_export(name).map(|(k, v)| format!("{k}={v}")),
        })
        .collect();
    let agents_home = AgentsHome {
        mode: if c.agents_home.isolated() { AgentsHomeMode::Isolated } else { AgentsHomeMode::Shared },
        dir: abs(c.agents_home.dir()),
    };
    SettingsInfo { server, git_cli, agents, agents_home }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn container_detection_reads_the_marker_files() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join(".dockerenv");
        assert!(!in_container_at(&[&marker, &dir.path().join("missing")]));
        std::fs::write(&marker, "").unwrap();
        assert!(in_container_at(&[&dir.path().join("missing"), &marker]));
    }

    #[test]
    fn run_mode_prefers_the_container_then_systemd_then_launchd() {
        let env = |pairs: &[(&str, &str)]| {
            let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            move |k: &str| m.get(k).cloned()
        };
        assert_eq!(run_mode(true, env(&[("INVOCATION_ID", "abc")])), RunMode::Docker);
        assert_eq!(run_mode(false, env(&[("INVOCATION_ID", "abc")])), RunMode::Systemd);
        assert_eq!(run_mode(false, env(&[("XPC_SERVICE_NAME", "com.multi-agent-platform.map")])), RunMode::Launchd);
        assert_eq!(run_mode(false, env(&[("XPC_SERVICE_NAME", "0")])), RunMode::Manual, "a macOS login shell");
        assert_eq!(run_mode(false, env(&[])), RunMode::Manual);
        assert!(restart_hint(RunMode::Manual, Some("/etc/map.toml")).contains("map serve -c /etc/map.toml"));
        assert!(restart_hint(RunMode::Docker, None).contains("docker compose restart"));
    }

    #[test]
    fn attach_command_names_the_private_socket() {
        assert_eq!(attach_command(None), "tmux attach -t '=<session>'");
        assert_eq!(attach_command(Some("map")), "tmux -L map attach -t '=<session>'");
    }

    #[tokio::test]
    async fn agent_homes_and_the_tmux_socket_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[server]\ndata_dir = \"{0}/data\"\n[workspace]\ndir = \"{0}\"\ntmux_socket = \"map-t\"\n[agents_home]\nmode = \"isolated\"\ndir = \"{0}/ah\"\n",
            dir.path().display()
        );
        let config = crate::config::Config::parse(&text, Path::new("/")).unwrap();
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        let info = info(&hub);
        assert_eq!(info.agents_home.mode, AgentsHomeMode::Isolated);
        assert_eq!(info.agents_home.dir, dir.path().join("ah").display().to_string());
        let claude = info.agents.iter().find(|a| a.provider == "claude").unwrap();
        assert_eq!(claude.home.as_deref(), Some(dir.path().join("ah/claude").display().to_string().as_str()));
        assert_eq!(claude.home_env, Some(format!("CLAUDE_CONFIG_DIR={}/ah/claude", dir.path().display())));
        assert_eq!(info.server.tmux_socket.as_deref(), Some("map-t"));
        assert_eq!(info.server.attach_command, "tmux -L map-t attach -t '=<session>'");
        assert_eq!(hub.server_info().tmux_socket.as_deref(), Some("map-t"));
    }

    #[tokio::test]
    async fn settings_never_carry_the_api_token_or_variable_values() {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[server]\napi_token = \"tok-SECRET-api-123\"\ndata_dir = \"{0}/data\"\nallowed_hosts = [\"agents.example.com\"]\n[workspace]\ndir = \"{0}\"\n[workspace.env]\nGH_TOKEN = \"ghp_SECRETvalue456\"\nGITLAB_HOST = \"gitlab.example.com\"\n",
            dir.path().display()
        );
        let config = crate::config::Config::parse(&text, Path::new("/")).unwrap();
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        let info = info(&hub);
        let json = serde_json::to_string(&info).unwrap();
        for secret in ["tok-SECRET-api-123", "ghp_SECRETvalue456", "gitlab.example.com"] {
            assert!(!json.contains(secret), "{secret} leaked: {json}");
        }
        assert!(info.server.api_token_set);
        assert_eq!(info.server.allowed_hosts, ["agents.example.com"]);
        let gh = info.git_cli.iter().find(|t| t.name == "gh").unwrap();
        let token = gh.vars.iter().find(|v| v.name == "GH_TOKEN").unwrap();
        assert!(token.in_workspace_env);
        let glab = info.git_cli.iter().find(|t| t.name == "glab").unwrap();
        assert!(glab.vars.iter().any(|v| v.name == "GITLAB_HOST" && v.in_workspace_env));
        assert!(glab.vars.iter().any(|v| v.name == "GITLAB_TOKEN" && !v.in_workspace_env));
        assert_eq!(info.agents.len(), hub.config.providers.len());
    }
}
