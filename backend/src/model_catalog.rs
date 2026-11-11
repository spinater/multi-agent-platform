//! Each provider's model list as its own CLI reports it, so a model the CLI gains can be picked without a change to
//! MAP: Codex's cached list (`models_cache.json` in its state folder), `agy models`, `cursor-agent --list-models`.
//! Read at start and every hour; until a read lists something, the list from `map.toml` (or built in) stays, and a
//! failed read keeps the last good one. `[providers.<name>] models_from` names another source; a `models` list set in
//! `map.toml` turns the reading off for that provider unless `models_from` is set too. Claude Code has no such list:
//! its aliases (`opus`, `sonnet`, …) already follow the newest model.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, LazyLock, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::Deserialize;

use crate::config::{Config, ProviderSpec};
use crate::hub::Hub;

const EVERY: Duration = Duration::from_secs(3600);
const TIMEOUT: Duration = Duration::from_secs(30);
const MAX_FILE: u64 = 8 << 20;
const MAX_MODELS: usize = 100;

static ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9._:/~@+-]{0,79}$").unwrap());

/// Where a provider's CLI lists its models (`[providers.<name>] models_from`). Neither `command` nor `file`: off.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelsFrom {
    /// A command that prints one model id per line (`{binary}` substituted), run like the provider's other commands.
    pub command: Vec<String>,
    /// Or a JSON file the CLI keeps: `{home}` is its state folder, `~/` the home directory.
    pub file: Option<String>,
    /// The JSON pointer of the model array in `file` (default `/models`).
    pub list: Option<String>,
    /// Each model's id field (default `slug`).
    pub id: Option<String>,
    /// Skips a model whose field has this value (Codex: `visibility = "hide"`).
    pub skip: BTreeMap<String, String>,
    /// `default_model` was set in `map.toml`: it stays the default even when the CLI does not list it.
    #[serde(skip)]
    pub keep_default: bool,
}

impl ModelsFrom {
    fn off(&self) -> bool {
        self.command.is_empty() && self.file.is_none()
    }
}

/// The source MAP knows for a CLI, by its binary's file name.
fn builtin(binary: &str) -> Option<ModelsFrom> {
    let strings = |a: &[&str]| a.iter().map(|s| s.to_string()).collect();
    match std::path::Path::new(binary).file_name()?.to_str()? {
        "codex" => Some(ModelsFrom {
            file: Some("{home}/models_cache.json".into()),
            skip: BTreeMap::from([("visibility".into(), "hide".into())]),
            ..Default::default()
        }),
        "agy" => Some(ModelsFrom { command: strings(&["{binary}", "models"]), ..Default::default() }),
        "cursor-agent" => Some(ModelsFrom { command: strings(&["{binary}", "--list-models"]), ..Default::default() }),
        _ => None,
    }
}

/// A provider's `models_from` from its `map.toml` table: `set` itself, off when `models` is set, else the built-in
/// source (`None` here means that too, resolved by [`source_of`]).
pub(crate) fn source(set: Option<ModelsFrom>, models_set: bool, default_set: bool, binary: &str) -> Option<ModelsFrom> {
    let mut from = match set {
        Some(s) => s,
        None if models_set => ModelsFrom::default(),
        None if default_set => builtin(binary)?,
        None => return None,
    };
    from.keep_default = default_set;
    Some(from)
}

fn source_of(spec: &ProviderSpec) -> Option<ModelsFrom> {
    spec.models_from.clone().or_else(|| builtin(&spec.binary)).filter(|f| !f.off())
}

/// The provider specs with the models their CLIs listed; [`Hub::spec`] reads them.
#[derive(Default)]
pub struct Catalog(RwLock<HashMap<String, Arc<ProviderSpec>>>);

impl Catalog {
    pub fn spec(&self, config: &Config, name: &str) -> Option<Arc<ProviderSpec>> {
        if let Some(s) = self.0.read().unwrap().get(name) {
            return Some(s.clone());
        }
        let spec = Arc::new(config.providers.get(name)?.clone());
        Some(self.0.write().unwrap().entry(name.to_string()).or_insert(spec).clone())
    }

    fn put(&self, name: &str, spec: ProviderSpec) {
        self.0.write().unwrap().insert(name.to_string(), Arc::new(spec));
    }
}

/// `spec` with the models a CLI listed: their order, then the default. The default stays when listed (or set in
/// `map.toml`), else it is the first listed.
fn with_models(spec: &ProviderSpec, from: &ModelsFrom, found: Vec<String>) -> ProviderSpec {
    let mut models: Vec<String> = Vec::new();
    for m in found {
        if !models.contains(&m) && models.len() < MAX_MODELS {
            models.push(m);
        }
    }
    let keep = from.keep_default || models.contains(&spec.default_model);
    let default_model = if keep || models.is_empty() { spec.default_model.clone() } else { models[0].clone() };
    if keep && !default_model.is_empty() && !models.contains(&default_model) {
        models.push(default_model.clone());
    }
    ProviderSpec { models, default_model, ..spec.clone() }
}

/// Model ids in a command's output, one per line: the first word, lowercase, after any bullet. A word with no digit,
/// `-`, `/` or `:` only counts alone on its line or before a ` - ` or `(` (`auto - Auto`), so prose does not.
pub(crate) fn parse_lines(text: &str) -> Vec<String> {
    let text = crate::detect::strip_ansi(text);
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_start_matches(['-', '*', '•', '>', '›']).trim();
        let mut words = line.split_whitespace();
        let Some(word) = words.next().map(|w| w.trim_end_matches(',')) else { continue };
        if !ID.is_match(word) {
            continue;
        }
        let rest = words.next();
        let plain = !word.contains(|c: char| c.is_ascii_digit() || matches!(c, '-' | '/' | ':'));
        if plain && !(rest.is_none() || rest == Some("-") || rest.is_some_and(|r| r.starts_with('('))) {
            continue;
        }
        if !out.iter().any(|m| m == word) {
            out.push(word.to_string());
        }
    }
    out
}

/// Model ids in a CLI's JSON file: the array at `from.list`, each entry's `from.id`, skipping `from.skip` matches.
pub(crate) fn parse_file(text: &str, from: &ModelsFrom) -> Result<Vec<String>> {
    let json: serde_json::Value = serde_json::from_str(text).context("not JSON")?;
    let pointer = from.list.as_deref().unwrap_or("/models");
    let Some(list) = json.pointer(pointer).and_then(|v| v.as_array()) else { bail!("no array at `{pointer}`") };
    let id = from.id.as_deref().unwrap_or("slug");
    let skipped = |m: &serde_json::Value| from.skip.iter().any(|(k, v)| m.get(k).and_then(|x| x.as_str()) == Some(v.as_str()));
    Ok(list.iter().filter(|m| !skipped(m)).filter_map(|m| m.get(id)?.as_str()).filter(|s| ID.is_match(s)).map(String::from).collect())
}

fn file_path(hub: &Hub, name: &str, file: &str) -> Result<PathBuf> {
    if let Some(rest) = file.strip_prefix("{home}/") {
        return Ok(hub.provider_home(name).context("the provider has no state folder")?.join(rest));
    }
    Ok(crate::config::expand_home(std::path::Path::new(file)))
}

/// What the CLI lists now (empty when it lists nothing it recognises).
async fn list(hub: &Hub, name: &str, spec: &ProviderSpec, from: &ModelsFrom) -> Result<Vec<String>> {
    if let Some(file) = &from.file {
        let path = file_path(hub, name, file)?;
        let size = std::fs::metadata(&path).with_context(|| format!("{}", path.display()))?.len();
        if size > MAX_FILE {
            bail!("{} is larger than {MAX_FILE} bytes", path.display());
        }
        return parse_file(&std::fs::read_to_string(&path)?, from).with_context(|| format!("{}", path.display()));
    }
    let binary = hub.binary_path(&spec.binary).context("not on PATH")?;
    let argv: Vec<String> = from.command.iter().map(|a| a.replace("{binary}", &binary.to_string_lossy())).collect();
    let mut cmd = tokio::process::Command::new(&argv[0]);
    cmd.args(&argv[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
    let workspace: Vec<(String, String)> = hub.config.workspace.env.clone().into_iter().collect();
    cmd.envs(hub.run_env(name, workspace)).env("NO_COLOR", "1");
    if hub.config.workspace.dir.is_dir() {
        cmd.current_dir(&hub.config.workspace.dir);
    }
    let out = tokio::time::timeout(TIMEOUT, cmd.output()).await.context("timed out")??;
    if !out.status.success() {
        bail!("`{}` failed ({})", from.command.join(" "), out.status);
    }
    Ok(parse_lines(&String::from_utf8_lossy(&out.stdout)))
}

/// Reads every provider's list once; returns whether one changed.
pub async fn refresh(hub: &Hub) -> bool {
    let mut changed = false;
    for (name, spec) in &hub.config.providers {
        let Some(from) = source_of(spec) else { continue };
        if !hub.provider_available(name) {
            continue;
        }
        let found = match list(hub, name, spec, &from).await {
            Ok(found) if !found.is_empty() => found,
            Ok(_) => continue,
            Err(err) => {
                tracing::debug!("{name}: no model list from the CLI: {err:#}");
                continue;
            }
        };
        let next = with_models(spec, &from, found);
        let Some(now) = hub.models.spec(&hub.config, name) else { continue };
        if now.models != next.models || now.default_model != next.default_model {
            tracing::info!("{name}: models from the CLI: {} (default {})", next.models.join(", "), next.default_model);
            hub.models.put(name, next);
            changed = true;
        }
    }
    changed
}

pub async fn run(hub: Arc<Hub>) {
    loop {
        if refresh(&hub).await {
            hub.announce_providers_if_changed();
        }
        tokio::time::sleep(EVERY).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_keep_ids_and_drop_prose() {
        let out = "Fetching available models...\nAvailable models:\n\n  * gemini-4-argon-high (current)\n  - gemini-3.8-flash\n\x1b[1mauto - Auto\x1b[0m\nloading the list now\nError: Please sign in\ngpt-6-luna, fast\ngemini-3.8-flash\n";
        assert_eq!(parse_lines(out), ["gemini-4-argon-high", "gemini-3.8-flash", "auto", "gpt-6-luna"]);
        assert!(parse_lines("Error: Please sign in to view available models. Launch the CLI without arguments to sign in.\n").is_empty());
    }

    #[test]
    fn codex_cache_lists_visible_slugs() {
        let from = builtin("/usr/local/bin/codex").unwrap();
        let json = r#"{"fetched_at":"x","models":[{"slug":"gpt-reserve","visibility":"hide"},{"slug":"gpt-6-luna","visibility":"list"},{"slug":"gpt-5.6-terra","visibility":"list"},{"display_name":"no slug"}]}"#;
        assert_eq!(parse_file(json, &from).unwrap(), ["gpt-6-luna", "gpt-5.6-terra"]);
        assert!(parse_file("{}", &from).is_err());
        assert!(parse_file("nope", &from).is_err());
    }

    #[test]
    fn sources_follow_map_toml() {
        assert!(builtin("claude").is_none());
        assert_eq!(source(None, false, false, "agy"), None);
        assert!(source(None, true, false, "agy").unwrap().off(), "a models list in map.toml pins it");
        let kept = source(None, false, true, "agy").unwrap();
        assert!(kept.keep_default && !kept.off());
        let own = ModelsFrom { command: vec!["x".into()], ..Default::default() };
        assert_eq!(source(Some(own.clone()), true, false, "agy"), Some(own));
        let spec = ProviderSpec { binary: "agy".into(), ..Default::default() };
        assert!(source_of(&spec).is_some());
        assert!(source_of(&ProviderSpec { models_from: Some(ModelsFrom::default()), ..spec }).is_none());
    }

    #[test]
    fn listed_models_replace_the_list_and_maybe_the_default() {
        let spec = ProviderSpec { models: vec!["old".into()], default_model: "old".into(), ..Default::default() };
        let from = ModelsFrom::default();
        let next = with_models(&spec, &from, vec!["b".into(), "a".into(), "b".into()]);
        assert_eq!((next.models, next.default_model.as_str()), (vec!["b".to_string(), "a".into()], "b"));
        let next = with_models(&spec, &from, vec!["a".into(), "old".into()]);
        assert_eq!(next.default_model, "old");
        let next = with_models(&spec, &ModelsFrom { keep_default: true, ..from }, vec!["a".into()]);
        assert_eq!((next.models, next.default_model.as_str()), (vec!["a".to_string(), "old".into()], "old"));
    }

    #[tokio::test]
    async fn refresh_reads_the_cli_and_hub_spec_follows() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("lister");
        std::fs::write(&bin, "#!/bin/sh\necho 'Available models:'\necho 'new-model-2'\necho 'gemini-3.8-flash-high'\n").unwrap();
        std::fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        let text = format!(
            "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n[providers.gemini]\nbinary = \"{1}\"\nmodels_from = {{ command = [\"{{binary}}\"] }}\n",
            dir.path().display(),
            bin.display()
        );
        let config = Config::parse(&text, dir.path()).unwrap();
        let hub = Hub::new(config, crate::store::Store::memory().await.unwrap(), Arc::new(crate::terminal::fake::FakeTerminal::default()));
        assert!(!hub.spec("gemini").unwrap().models.contains(&"new-model-2".to_string()));
        assert!(refresh(&hub).await);
        let spec = hub.spec("gemini").unwrap();
        assert_eq!((spec.models.clone(), spec.default_model.as_str()), (vec!["new-model-2".to_string(), "gemini-3.8-flash-high".into()], "gemini-3.8-flash-high"));
        assert!(hub.providers().iter().any(|p| p.name == "gemini" && p.models.contains(&"new-model-2".to_string())));
        assert!(Hub::resolve_model(&spec, Some("new-model-2")).is_ok());
        assert!(!refresh(&hub).await, "same list: no change");
    }
}
