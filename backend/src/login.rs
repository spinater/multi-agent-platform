//! Provider sign-in: whether a provider is signed in (from its credential files, which are checked for existence and
//! parse-ability but never returned), and a helper tmux session that runs the provider's own login command so a
//! person can finish it in the browser. The pane parser pulls out the sign-in URL and one-time code.

use std::path::PathBuf;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use regex::Regex;

use crate::config::ProviderSpec;
use crate::hub::{Hub, LaunchRequest};
use crate::model::{Level, LoginInfo, LoginState, Role, Session};

static URL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"https?://[^\s<>"'`]+"#).unwrap());
/// Characters a URL wrapped onto the next line can consist of.
static URL_REST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9\-._~:/?#\[\]@!$&'()*+,;=%]+$").unwrap());
/// Device codes such as `UURC-1WR31` or `ABCD-EFGH`: dash-separated groups of capitals and digits.
static CODE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b([A-Z0-9]{4,}(?:-[A-Z0-9]{4,})+)\b").unwrap());

const MAX_CREDENTIAL_BYTES: u64 = 1 << 20;

/// `~/x` is `$HOME/x`; anything else is taken as is.
pub fn expand_home(path: &str) -> PathBuf {
    match (path.strip_prefix("~/"), std::env::var_os("HOME")) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// A credential file counts when it exists, is small, is not empty, and parses as JSON if it looks like JSON.
/// Its contents are dropped right away.
fn credential_ok(path: &PathBuf) -> bool {
    let Ok(meta) = std::fs::metadata(path) else { return false };
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_CREDENTIAL_BYTES {
        return false;
    }
    let Ok(bytes) = std::fs::read(path) else { return false };
    let looks_json = path.extension().is_some_and(|e| e == "json") || bytes.iter().find(|b| !b.is_ascii_whitespace()) == Some(&b'{');
    !looks_json || serde_json::from_slice::<serde_json::Value>(&bytes).is_ok()
}

fn api_key_set(hub: &Hub, spec: &ProviderSpec) -> bool {
    spec.api_key_env.as_deref().is_some_and(|key| {
        hub.config.workspace.env.get(key).is_some_and(|v| !v.is_empty()) || std::env::var(key).is_ok_and(|v| !v.is_empty())
    })
}

/// `Some(true)` with a valid credential file or an API key variable, `Some(false)` when credential files are
/// configured and none is valid, `None` when the platform cannot tell.
pub fn signed_in(hub: &Hub, spec: &ProviderSpec) -> Option<bool> {
    if spec.credentials.iter().any(|c| credential_ok(&expand_home(c))) || api_key_set(hub, spec) {
        return Some(true);
    }
    (!spec.credentials.is_empty()).then_some(false)
}

fn credentials_updated_at(spec: &ProviderSpec) -> Option<DateTime<Utc>> {
    spec.credentials.iter().filter_map(|c| std::fs::metadata(expand_home(c)).ok()?.modified().ok()).map(DateTime::<Utc>::from).max()
}

pub fn session_name(hub: &Hub, provider: &str) -> String {
    format!("{}login-{provider}", hub.config.workspace.session_prefix)
}

pub fn info(hub: &Hub, name: &str, spec: &ProviderSpec) -> LoginInfo {
    let command = spec.login.iter().map(|a| a.replace("{binary}", &spec.binary)).collect::<Vec<_>>().join(" ");
    LoginInfo {
        command,
        send: spec.login_send.clone(),
        session: session_name(hub, name),
        credentials: spec.credentials.iter().map(|c| expand_home(c).display().to_string()).collect(),
        api_key_env: spec.api_key_env.clone(),
        api_key_set: api_key_set(hub, spec),
        warning: spec.login_warning.clone(),
    }
}

/// What the sign-in modal needs from a login pane.
#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub url: Option<String>,
    pub code: Option<String>,
    pub wants_code: bool,
    pub success: bool,
}

/// Lines with a URL that a TUI hard-wrapped are joined back, so a long OAuth URL comes out whole.
fn join_wrapped(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    let mut in_url = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if in_url && !trimmed.is_empty() && URL_REST.is_match(trimmed) && !trimmed.starts_with("http") {
            out.last_mut().expect("in_url follows a line").push_str(trimmed);
            continue;
        }
        let ends_with_url = URL.find_iter(trimmed).last().is_some_and(|m| m.end() == trimmed.len());
        in_url = ends_with_url;
        out.push(trimmed.to_string());
    }
    out
}

pub fn parse(text: &str) -> Parsed {
    let lines = join_wrapped(text);
    let urls: Vec<String> = lines.iter().flat_map(|l| URL.find_iter(l).map(|m| m.as_str().trim_end_matches(['.', ',', ')']).to_string())).collect();
    let signin = |u: &&String| ["oauth", "authorize", "device", "login", "auth.", "/auth"].iter().any(|k| u.to_lowercase().contains(k));
    let url = urls.iter().rfind(signin).or(urls.last()).cloned();

    // A code is only looked for on a line that mentions a code, or the two lines after it, outside any URL. The first
    // one wins, and paste prompts are skipped: what follows them is what the person typed back.
    let pasted = |l: &str| l.to_lowercase().contains("paste");
    let code = lines.iter().enumerate().filter(|(_, l)| l.to_lowercase().contains("code") && !pasted(l)).find_map(|(i, _)| {
        lines.iter().skip(i).take(3).filter(|l| !pasted(l)).find_map(|candidate| {
            let plain = URL.replace_all(candidate, " ");
            CODE.captures_iter(&plain)
                .filter_map(|c| c.get(1))
                .find(|m| m.as_str().chars().any(|c| c.is_ascii_digit()) || m.as_str().len() >= 9)
                .map(|m| m.as_str().to_string())
        })
    });

    let lower = text.to_lowercase();
    let wants_code = ["paste code here", "paste the authorization code", "paste the code", "enter the authorization code", "authorization code here"]
        .iter()
        .any(|k| lower.contains(k));
    let success = ["login successful", "successfully logged in", "logged in successfully", "signed in successfully", "successfully signed in", "authentication successful"]
        .iter()
        .any(|k| lower.contains(k));
    Parsed { url, code, wants_code, success }
}

/// Starts (or returns the running) helper session for `provider`'s sign-in.
pub async fn start(hub: &Arc<Hub>, provider: &str) -> Result<Session> {
    let spec = hub.spec(provider)?.clone();
    if spec.login.is_empty() {
        bail!("provider `{provider}` has no login command; set providers.{provider}.login in map.toml");
    }
    if !hub.provider_available(provider) {
        bail!("`{}` is not on the workspace PATH", spec.binary);
    }
    let name = session_name(hub, provider);
    if let Some(existing) = hub.store.session(&name).await?
        && existing.state.is_live()
        && hub.term.exists(&name).await?
    {
        return Ok(existing);
    }
    let req = LaunchRequest { name: Some(name.clone()), provider: provider.into(), model: None, role: Role::Manual, task_id: None, project: None, prompt: None };
    let session = hub.launch_command(req, Some(&spec.login)).await?;
    if let Some(text) = spec.login_send.clone() {
        let (hub, target) = (Arc::clone(hub), name.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(spec.startup_secs)).await;
            if let Err(err) = hub.term.send_text(&target, &text, true).await {
                hub.event(Level::Error, "provider.login", Some(&target), None, format!("Could not type {text}: {err:#}")).await;
            }
        });
    }
    hub.event(Level::Info, "provider.login", Some(&name), None, format!("Sign-in for {provider} started")).await;
    Ok(session)
}

pub async fn state(hub: &Hub, provider: &str) -> Result<LoginState> {
    let spec = hub.spec(provider)?;
    let name = session_name(hub, provider);
    let session = hub.store.session(&name).await?;
    let text = match &session {
        Some(s) if s.state.is_live() && hub.term.exists(&name).await? => crate::detect::strip_ansi(&hub.term.capture(&name, 200, false).await?),
        Some(_) => std::fs::read_to_string(hub.run_dir(&name).join("final-pane.txt")).unwrap_or_default(),
        None => String::new(),
    };
    let parsed = parse(&text);
    let lines: Vec<&str> = text.trim_end().lines().collect();
    let tail = lines[lines.len().saturating_sub(40)..].join("\n");
    Ok(LoginState {
        session,
        text: tail,
        url: parsed.url,
        code: parsed.code,
        wants_code: parsed.wants_code,
        success: parsed.success,
        signed_in: signed_in(hub, spec),
        credentials_updated_at: credentials_updated_at(spec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captured from `codex login --device-auth` (codex-cli 0.159.2) in a 200-column tmux pane.
    const CODEX: &str = "Welcome to Codex [v0.159.2]\nOpenAI's command-line coding agent\n\nFollow these steps to sign in with ChatGPT using device code authorization:\n\n1. Open this link in your browser and sign in to your account\n   https://auth.openai.com/codex/device\n\n2. Enter this one-time code (expires in 15 minutes)\n   UURC-1WR31\n\nContinue only if you started this login in Codex. If a website or another person gave you this code, cancel.\n";

    /// Modelled on `claude auth login` (not captured: running it would replace the sign-in), its URL hard-wrapped.
    const CLAUDE: &str = "Opening browser to sign in…\nBrowser didn't open? Use the url below to sign in (c to copy)\n\nhttps://claude.ai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile\n&code_challenge=abc123&code_challenge_method=S256&state=xyz789\n\nPaste code here if prompted >\n";

    /// agy's strings for its `/login` flow (from the agy 1.2.10 binary).
    const AGY: &str = "Please visit the following URL to authorize the application:\n\nhttps://accounts.google.com/o/oauth2/auth?access_type=offline&client_id=123.apps.googleusercontent.com&response_type=code&state=s1\n\nOr, paste the authorization code here and press Enter:\n";

    #[test]
    fn parses_codex_device_code() {
        let p = parse(CODEX);
        assert_eq!(p.url.as_deref(), Some("https://auth.openai.com/codex/device"));
        assert_eq!(p.code.as_deref(), Some("UURC-1WR31"));
        assert!(!p.wants_code && !p.success);
        // What a person types back after a paste prompt is never taken for the CLI's code.
        let echoed = format!("{CODEX}Paste code here if prompted > PASTED-CODE9\ngot: PASTED-CODE9\n");
        assert_eq!(parse(&echoed).code.as_deref(), Some("UURC-1WR31"));
        assert_eq!(parse("Paste the code here > ABCD-12345\n").code, None);
    }

    #[test]
    fn joins_a_wrapped_oauth_url_and_sees_the_paste_prompt() {
        let p = parse(CLAUDE);
        let url = p.url.unwrap();
        assert!(url.starts_with("https://claude.ai/oauth/authorize?code=true"), "{url}");
        assert!(url.ends_with("&state=xyz789"), "{url}");
        assert_eq!(p.code, None, "nothing in a URL is a code");
        assert!(p.wants_code);
    }

    #[test]
    fn parses_agy_and_success() {
        let p = parse(AGY);
        assert!(p.url.unwrap().starts_with("https://accounts.google.com/o/oauth2/auth?"));
        assert!(p.wants_code);
        assert!(parse("Login successful. Press Enter to continue").success);
        assert!(!parse(CODEX).success);
    }

    #[test]
    fn credential_files_count_only_when_they_parse() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("auth.json");
        std::fs::write(&good, r#"{"tokens": "x"}"#).unwrap();
        let bad = dir.path().join("broken.json");
        std::fs::write(&bad, "{not json").unwrap();
        let plain = dir.path().join("token");
        std::fs::write(&plain, "opaque").unwrap();
        assert!(credential_ok(&good));
        assert!(!credential_ok(&bad));
        assert!(credential_ok(&plain));
        assert!(!credential_ok(&dir.path().join("missing.json")));
    }

    #[tokio::test]
    async fn start_runs_the_login_command_in_a_prefixed_helper_session() {
        use crate::terminal::fake::FakeTerminal;
        let dir = tempfile::tempdir().unwrap();
        let cred = dir.path().join("auth.json");
        let text = format!(
            "[workspace]\ndir = \"{0}\"\nsession_prefix = \"agp-\"\n[server]\ndata_dir = \"{0}/data\"\n[providers.codex]\nbinary = \"/bin/sh\"\ncredentials = [\"{1}\"]\nstartup_secs = 0\n[providers.gemini]\nbinary = \"/bin/sh\"\nlogin = []\n",
            dir.path().display(),
            cred.display()
        );
        let config = crate::config::Config::parse(&text, std::path::Path::new("/")).unwrap();
        let term = Arc::new(FakeTerminal::default());
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), term.clone());

        assert_eq!(signed_in(&hub, hub.spec("codex").unwrap()), Some(false));
        let session = start(&hub, "codex").await.unwrap();
        assert_eq!(session.name, "agp-login-codex");
        let script = term.sessions.lock().unwrap()["agp-login-codex"].script.clone();
        assert!(script.contains("exec /bin/sh login --device-auth\n"), "{script}");
        // A second start reuses the running helper.
        assert_eq!(start(&hub, "codex").await.unwrap().created_at, session.created_at);
        assert!(start(&hub, "gemini").await.is_err(), "no login command");

        std::fs::write(&cred, "{}").unwrap();
        let st = state(&hub, "codex").await.unwrap();
        assert_eq!(st.signed_in, Some(true));
        assert!(st.credentials_updated_at.is_some());
        assert_eq!(info(&hub, "codex", hub.spec("codex").unwrap()).command, "/bin/sh login --device-auth");
    }

    #[test]
    fn home_expansion() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_home("~/.codex/auth.json"), PathBuf::from(home).join(".codex/auth.json"));
        assert_eq!(expand_home("/etc/x"), PathBuf::from("/etc/x"));
    }
}
