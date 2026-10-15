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

/// Whether the provider's key variable has a value: in `[workspace.env]`, MAP's environment, or the provider's own
/// `env` (a literal, or a reference to a variable that is set). Only ever a boolean: the value is not kept.
fn api_key_set(hub: &Hub, spec: &ProviderSpec) -> bool {
    spec.api_key_env.as_deref().is_some_and(|key| {
        hub.config.env_value(key).is_some()
            || spec.env.get(key).is_some_and(|v| match crate::config::env_ref(v) {
                Some(r) => hub.config.env_value(r).is_some(),
                None => !v.is_empty(),
            })
    })
}

/// `Some(true)` with a valid credential file or an API key variable, `Some(false)` when credential files are
/// configured and none is valid (or the provider signs in by its key alone, with no sign-in command, and the key is
/// not set), `None` when the platform cannot tell.
pub fn signed_in(hub: &Hub, spec: &ProviderSpec) -> Option<bool> {
    if spec.credentials.iter().any(|c| credential_ok(&expand_home(c))) || api_key_set(hub, spec) {
        return Some(true);
    }
    let key_only = spec.login.is_empty() && spec.api_key_env.is_some();
    (!spec.credentials.is_empty() || key_only).then_some(false)
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
    let wants_code = ["paste code here", "paste the authorization code", "paste the code", "enter the authorization code", "authorization code here", "paste it below", "authorization code..."]
        .iter()
        .any(|k| lower.contains(k));
    let success = ["login successful", "successfully logged in", "logged in successfully", "signed in successfully", "successfully signed in", "authentication successful"]
        .iter()
        .any(|k| lower.contains(k));
    Parsed { url, code, wants_code, success }
}

/// What a sign-in pane shows before `login_send` can be typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// The CLI's input prompt (`login_ready`, else `idle_markers`) is in the bottom lines.
    Ready,
    /// One of the provider's known first-run pickers (`login_onboarding`): Enter takes the default.
    Onboarding,
    /// A folder-trust or permission question: a person must answer it.
    Blocked,
    /// Nothing recognised yet (starting, or a screen MAP does not know).
    Unknown,
}

/// The bottom non-empty lines of a pane, ANSI stripped, joined.
fn bottom(pane: &str, n: usize) -> String {
    let text = crate::detect::strip_ansi(pane);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

pub fn readiness(spec: &ProviderSpec, pane: &str) -> Readiness {
    if crate::detect::trust_prompt(spec, pane).is_some() {
        return Readiness::Blocked;
    }
    let tail = bottom(pane, 25);
    let lower = tail.to_lowercase();
    let blocked = ["do you want", "allow", "permission", "(y/n)", "trust"];
    let ready = if spec.login_ready.is_empty() { &spec.idle_markers } else { &spec.login_ready };
    // The picker first: a welcome screen may show the prompt's placeholder under it.
    if spec.login_onboarding.iter().any(|m| !m.is_empty() && tail.contains(m.as_str())) {
        // A known picker that also asks about trust or permissions is not harmless.
        return if blocked.iter().any(|b| lower.contains(b)) { Readiness::Blocked } else { Readiness::Onboarding };
    }
    if ready.iter().any(|m| !m.is_empty() && tail.contains(m.as_str())) {
        return Readiness::Ready;
    }
    Readiness::Unknown
}

/// The sign-in's phase as the modal shows it, and a hint for the person.
pub fn phase(spec: &ProviderSpec, pane: &str, parsed: &Parsed, live: bool) -> (String, Option<String>) {
    let (p, hint): (&str, Option<&str>) = if parsed.success {
        ("success", None)
    } else if bottom(pane, 4).to_lowercase().contains("got an error") {
        // agy after a wrong or expired code: "Got an error: token exchange failed … Press any key to go back."
        ("failed", Some("The CLI refused the code (wrong or expired). Start the sign-in again and paste the new code."))
    } else if spec.login_consent.iter().any(|m| !m.is_empty() && bottom(pane, 40).contains(m.as_str())) {
        // A terms / data-use question: the person decides; MAP never accepts it for them.
        ("consent", Some("The CLI asks you to accept its terms, and whether Google may use your interactions (the [x] box, on by default). Decide below: Enter toggles the box, ↓ moves to [Done], Enter again finishes."))
    } else if parsed.wants_code {
        ("waiting_for_code", Some("Paste the code the browser gave you."))
    } else if parsed.url.is_some() || parsed.code.is_some() {
        ("browser", Some("Open the link and approve the sign-in in your browser."))
    } else if !live {
        ("ended", Some("The sign-in session has ended."))
    } else if pane.trim().is_empty() {
        ("starting", None)
    } else {
        match readiness(spec, pane) {
            Readiness::Onboarding => ("onboarding", Some("The CLI is asking a first-run question; MAP answers it with the default.")),
            Readiness::Ready => ("ready", None),
            Readiness::Blocked => ("console", Some("The CLI is asking a question only you can answer: use the console below.")),
            Readiness::Unknown => ("console", Some("The CLI is waiting for you in the console below.")),
        }
    };
    (p.to_string(), hint.map(String::from))
}

/// How long the helper waits for the CLI's prompt before typing `login_send`, and how often it looks.
pub const READY_WAIT: Duration = Duration::from_secs(60);
const READY_POLL: Duration = Duration::from_millis(1000);

/// Waits for the CLI's input prompt, answering known first-run pickers with Enter, then types `login_send`. Gives up
/// (with an event) on a trust or permission question, or after `wait`.
pub async fn send_when_ready(hub: &Hub, spec: &ProviderSpec, target: &str, text: &str, wait: Duration, poll: Duration) -> Result<bool> {
    let deadline = tokio::time::Instant::now() + wait;
    let mut answered = 0;
    loop {
        tokio::time::sleep(poll).await;
        let pane = hub.term.capture(target, 60, false).await.unwrap_or_default();
        // Some CLIs open straight onto the sign-in (agy signed out): a link or a code prompt means it is under way.
        let shown = parse(&crate::detect::strip_ansi(&pane));
        if shown.url.is_some() || shown.wants_code || shown.success {
            return Ok(true);
        }
        if spec.login_consent.iter().any(|m| !m.is_empty() && bottom(&pane, 40).contains(m.as_str())) {
            hub.event(Level::Info, "provider.login", Some(target), None, "The CLI asks you to accept its terms; answer it in the sign-in console").await;
            return Ok(false);
        }
        match readiness(spec, &pane) {
            Readiness::Ready => {
                hub.term.send_text(target, text, true).await?;
                return Ok(true);
            }
            Readiness::Onboarding if answered < 10 => {
                answered += 1;
                hub.term.send_key(target, "Enter").await?;
                hub.event(Level::Info, "provider.login", Some(target), None, "The CLI asked a first-run question; answered with the default").await;
            }
            Readiness::Blocked => {
                hub.event(Level::Warning, "provider.login", Some(target), None, format!("The CLI asks a question MAP does not answer; type {text} yourself after answering it")).await;
                return Ok(false);
            }
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            hub.event(Level::Warning, "provider.login", Some(target), None, format!("The CLI's prompt did not appear; type {text} in its console")).await;
            return Ok(false);
        }
    }
}

/// Types the authorization code into the sign-in session, only while its pane asks for one.
pub async fn send_code(hub: &Hub, provider: &str, code: &str) -> Result<()> {
    let name = session_name(hub, provider);
    hub.managed(&name).await?;
    let pane = crate::detect::strip_ansi(&hub.term.capture(&name, 200, false).await?);
    // The code prompt must be the last thing on screen: an old one above a typed code does not count.
    // agy keeps a key hint under its input, whose empty placeholder reads "authorization code...".
    let last = bottom(&pane, 1);
    let empty_input = bottom(&pane, 3).to_lowercase().contains("authorization code...");
    let parsed = parse(&pane);
    if !parsed.wants_code || parsed.success || !(parse(&last).wants_code || empty_input) {
        bail!("the sign-in is not asking for a code right now");
    }
    let code = code.trim();
    if code.is_empty() || code.contains(char::is_whitespace) || code.len() > 4096 {
        bail!("the code is one word");
    }
    hub.term.send_text(&name, code, true).await
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
            // Not blindly after `startup_secs`: a first-run picker would swallow the command and the code after it.
            if let Err(err) = send_when_ready(&hub, &spec, &target, &text, READY_WAIT, READY_POLL).await {
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
    let live = session.as_ref().is_some_and(|s| s.state.is_live());
    let (phase, hint) = phase(spec, &text, &parsed, live);
    if parsed.success || session.as_ref().is_some_and(|s| !s.state.is_live()) {
        hub.announce_providers_if_changed();
    }
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
        phase,
        hint,
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

    /// agy signed out, as captured from the real CLI (Antigravity, 2026-10): it opens on a login-method picker, then a
    /// link and an input whose placeholder is the prompt, with a key hint under it.
    #[test]
    fn real_agy_signed_out_screens() {
        let spec = crate::config::builtin_providers()["gemini"].clone();
        let picker = " Welcome to the Antigravity CLI. You are currently not signed in.\n Select login method:\n > 1. Google OAuth\n   2. Use a Google Cloud project\n   ↑/↓ Navigate · enter Select\n";
        assert_eq!(readiness(&spec, picker), Readiness::Onboarding, "Enter takes Google OAuth");
        let code = " Open the URL below in your browser:\n https://accounts.google.com/o/oauth2/auth?access_type=offline&state=s\n After authenticating, copy the code displayed in the browser and paste it below:\n authorization code...\n  shift+up/down Navigate\n";
        let p = parse(code);
        assert!(p.url.is_some() && p.wants_code);
        assert!(bottom(code, 3).to_lowercase().contains("authorization code..."));
        let refused = " Got an error: token exchange failed: oauth2: \"invalid_grant\" \"Bad Request\"\n Press any key to go back.\n  shift+up/down Navigate\n";
        assert_eq!(phase(&spec, refused, &parse(refused), true).0, "failed");
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

    #[tokio::test]
    async fn isolated_mode_signs_in_and_checks_credentials_in_maps_own_folder() {
        use crate::terminal::fake::FakeTerminal;
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().display();
        let config = |mode: &str| {
            let text = format!(
                "[workspace]\ndir = \"{d}\"\n[server]\ndata_dir = \"{d}/data\"\n[agents_home]\nmode = \"{mode}\"\ndir = \"{d}/ah\"\n[providers.claude]\nbinary = \"/bin/sh\"\nstartup_secs = 0\napi_key_env = \"\"\n[providers.codex]\nbinary = \"/bin/sh\"\nstartup_secs = 0\napi_key_env = \"\"\n[providers.gemini]\nbinary = \"/bin/sh\"\nlogin_send = \"\"\nstartup_secs = 0\napi_key_env = \"\"\n"
            );
            crate::config::Config::parse(&text, std::path::Path::new("/")).unwrap()
        };
        let term = Arc::new(FakeTerminal::default());
        let hub = Hub::new(config("isolated"), crate::store::Store::memory().await.unwrap(), term.clone());
        let ah = dir.path().join("ah");
        // Nothing is copied over: not signed in until the helper session has run in MAP's folder.
        assert_eq!(signed_in(&hub, hub.spec("codex").unwrap()), Some(false));
        assert_eq!(info(&hub, "codex", hub.spec("codex").unwrap()).credentials, [ah.join("codex/auth.json").display().to_string()]);
        for (provider, export) in [
            ("codex", format!("export CODEX_HOME={}/codex\n", ah.display())),
            ("claude", format!("export CLAUDE_CONFIG_DIR={}/claude\n", ah.display())),
            ("gemini", format!("export HOME={}/gemini-home\n", ah.display())),
        ] {
            start(&hub, provider).await.unwrap();
            let script = term.sessions.lock().unwrap()[&format!("map-login-{provider}")].script.clone();
            assert!(script.contains(&export), "{script}");
        }
        std::fs::create_dir_all(ah.join("codex")).unwrap();
        std::fs::write(ah.join("codex/auth.json"), "{}").unwrap();
        assert_eq!(signed_in(&hub, hub.spec("codex").unwrap()), Some(true));
        std::fs::create_dir_all(ah.join("gemini-home/.gemini")).unwrap();
        std::fs::write(ah.join("gemini-home/.gemini/jetski-standalone-oauth-token"), "t").unwrap();
        assert_eq!(signed_in(&hub, hub.spec("gemini").unwrap()), Some(true));

        // Shared mode: the same launch exports none of them.
        let term = Arc::new(FakeTerminal::default());
        let hub = Hub::new(config("shared"), crate::store::Store::memory().await.unwrap(), term.clone());
        start(&hub, "codex").await.unwrap();
        let script = term.sessions.lock().unwrap()["map-login-codex"].script.clone();
        for var in ["CODEX_HOME", "CLAUDE_CONFIG_DIR", "export HOME", "CARGO_HOME"] {
            assert!(!script.contains(var), "{var}: {script}");
        }
    }

    #[tokio::test]
    async fn openrouter_is_signed_in_by_its_key_and_launches_with_it_by_reference() {
        use crate::terminal::fake::FakeTerminal;
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path().display();
        // A key variable no environment has, so the test does not depend on the machine.
        let config = |extra: &str| {
            let text = format!(
                "[workspace]\ndir = \"{d}\"\n{extra}\n[server]\ndata_dir = \"{d}/data\"\n[agents_home]\nmode = \"isolated\"\ndir = \"{d}/ah\"\n[providers.openrouter]\nbinary = \"/bin/sh\"\napi_key_env = \"MAP_TEST_OR_KEY_7F3\"\nenv = {{ ANTHROPIC_BASE_URL = \"https://openrouter.ai/api\", ANTHROPIC_AUTH_TOKEN = \"$MAP_TEST_OR_KEY_7F3\", ANTHROPIC_API_KEY = \"\" }}\n"
            );
            crate::config::Config::parse(&text, std::path::Path::new("/")).unwrap()
        };
        let term = Arc::new(FakeTerminal::default());
        let hub = Hub::new(config(""), crate::store::Store::memory().await.unwrap(), term.clone());
        let spec = hub.spec("openrouter").unwrap();
        assert_eq!(signed_in(&hub, spec), Some(false), "no sign-in command: the key is the sign-in");
        assert!(start(&hub, "openrouter").await.is_err(), "nothing to sign in with");
        let info = info(&hub, "openrouter", spec);
        assert_eq!((info.command.as_str(), info.api_key_env.as_deref(), info.api_key_set), ("", Some("MAP_TEST_OR_KEY_7F3"), false));

        hub.launch(LaunchRequest { name: Some("or".into()), provider: "openrouter".into(), model: None, role: Role::Manual, task_id: None, project: None, prompt: Some("hi".into()) }).await.unwrap();
        let script = term.sessions.lock().unwrap()["map-or"].script.clone();
        let at = |needle: &str| script.find(needle).unwrap_or_else(|| panic!("{needle} missing: {script}"));
        at("export ANTHROPIC_AUTH_TOKEN=\"$MAP_TEST_OR_KEY_7F3\"\n");
        at("export ANTHROPIC_BASE_URL=https://openrouter.ai/api\n");
        at("export ANTHROPIC_API_KEY=''\n");
        at(&format!("export CLAUDE_CONFIG_DIR={}/ah/openrouter\n", dir.path().display()));
        assert!(at("ANTHROPIC_AUTH_TOKEN") < at("exec "), "exported before the CLI starts");
        assert!(script.contains("--model '~anthropic/claude-sonnet-latest'"), "{script}");

        // The key in [workspace.env]: signed in, and only ever a boolean in what the dashboard gets.
        let hub = Hub::new(config("[workspace.env]\nMAP_TEST_OR_KEY_7F3 = \"sk-or-v1-SECRET-42\""), crate::store::Store::memory().await.unwrap(), term.clone());
        assert_eq!(signed_in(&hub, hub.spec("openrouter").unwrap()), Some(true));
        let json = serde_json::to_string(&hub.providers()).unwrap();
        assert!(!json.contains("SECRET"), "{json}");
        assert!(json.contains("\"api_key_set\":true"), "{json}");
    }

    #[test]
    fn home_expansion() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(expand_home("~/.codex/auth.json"), PathBuf::from(home).join(".codex/auth.json"));
        assert_eq!(expand_home("/etc/x"), PathBuf::from("/etc/x"));
    }

    fn agy() -> ProviderSpec {
        crate::config::builtin_providers()["gemini"].clone()
    }

    const PICKER: &str = "Welcome to Antigravity CLI!\nChoose your color scheme:\n > dark\n   light\n[Next]\n↑/↓ Navigate · enter Confirm\n";

    #[test]
    fn readiness_answers_only_known_pickers() {
        let spec = agy();
        assert_eq!(readiness(&spec, PICKER), Readiness::Onboarding);
        assert_eq!(readiness(&spec, "Welcome back\n\n > Type your message or @path/to/file\n"), Readiness::Ready);
        assert_eq!(readiness(&spec, "Do you trust the contents of this project?\n > Yes, I trust this folder\n   No\n"), Readiness::Blocked, "trust: never Enter");
        assert_eq!(readiness(&spec, "Choose your color scheme\nAllow telemetry? (y/n)\n[Next]\n"), Readiness::Blocked, "a picker that asks permission");
        assert_eq!(readiness(&spec, "loading…\n"), Readiness::Unknown);
        let parsed = parse(PICKER);
        assert_eq!(phase(&spec, PICKER, &parsed, true).0, "onboarding");
        assert_eq!(phase(&spec, "something odd\n", &parse("something odd"), true).0, "console");
        let code = "Please visit https://accounts.google.com/o/oauth2/auth?x=1\nOr, paste the authorization code here and press Enter:\n";
        assert_eq!(phase(&spec, code, &parse(code), true).0, "waiting_for_code");
        assert_eq!(phase(&spec, "Login successful", &parse("Login successful"), true).0, "success");
        let tos = "Terms of Service & Data Use\n  > [x] Yes, I agree to help improve Antigravity CLI by allowing\n      Google to collect and use my Interactions data\n    [Previous]      [Done]\n  ↑/↓ Navigate · enter Toggle\n";
        assert_eq!(phase(&spec, tos, &parse(tos), true).0, "consent", "the person decides; MAP never accepts terms");
        assert_ne!(readiness(&spec, tos), Readiness::Onboarding, "never answered with Enter");
    }

    /// A fake `agy` on a private tmux socket: the colour-scheme picker, then the prompt, `/login`, the code prompt,
    /// and a credential file written from the code.
    #[tokio::test]
    async fn agy_sign_in_gets_past_the_first_run_picker() {
        if std::process::Command::new("tmux").arg("-V").output().is_err() {
            eprintln!("tmux missing: skipped");
            return;
        }
        let socket = format!("map-login-test-{}", std::process::id());
        let tmux = crate::lanes::tests::PrivateTmux(socket.clone());
        let dir = tempfile::tempdir().unwrap();
        let cred = dir.path().join("token.json");
        let script = dir.path().join("fake-agy.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/bash\nprintf 'Welcome to Antigravity CLI!\\nChoose your color scheme:\\n > dark\\n   light\\n[Next]\\n  Navigate · enter Confirm\\n'\nread -r first\nprintf '%s\\n' \"$first\" > {d}/first.log\nclear\nprintf 'Type your message or @path/to/file\\n> '\nread -r cmd\nprintf '%s\\n' \"$cmd\" > {d}/cmd.log\nif [ \"$cmd\" = /login ]; then\n  printf 'Please visit https://accounts.google.com/o/oauth2/auth?client_id=x\\nOr, paste the authorization code here and press Enter:\\n'\n  read -r code\n  printf '{{\"token\": \"%s\"}}' \"$code\" > {cred}\n  printf 'Login successful\\n'\nfi\nsleep 60\n",
                d = dir.path().display(),
                cred = cred.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let text = format!(
            "[workspace]\ndir = \"{d}\"\ntmux_socket = \"{socket}\"\n[server]\ndata_dir = \"{d}/data\"\n[agents_home]\nmode = \"isolated\"\ndir = \"{d}/homes\"\n[providers.gemini]\nbinary = \"{s}\"\ncredentials = [\"{c}\"]\nstartup_secs = 0\n",
            d = dir.path().display(),
            s = script.display(),
            c = cred.display()
        );
        let config = crate::config::Config::parse(&text, std::path::Path::new("/")).unwrap();
        for d in config.create_agent_homes().unwrap() {
            assert!(d.starts_with(dir.path()));
        }
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::tmux::Tmux::new(Some(socket.clone()))));
        let s = start(&hub, "gemini").await.unwrap();
        let mut ok = false;
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(500)).await;
            if std::fs::read_to_string(dir.path().join("cmd.log")).is_ok_and(|c| c.trim() == "/login") {
                ok = true;
                break;
            }
        }
        assert!(ok, "the picker was answered and /login typed after it; pane:\n{}", hub.term.capture(&s.name, 40, false).await.unwrap_or_default());
        assert_eq!(std::fs::read_to_string(dir.path().join("first.log")).unwrap().trim(), "", "Enter took the default; /login did not land in the picker");
        let mut st = state(&hub, "gemini").await.unwrap();
        for _ in 0..10 {
            if st.wants_code {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            st = state(&hub, "gemini").await.unwrap();
        }
        assert_eq!((st.wants_code, st.phase.as_str()), (true, "waiting_for_code"));
        send_code(&hub, "gemini", "4/abc-DEF").await.unwrap();
        for _ in 0..20 {
            if cred.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        assert_eq!(signed_in(&hub, hub.spec("gemini").unwrap()), Some(true));
        assert!(send_code(&hub, "gemini", "again").await.is_err(), "no code prompt any more: nothing typed");
        drop(tmux);
    }
}
