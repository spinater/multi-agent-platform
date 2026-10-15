//! The Configure screen's Env & secrets: variables every agent session, hook and verify/audit command of a project
//! gets. Two scopes: the machine (every project) and a project, whose variable replaces a machine one with the same
//! key (`override`).
//!
//! Plain values live in the database: the machine's in `runtime_settings` (`env`, a JSON object), a project's in its
//! settings (`ProjectSettings.env`). Secrets never do: this server is a headless box without an OS keychain, so they
//! live in files under the data folder, `<data_dir>/secrets/machine.json` and `<data_dir>/secrets/projects/<slug>.json`
//! (folders 0700, files 0600, written to a temporary file and renamed). A secret's value is never returned by the API,
//! logged, put in an event or an error message, or written into a launch script: sessions get secrets from a 0600
//! file next to the script that the script sources and deletes before it starts the agent (see [`launch_files`]).

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;

use crate::hub::Hub;
use crate::model::{ConfigSource, EnvList, EnvVar, EnvVarInput, Level};

/// The `runtime_settings` key of the machine's plain variables.
const MACHINE_SETTING: &str = "env";

static KEY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Z_][A-Z0-9_]*$").unwrap());

/// Variables MAP sets itself, or that would break the agent's shell or move a CLI off the folders MAP gives it.
const RESERVED: &[&str] = &[
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "PWD",
    "OLDPWD",
    "TERM",
    "IFS",
    "ENV",
    "BASH_ENV",
    "CDPATH",
    "TMUX",
    "TMUX_PANE",
    "TMUX_TMPDIR",
    "GIT_CONFIG_GLOBAL",
    "GIT_CONFIG_SYSTEM",
    "GIT_CONFIG_NOSYSTEM",
    "GIT_SSH",
    "GIT_SSH_COMMAND",
    "CLAUDE_CONFIG_DIR",
    "CODEX_HOME",
    "GEMINI_CLI_HOME",
    "COPILOT_HOME",
    "CURSOR_CONFIG_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
];

/// Prefixes MAP owns (`MAP_SESSION`, `MAP_RESULT_FILE`, `MAP_PROJECT_TOKEN`, …) or the dynamic loader reads.
const RESERVED_PREFIXES: &[&str] = &["MAP_", "LD_", "DYLD_"];

/// One variable of one scope as stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub value: String,
    pub secret: bool,
}

/// One scope's variables (key → value), plain and secret together; a key is one or the other.
type Scope = BTreeMap<String, Stored>;

/// Serializes the read-modify-write of `PUT`/`DELETE` so two saves never interleave.
static WRITE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Every variable name MAP refuses, with why: the fixed list, plus each provider's state-folder variable
/// (`home_env`), the folders kept beside it (`home_keep`) and its trust file's folder variable.
fn reserved(hub: &Hub) -> BTreeSet<String> {
    let mut keys: BTreeSet<String> = RESERVED.iter().map(|k| k.to_string()).collect();
    for spec in hub.config.providers.values() {
        keys.extend(spec.home_env.iter().filter(|k| !k.is_empty()).cloned());
        keys.extend(spec.home_keep.iter().filter_map(|k| k.split_once('=')).map(|(k, _)| k.to_string()));
        keys.extend(spec.trust.as_ref().and_then(|t| t.dir_env.clone()));
    }
    keys
}

/// A variable name the dashboard may set: `^[A-Z_][A-Z0-9_]*$`, not one MAP owns.
pub fn validate_key(hub: &Hub, key: &str) -> Result<()> {
    if key.len() > 128 || !KEY.is_match(key) {
        bail!("`{key}` is not a variable name: use capital letters, digits and _, not starting with a digit");
    }
    if let Some(p) = RESERVED_PREFIXES.iter().find(|p| key.starts_with(*p)) {
        bail!("`{key}` cannot be set here: MAP owns the {p}* variables");
    }
    if reserved(hub).contains(key) {
        bail!("`{key}` cannot be set here: MAP sets it for each session (or overriding it would break the agent's shell)");
    }
    Ok(())
}

fn validate_value(key: &str, value: &str) -> Result<()> {
    // Never the value itself in a message.
    if value.contains('\0') {
        bail!("`{key}`'s value contains a NUL byte");
    }
    if value.len() > 64 * 1024 {
        bail!("`{key}`'s value is longer than 64 KiB");
    }
    Ok(())
}

// ---- the secret store

fn secrets_dir(hub: &Hub) -> PathBuf {
    hub.config.server.data_dir.join("secrets")
}

/// `machine.json`, or `projects/<slug>.json` (slugs are checked before they get here).
pub fn secrets_file(hub: &Hub, project: Option<&str>) -> PathBuf {
    match project {
        None => secrets_dir(hub).join("machine.json"),
        Some(slug) => secrets_dir(hub).join("projects").join(format!("{slug}.json")),
    }
}

fn read_secrets(path: &Path) -> Result<BTreeMap<String, String>> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("the secret store {} is not valid JSON", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(anyhow::anyhow!("reading the secret store {}: {}", path.display(), e.kind())),
    }
}

/// Creates `dir` (and its parents) with mode 0700, and tightens it to 0700 if it exists.
fn private_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir).with_context(|| format!("creating {}", dir.display()))?;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    std::fs::create_dir_all(dir)?;
    Ok(())
}

/// Writes `bytes` to `path` with mode 0600: a temporary file in the same folder (created 0600), synced, renamed.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().context("no parent folder")?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("secret"), std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut f = opts.open(&tmp).with_context(|| format!("creating {}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path).with_context(|| format!("saving {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn write_secrets(hub: &Hub, project: Option<&str>, secrets: &BTreeMap<String, String>) -> Result<()> {
    let path = secrets_file(hub, project);
    if secrets.is_empty() {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(anyhow::anyhow!("removing {}: {}", path.display(), e.kind())),
            _ => Ok(()),
        };
    }
    private_dir(&secrets_dir(hub))?;
    private_dir(path.parent().context("no parent folder")?)?;
    write_private(&path, &serde_json::to_vec_pretty(secrets)?)
}

/// Forgets a deleted project's secrets.
pub fn forget_project(hub: &Hub, slug: &str) {
    let _ = std::fs::remove_file(secrets_file(hub, Some(slug)));
}

// ---- scopes

async fn plain(hub: &Hub, project: Option<&str>) -> Result<BTreeMap<String, String>> {
    match project {
        None => Ok(match hub.store.runtime_setting(MACHINE_SETTING).await? {
            Some(v) => serde_json::from_str(&v).context("the machine's env setting is not a JSON object")?,
            None => BTreeMap::new(),
        }),
        Some(slug) => Ok(hub.store.project(slug).await?.with_context(|| format!("unknown project `{slug}`"))?.settings.env),
    }
}

/// One scope's variables, plain and secret (a key in both counts as the secret).
async fn load(hub: &Hub, project: Option<&str>) -> Result<Scope> {
    let mut scope: Scope = plain(hub, project).await?.into_iter().map(|(k, value)| (k, Stored { value, secret: false })).collect();
    for (k, value) in read_secrets(&secrets_file(hub, project))? {
        scope.insert(k, Stored { value, secret: true });
    }
    Ok(scope)
}

async fn save(hub: &Hub, project: Option<&str>, scope: &Scope) -> Result<()> {
    let plain: BTreeMap<String, String> = scope.iter().filter(|(_, v)| !v.secret).map(|(k, v)| (k.clone(), v.value.clone())).collect();
    let secrets: BTreeMap<String, String> = scope.iter().filter(|(_, v)| v.secret).map(|(k, v)| (k.clone(), v.value.clone())).collect();
    // Secrets first: a failed write leaves the plain list as it was.
    write_secrets(hub, project, &secrets)?;
    match project {
        None => {
            let json = if plain.is_empty() { None } else { Some(serde_json::to_string(&plain)?) };
            hub.store.set_runtime_setting(MACHINE_SETTING, json.as_deref()).await?;
        }
        Some(slug) => {
            let mut p = hub.store.project(slug).await?.with_context(|| format!("unknown project `{slug}`"))?;
            p.settings.env = plain;
            hub.put_project(&p).await?;
        }
    }
    Ok(())
}

async fn check_project(hub: &Hub, project: Option<&str>) -> Result<()> {
    if let Some(slug) = project {
        crate::projects::validate_slug(slug)?;
        hub.store.project(slug).await?.with_context(|| format!("unknown project `{slug}`"))?;
    }
    Ok(())
}

/// The machine's variables, then the project's over them: (key, stored, source), sorted by key.
async fn merged(hub: &Hub, project: Option<&str>) -> Result<Vec<(String, Stored, ConfigSource)>> {
    let machine = load(hub, None).await?;
    let mut out: BTreeMap<String, (Stored, ConfigSource)> = machine.into_iter().map(|(k, v)| (k, (v, ConfigSource::Machine))).collect();
    if let Some(slug) = project {
        for (k, v) in load(hub, Some(slug)).await? {
            let source = if out.contains_key(&k) { ConfigSource::Override } else { ConfigSource::Project };
            out.insert(k, (v, source));
        }
    }
    Ok(out.into_iter().map(|(k, (v, s))| (k, v, s)).collect())
}

/// `GET /api/env`: never a secret's value.
pub async fn list(hub: &Hub, project: Option<&str>) -> Result<EnvList> {
    check_project(hub, project).await?;
    let vars = merged(hub, project)
        .await?
        .into_iter()
        .map(|(key, v, source)| EnvVar { set: !v.secret || !v.value.is_empty(), value: (!v.secret).then_some(v.value), secret: v.secret, key, source })
        .collect();
    Ok(EnvList {
        project: project.map(str::to_string),
        vars,
        values_at: if project.is_some() { "database: project settings".into() } else { "database: runtime_settings (env)".into() },
        secrets_at: secrets_file(hub, project).display().to_string(),
    })
}

/// `PUT /api/env`: replaces the scope's list. `value: null` keeps a secret's stored value (or makes a plain
/// variable's value secret); a secret made plain needs a new value; a variable left out is deleted.
pub async fn replace(hub: &Hub, project: Option<&str>, vars: Vec<EnvVarInput>) -> Result<EnvList> {
    check_project(hub, project).await?;
    let guard = WRITE.lock().await;
    let before = load(hub, project).await?;
    let mut after = Scope::new();
    for v in vars {
        let key = v.key.trim().to_string();
        validate_key(hub, &key)?;
        if after.contains_key(&key) {
            bail!("`{key}` is listed twice");
        }
        let old = before.get(&key);
        let value = match (v.value, v.secret, old) {
            (Some(value), _, _) => value,
            (None, true, Some(old)) => old.value.clone(),
            (None, true, None) => bail!("`{key}` is a new secret: send its value"),
            (None, false, Some(old)) if !old.secret => old.value.clone(),
            (None, false, Some(_)) => bail!("`{key}` was a secret: making it plain needs a new value"),
            (None, false, None) => bail!("`{key}` needs a value"),
        };
        validate_value(&key, &value)?;
        if v.secret && value.is_empty() {
            bail!("`{key}` is a secret with an empty value");
        }
        after.insert(key, Stored { value, secret: v.secret });
    }
    save(hub, project, &after).await?;
    announce(hub, project, &after).await;
    drop(guard);
    list(hub, project).await
}

/// `DELETE /api/env`: removes one variable from the scope; an error when the scope has none by that name.
pub async fn remove(hub: &Hub, project: Option<&str>, key: &str) -> Result<Option<EnvList>> {
    check_project(hub, project).await?;
    let guard = WRITE.lock().await;
    let mut scope = load(hub, project).await?;
    if scope.remove(key).is_none() {
        return Ok(None);
    }
    save(hub, project, &scope).await?;
    announce(hub, project, &scope).await;
    drop(guard);
    list(hub, project).await.map(Some)
}

/// An event naming the scope and its keys (never a value).
async fn announce(hub: &Hub, project: Option<&str>, scope: &Scope) {
    let secrets = scope.values().filter(|v| v.secret).count();
    let keys: Vec<&str> = scope.keys().map(String::as_str).collect();
    let what = match project {
        Some(slug) => format!("Project {slug}'s env"),
        None => "The machine's env".to_string(),
    };
    let list = if keys.is_empty() { "none".to_string() } else { keys.join(", ") };
    hub.event(Level::Info, "env.updated", None, None, format!("{what}: {} variable(s), {secrets} secret ({list})", keys.len())).await;
}

// ---- for the modules that start things

/// One merged variable for a run: its key, value and whether it is a secret.
#[derive(Debug, Clone)]
pub struct RunVar {
    pub key: String,
    pub value: String,
    pub secret: bool,
}

/// The merged variables (machine, then the project's over them) with whether each is a secret. A store that cannot
/// be read gives nothing from that scope (and a warning without any value).
pub async fn run_vars(hub: &Hub, project: Option<&str>) -> Vec<RunVar> {
    match merged(hub, project).await {
        Ok(vars) => vars.into_iter().map(|(key, v, _)| RunVar { key, value: v.value, secret: v.secret }).collect(),
        Err(err) => {
            tracing::warn!("env variables for {}: {err:#}", project.unwrap_or("the machine"));
            vec![]
        }
    }
}

/// The variables every session, hook and verify/audit command of `project` gets (the machine's alone without one),
/// the machine's first and the project's over them, in key order. Values include secrets: pass them to a child's
/// environment only, never to a file, a log or an event.
pub async fn vars_for(hub: &Hub, project: Option<&str>) -> Vec<(String, String)> {
    run_vars(hub, project).await.into_iter().map(|v| (v.key, v.value)).collect()
}

/// The secret values among `vars` (for [`redact`]); short values are left out, they would match too much.
pub fn secret_values(vars: &[RunVar]) -> Vec<String> {
    vars.iter().filter(|v| v.secret && v.value.chars().count() >= 4).map(|v| v.value.clone()).collect()
}

/// `text` with every secret value replaced by `***` (command output that lands in events or task results).
pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets {
        if out.contains(s.as_str()) {
            out = out.replace(s.as_str(), "***");
        }
    }
    out
}

/// What a launch script needs: the plain variables' `export` lines (they may stay on disk with the script) and, when
/// there are secrets, the lines that source the 0600 file `secrets_env` (written here) and delete it at once. The
/// secrets' values are only ever in that file, which lives until the script starts.
pub fn launch_files(vars: &[RunVar], secrets_env: &Path) -> Result<Vec<String>> {
    let mut lines = vec![];
    let mut secret_lines = String::new();
    for v in vars {
        validate_value(&v.key, &v.value)?;
        let quoted = shlex::try_quote(&v.value).map_err(|_| anyhow::anyhow!("`{}`'s value cannot be quoted", v.key))?;
        let line = format!("export {}={quoted}\n", v.key);
        if v.secret {
            secret_lines.push_str(&line);
        } else {
            lines.push(line.trim_end().to_string());
        }
    }
    if !secret_lines.is_empty() {
        write_private(secrets_env, secret_lines.as_bytes())?;
        let path = crate::hub::quote(secrets_env.to_str().context("non-UTF-8 path")?)?;
        lines.push(format!(". {path}"));
        lines.push(format!("rm -f {path}"));
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use crate::config::Config;
    use crate::hub::LaunchRequest;
    use crate::model::Role;
    use crate::store::Store;
    use crate::terminal::fake::FakeTerminal;

    const SECRET: &str = "s3cr3t-value-never-shown";
    const MACHINE_SECRET: &str = "machine-token-0xdeadbeef";

    /// `claude` is installed (`/bin/sh`); isolated agent homes in the temp dir.
    async fn hub() -> (Arc<Hub>, Arc<FakeTerminal>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let text = format!(
            "[projects]\npath = \"{0}\"\n[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\napi_token = \"secret\"\n[agents_home]\nmode = \"isolated\"\ndir = \"{0}/agent-home\"\n[orchestrator]\nprovider_order = [\"claude\"]\n[reflex]\nbinary = \"/nonexistent/reflex\"\n[providers.claude]\nbinary = \"/bin/sh\"\ncredentials = []\n[templates]\ndirs = []\n",
            dir.path().display()
        );
        let config = Config::parse(&text, Path::new("/")).unwrap();
        assert!(config.agents_home.isolated());
        let term = Arc::new(FakeTerminal::default());
        (Hub::new(config, Store::memory().await.unwrap(), term.clone()), term, dir)
    }

    fn var(key: &str, value: Option<&str>, secret: bool) -> EnvVarInput {
        EnvVarInput { key: key.into(), value: value.map(str::to_string), secret }
    }

    fn mode(path: &Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn find<'a>(list: &'a EnvList, key: &str) -> &'a EnvVar {
        list.vars.iter().find(|v| v.key == key).unwrap_or_else(|| panic!("no {key} in {list:?}"))
    }

    async fn all_events(hub: &Hub) -> String {
        serde_json::to_string(&hub.store.events(1000, None).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn keys_are_validated_and_reserved_ones_refused() {
        let (hub, _, _dir) = hub().await;
        for bad in ["", "lower", "1ABC", "A-B", "A B", "ÄB"] {
            let err = replace(&hub, None, vec![var(bad, Some("x"), false)]).await.unwrap_err().to_string();
            assert!(err.contains("not a variable name"), "{bad}: {err}");
        }
        for reserved in ["PATH", "HOME", "TMUX", "MAP_SESSION", "MAP_PROJECT_TOKEN", "CLAUDE_CONFIG_DIR", "CODEX_HOME", "GIT_CONFIG_GLOBAL", "LD_PRELOAD"] {
            let err = replace(&hub, None, vec![var(reserved, Some("x"), false)]).await.unwrap_err().to_string();
            assert!(err.contains("cannot be set here"), "{reserved}: {err}");
        }
        // Derived from the providers: gemini's `home_keep` (CARGO_HOME) and copilot's home.
        assert!(validate_key(&hub, "CARGO_HOME").is_err());
        assert!(validate_key(&hub, "COPILOT_HOME").is_err());
        assert!(validate_key(&hub, "DATABASE_URL").is_ok() && validate_key(&hub, "_X1").is_ok());
        let err = replace(&hub, None, vec![var("A", Some("1"), false), var("A", Some("2"), false)]).await.unwrap_err().to_string();
        assert!(err.contains("listed twice"), "{err}");
        // A bad value's error never repeats the value.
        let err = replace(&hub, None, vec![var("NUL", Some("ab\0cd-secretish"), true)]).await.unwrap_err().to_string();
        assert!(err.contains("NUL") && !err.contains("secretish"), "{err}");
        // Nothing was saved by the failed calls; a project must exist.
        assert!(list(&hub, None).await.unwrap().vars.is_empty());
        assert!(list(&hub, Some("nope")).await.is_err());
        assert!(list(&hub, Some("../etc")).await.is_err());
        // Project settings sent through PATCH are checked the same way.
        let folder = hub.config.workspace.dir.join("app");
        let slug = crate::plans::tests::project(&hub, &folder, "App").await;
        let mut s = hub.store.project(&slug).await.unwrap().unwrap().settings;
        assert!(crate::project_settings::validate(&hub, &s).is_ok());
        s.env.insert("PATH".into(), "/x".into());
        let err = crate::project_settings::validate(&hub, &s).unwrap_err().to_string();
        assert!(err.contains("`PATH` cannot be set here"), "{err}");
    }

    #[tokio::test]
    async fn project_overrides_machine_and_secrets_never_come_back() {
        let (hub, _, dir) = hub().await;
        let slug = crate::plans::tests::project(&hub, &dir.path().join("app"), "App").await;
        replace(&hub, None, vec![var("SHARED", Some("machine"), false), var("GITHUB_TOKEN", Some(MACHINE_SECRET), true), var("ONLY_MACHINE", Some("m"), false)]).await.unwrap();
        let got = replace(&hub, Some(&slug), vec![var("SHARED", Some("project"), false), var("DATABASE_URL", Some(SECRET), true)]).await.unwrap();

        let json = serde_json::to_string(&got).unwrap();
        assert!(!json.contains(SECRET) && !json.contains(MACHINE_SECRET), "{json}");
        assert_eq!(find(&got, "SHARED"), &EnvVar { key: "SHARED".into(), value: Some("project".into()), secret: false, set: true, source: ConfigSource::Override });
        assert_eq!(find(&got, "DATABASE_URL"), &EnvVar { key: "DATABASE_URL".into(), value: None, secret: true, set: true, source: ConfigSource::Project });
        assert_eq!(find(&got, "GITHUB_TOKEN").source, ConfigSource::Machine);
        assert_eq!(find(&got, "GITHUB_TOKEN").value, None);
        assert_eq!(find(&got, "ONLY_MACHINE").value.as_deref(), Some("m"));

        // Without a project: the machine's alone.
        let machine = list(&hub, None).await.unwrap();
        assert_eq!(machine.vars.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(), ["GITHUB_TOKEN", "ONLY_MACHINE", "SHARED"]);
        assert_eq!(find(&machine, "SHARED").value.as_deref(), Some("machine"));
        assert!(!serde_json::to_string(&machine).unwrap().contains(MACHINE_SECRET));

        // The merged values for runs: machine first, the project's over it.
        let vars: BTreeMap<String, String> = vars_for(&hub, Some(&slug)).await.into_iter().collect();
        assert_eq!(vars["SHARED"], "project");
        assert_eq!(vars["DATABASE_URL"], SECRET);
        assert_eq!(vars["GITHUB_TOKEN"], MACHINE_SECRET);
        assert_eq!(vars_for(&hub, None).await.into_iter().find(|(k, _)| k == "SHARED").unwrap().1, "machine");

        // The project's info (settings) carries the plain values only; the database never holds a secret.
        let info = serde_json::to_string(&hub.projects().await.unwrap()).unwrap();
        assert!(info.contains("\"SHARED\":\"project\"") && !info.contains(SECRET), "{info}");
        let setting = hub.store.runtime_setting("env").await.unwrap().unwrap();
        assert!(!setting.contains(MACHINE_SECRET) && setting.contains("ONLY_MACHINE"));
        let events = all_events(&hub).await;
        assert!(events.contains("env.updated") && !events.contains(SECRET) && !events.contains(MACHINE_SECRET), "{events}");

        // Secret files: folders 0700, files 0600, outside the database.
        let data = hub.config.server.data_dir.clone();
        assert_eq!(mode(&data.join("secrets")), 0o700);
        assert_eq!(mode(&data.join("secrets/projects")), 0o700);
        assert_eq!(mode(&data.join("secrets/machine.json")), 0o600);
        assert_eq!(mode(&secrets_file(&hub, Some(&slug))), 0o600);
        assert!(std::fs::read_to_string(secrets_file(&hub, Some(&slug))).unwrap().contains(SECRET));
        assert!(std::fs::read_dir(data.join("secrets")).unwrap().all(|e| !e.unwrap().file_name().to_string_lossy().ends_with(".tmp")));

        // Deleting one variable; an unknown one is reported as missing.
        let after = remove(&hub, Some(&slug), "SHARED").await.unwrap().unwrap();
        assert_eq!(find(&after, "SHARED").source, ConfigSource::Machine);
        assert!(remove(&hub, Some(&slug), "SHARED").await.unwrap().is_none());

        // Deleting the project forgets its secrets.
        crate::projects::delete(&hub, &slug).await.unwrap();
        assert!(!secrets_file(&hub, Some(&slug)).exists());
    }

    #[tokio::test]
    async fn null_keeps_a_secret_and_switching_needs_a_value() {
        let (hub, _, _dir) = hub().await;
        replace(&hub, None, vec![var("TOKEN", Some(SECRET), true), var("PLAIN", Some("p"), false)]).await.unwrap();
        // `null` keeps the stored secret.
        replace(&hub, None, vec![var("TOKEN", None, true), var("PLAIN", None, false)]).await.unwrap();
        let v: BTreeMap<_, _> = vars_for(&hub, None).await.into_iter().collect();
        assert_eq!((v["TOKEN"].as_str(), v["PLAIN"].as_str()), (SECRET, "p"));
        // A string replaces it.
        replace(&hub, None, vec![var("TOKEN", Some("new-secret-value"), true), var("PLAIN", None, false)]).await.unwrap();
        assert_eq!(vars_for(&hub, None).await[1], ("TOKEN".to_string(), "new-secret-value".to_string()));
        // Secret → plain needs a value; with one it moves out of the secret store.
        let err = replace(&hub, None, vec![var("TOKEN", None, false)]).await.unwrap_err().to_string();
        assert!(err.contains("making it plain needs a new value") && !err.contains("new-secret-value"), "{err}");
        let got = replace(&hub, None, vec![var("TOKEN", Some("visible"), false), var("PLAIN", None, false)]).await.unwrap();
        assert_eq!(find(&got, "TOKEN").value.as_deref(), Some("visible"));
        assert!(!secrets_file(&hub, None).exists(), "no secrets left: the file goes");
        // Plain → secret with `null` keeps the value, now as a secret.
        let got = replace(&hub, None, vec![var("TOKEN", Some("visible"), false), var("PLAIN", None, true)]).await.unwrap();
        assert_eq!((find(&got, "PLAIN").secret, find(&got, "PLAIN").value.clone()), (true, None));
        assert!(!hub.store.runtime_setting("env").await.unwrap().unwrap().contains("PLAIN"));
        assert_eq!(read_secrets(&secrets_file(&hub, None)).unwrap()["PLAIN"], "p");
        // A new secret without a value, or an empty one, is refused; omitting a key deletes it.
        assert!(replace(&hub, None, vec![var("FRESH", None, true)]).await.unwrap_err().to_string().contains("new secret"));
        assert!(replace(&hub, None, vec![var("FRESH", Some(""), true)]).await.is_err());
        assert!(replace(&hub, None, vec![]).await.unwrap().vars.is_empty());
        assert!(hub.store.runtime_setting("env").await.unwrap().is_none() && !secrets_file(&hub, None).exists());
    }

    #[tokio::test]
    async fn sessions_get_the_vars_without_secrets_in_the_script_or_events() {
        let (hub, term, dir) = hub().await;
        let slug = crate::plans::tests::project(&hub, &dir.path().join("app"), "App").await;
        replace(&hub, None, vec![var("GITHUB_TOKEN", Some(MACHINE_SECRET), true), var("SHARED", Some("machine"), false)]).await.unwrap();
        replace(&hub, Some(&slug), vec![var("SHARED", Some("it's $HOME `x` project"), false), var("DATABASE_URL", Some(SECRET), true)]).await.unwrap();

        let req = LaunchRequest { name: Some("envtest".into()), provider: "claude".into(), model: None, role: Role::Manual, task_id: None, project: Some(slug.clone()), prompt: Some("hi".into()) };
        let session = hub.launch(req).await.unwrap();
        let run = hub.run_dir(&session.name);
        let script = std::fs::read_to_string(run.join("launch.sh")).unwrap();
        assert_eq!(term.sessions.lock().unwrap()[&session.name].script, script);
        // Plain values in the script, before MAP's own variables and the exec; no secret value.
        assert!(script.contains("export SHARED="), "{script}");
        assert!(!script.contains(SECRET) && !script.contains(MACHINE_SECRET), "{script}");
        let secrets = run.join(crate::hub::SECRETS_ENV);
        let quoted = crate::hub::quote(secrets.to_str().unwrap()).unwrap();
        let source = script.find(&format!(". {quoted}\nrm -f {quoted}\n")).expect("sources and removes the secrets file");
        assert!(source < script.find("export MAP_SESSION=").unwrap() && source < script.find("exec ").unwrap());
        // The secrets file: 0600, only the secrets.
        assert_eq!(mode(&secrets), 0o600);
        let body = std::fs::read_to_string(&secrets).unwrap();
        assert!(body.contains(&format!("export DATABASE_URL={SECRET}\n")) && body.contains(&format!("export GITHUB_TOKEN={MACHINE_SECRET}\n")) && !body.contains("SHARED"), "{body}");

        // Running the script (its `exec` replaced by printing the environment) gets every value and deletes the file.
        let runnable = format!("{}env\n", &script[..script.rfind("exec ").unwrap()]);
        let probe = run.join("probe.sh");
        std::fs::write(&probe, runnable).unwrap();
        let out = std::process::Command::new("sh").arg(&probe).output().unwrap();
        let env = String::from_utf8_lossy(&out.stdout);
        assert!(env.contains(&format!("DATABASE_URL={SECRET}")) && env.contains(&format!("GITHUB_TOKEN={MACHINE_SECRET}")) && env.contains("SHARED=it's $HOME `x` project\n"), "{env}");
        assert!(!secrets.exists(), "the script deletes the secrets file");

        // Nothing MAP recorded about the launch holds a secret.
        let events = all_events(&hub).await;
        assert!(events.contains("session.started") && !events.contains(SECRET) && !events.contains(MACHINE_SECRET), "{events}");
        let stored = serde_json::to_string(&hub.store.session(&session.name).await.unwrap()).unwrap();
        assert!(!stored.contains(SECRET) && !stored.contains(MACHINE_SECRET));

        // A session whose script never ran leaves no secrets file once closed.
        let req = LaunchRequest { name: Some("never".into()), provider: "claude".into(), model: None, role: Role::Manual, task_id: None, project: Some(slug.clone()), prompt: None };
        let s2 = hub.launch(req).await.unwrap();
        assert!(hub.run_dir(&s2.name).join(crate::hub::SECRETS_ENV).exists());
        hub.close_session(&s2.name, true, "test").await.unwrap();
        assert!(!hub.run_dir(&s2.name).join(crate::hub::SECRETS_ENV).exists());
    }

    #[tokio::test]
    async fn hooks_and_verify_commands_get_the_vars_with_secrets_masked_in_output() {
        let (hub, _, dir) = hub().await;
        let slug = crate::plans::tests::project(&hub, &dir.path().join("app"), "App").await;
        replace(&hub, None, vec![var("SHARED", Some("machine"), false)]).await.unwrap();
        replace(&hub, Some(&slug), vec![var("DATABASE_URL", Some(SECRET), true)]).await.unwrap();
        let cmd = format!("echo \"shared=$SHARED db=$DATABASE_URL\"; test \"$DATABASE_URL\" = '{SECRET}'");
        let (ok, out) = crate::orchestrator::shell_for(&hub, Some(&slug), dir.path(), &cmd, std::time::Duration::from_secs(10)).await.unwrap();
        assert!(ok, "the command saw the secret's value: {out}");
        assert!(out.contains("shared=machine db=***") && !out.contains(SECRET), "{out}");
        // The machine scope alone without a project.
        let (_, out) = crate::orchestrator::shell_for(&hub, None, dir.path(), "echo \"[$SHARED][${DATABASE_URL:-}]\"", std::time::Duration::from_secs(10)).await.unwrap();
        assert_eq!(out.trim(), "[machine][]");
    }

    #[test]
    fn redact_replaces_secret_values() {
        let vars = vec![
            RunVar { key: "A".into(), value: "abcd1234".into(), secret: true },
            RunVar { key: "B".into(), value: "x".into(), secret: true },
            RunVar { key: "C".into(), value: "plainval".into(), secret: false },
        ];
        let s = secret_values(&vars);
        assert_eq!(s, ["abcd1234"]);
        assert_eq!(redact("token abcd1234 and plainval", &s), "token *** and plainval");
    }
}
