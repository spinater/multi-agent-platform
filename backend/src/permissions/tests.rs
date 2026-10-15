use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Value, json};

use super::*;
use crate::hub::Hub;
use crate::model::{ConfigScope, ConfigSource, PermChange, PermValue, PermissionsView};

/// A hub in isolated mode (every CLI home under a temp agent home) and a project folder.
pub(crate) struct Setup {
    pub hub: Arc<Hub>,
    pub _dir: tempfile::TempDir,
    pub ah: tempfile::TempDir,
    pub slug: String,
    pub project: PathBuf,
}

pub(crate) async fn setup() -> Setup {
    let ah = tempfile::tempdir().unwrap();
    let extra = format!("[agents_home]\nmode = \"isolated\"\ndir = \"{}\"\n", ah.path().display());
    let (hub, _, dir) = crate::plans::tests::hub(&extra).await;
    let folder = dir.path().join("app");
    let slug = crate::plans::tests::project(&hub, &folder, "App").await;
    let project = hub.work_dir(Some(&slug)).await;
    assert_eq!(project, folder);
    // Never the owner's real folders.
    for p in ["claude", "codex", "gemini"] {
        assert!(hub.config.provider_home(p).unwrap().starts_with(ah.path()), "{p}");
    }
    Setup { hub, _dir: dir, ah, slug, project }
}

pub(crate) fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

pub(crate) fn json_of(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn change(rule: &str, provider: &str, value: Option<PermValue>) -> PermChange {
    PermChange { rule: rule.into(), provider: provider.into(), value }
}

fn cell<'a>(v: &'a PermissionsView, rule: &str, provider: &str) -> &'a crate::model::PermCell {
    let row = v.rows.iter().find(|r| r.rule == rule).unwrap_or_else(|| panic!("no row {rule}"));
    row.cells.iter().find(|c| c.provider == provider).unwrap()
}

#[tokio::test]
async fn claude_cells_write_rule_lists_and_keep_everything_else() {
    let s = setup().await;
    let c = &s.hub.config;
    let machine = s.ah.path().join("claude/settings.json");
    write(&machine, r#"{"model": "opus", "permissions": {"allow": ["Bash(git:*)", "Bash(cargo test)"], "defaultMode": "plan"}, "hooks": {"Stop": []}}"#);
    let local = s.project.join(".claude/settings.local.json");
    write(&local, r#"{"skillOverrides": {"verify": "off"}, "permissions": {"deny": ["Skill(commit)"]}}"#);

    // Machine scope: only the machine file; the legacy `Bash(git:*)` reads as the git row.
    let v = view(c, ConfigScope::Machine, &s.project, &[]);
    let git = cell(&v, "Bash(git *)", "claude");
    assert_eq!((git.value, git.source), (PermValue::Allow, Some(ConfigSource::Machine)));
    let edit = cell(&v, "Edit / Write", "claude");
    assert_eq!((edit.value, edit.source), (PermValue::Ask, None), "unset is the CLI's default");
    assert_eq!(cell(&v, "Read / Grep", "claude").value, PermValue::Allow, "read-only tools run by default");
    assert!(v.files.iter().any(|f| f.path == machine.display().to_string() && f.scope == ConfigScope::Machine && f.writable));

    // Project scope writes settings.local.json only.
    let written = apply(
        c,
        ConfigScope::Project,
        &s.project,
        &[change("Bash(git *)", "claude", Some(PermValue::Deny)), change("Edit / Write", "claude", Some(PermValue::Allow)), change("Bash(npm *, bun *)", "claude", Some(PermValue::Ask))],
    )
    .unwrap();
    assert_eq!(written, vec![local.clone()]);
    let j = json_of(&local);
    assert_eq!(j["skillOverrides"], json!({"verify": "off"}));
    assert_eq!(j["permissions"]["deny"], json!(["Skill(commit)", "Bash(git *)"]));
    assert_eq!(j["permissions"]["allow"], json!(["Edit", "Write"]));
    assert_eq!(j["permissions"]["ask"], json!(["Bash(npm *)", "Bash(bun *)"]));
    let m = json_of(&machine);
    assert_eq!(m["permissions"]["allow"], json!(["Bash(git:*)", "Bash(cargo test)"]), "the machine file is untouched");
    assert_eq!(m["hooks"], json!({"Stop": []}));

    let v = view(c, ConfigScope::Project, &s.project, &[]);
    let git = cell(&v, "Bash(git *)", "claude");
    assert_eq!((git.value, git.source), (PermValue::Deny, Some(ConfigSource::Override)), "a stricter project value overrides");
    assert_eq!(cell(&v, "Edit / Write", "claude").source, Some(ConfigSource::Project));

    // A project allow does not beat a machine deny: Claude checks deny rules from every file first.
    write(&machine, r#"{"permissions": {"deny": ["WebFetch", "WebSearch"]}}"#);
    apply(c, ConfigScope::Project, &s.project, &[change("WebFetch / WebSearch", "claude", Some(PermValue::Allow))]).unwrap();
    let web = view(c, ConfigScope::Project, &s.project, &[]);
    let web = cell(&web, "WebFetch / WebSearch", "claude");
    assert_eq!((web.value, web.source), (PermValue::Deny, Some(ConfigSource::Machine)));
    assert!(web.note.as_deref().unwrap().contains("does not apply"), "{:?}", web.note);

    // Inherited from the machine file.
    let v = view(c, ConfigScope::Project, &s.project, &[]);
    assert_eq!(cell(&v, "Bash(*)", "claude").value, PermValue::Ask);
    write(&machine, r#"{"permissions": {"allow": ["Bash(*)"]}}"#);
    let v = view(c, ConfigScope::Project, &s.project, &[]);
    let bash = cell(&v, "Bash(*)", "claude");
    assert_eq!((bash.value, bash.source), (PermValue::Allow, Some(ConfigSource::Machine)));
}

#[tokio::test]
async fn clearing_a_cell_removes_only_its_rules() {
    let s = setup().await;
    let c = &s.hub.config;
    let local = s.project.join(".claude/settings.local.json");
    write(&local, r#"{"permissions": {"allow": ["Read", "Bash(git:*)", "Bash(rm -rf build)"], "ask": ["Bash(git *)"]}}"#);
    apply(c, ConfigScope::Project, &s.project, &[change("Bash(git *)", "claude", None)]).unwrap();
    let j = json_of(&local);
    assert_eq!(j["permissions"]["allow"], json!(["Read", "Bash(rm -rf build)"]));
    assert_eq!(j["permissions"]["ask"], json!([]));
    let v = view(c, ConfigScope::Project, &s.project, &[]);
    let git = cell(&v, "Bash(git *)", "claude");
    assert_eq!((git.value, git.source), (PermValue::Ask, None));
    // `Read` alone of Read / Grep / Glob: partly set.
    let read = cell(&v, "Read / Grep", "claude");
    assert_eq!((read.value, read.source), (PermValue::Allow, Some(ConfigSource::Project)));
    assert!(read.note.as_deref().unwrap().contains("partly set"), "{:?}", read.note);
}

#[tokio::test]
async fn gemini_cells_use_its_allowed_and_exclude_lists() {
    let s = setup().await;
    let c = &s.hub.config;
    let machine = s.ah.path().join("gemini-home/.gemini/settings.json");
    write(&machine, r#"{"theme": "dark", "tools": {"exclude": ["run_command"], "sandbox": false}}"#);
    let v = view(c, ConfigScope::Machine, &s.project, &[]);
    assert_eq!(cell(&v, "Bash(*)", "gemini").value, PermValue::Deny);

    apply(c, ConfigScope::Machine, &s.project, &[change("Bash(git *)", "gemini", Some(PermValue::Allow)), change("WebFetch / WebSearch", "gemini", Some(PermValue::Deny))]).unwrap();
    let j = json_of(&machine);
    assert_eq!(j["theme"], "dark");
    assert_eq!(j["tools"]["sandbox"], false);
    assert_eq!(j["tools"]["allowed"], json!(["run_command(git)"]));
    assert_eq!(j["tools"]["exclude"], json!(["run_command", "read_url_content", "search_web"]));
    // Ask is neither list.
    apply(c, ConfigScope::Machine, &s.project, &[change("Bash(git *)", "gemini", Some(PermValue::Ask))]).unwrap();
    assert_eq!(json_of(&machine)["tools"]["allowed"], json!([]));

    // A project file that uses the older top-level keys keeps them.
    let project = s.project.join(".gemini/settings.json");
    write(&project, r#"{"excludeTools": ["write_to_file"], "mcpServers": {}}"#);
    apply(c, ConfigScope::Project, &s.project, &[change("Edit / Write", "gemini", Some(PermValue::Allow))]).unwrap();
    let j = json_of(&project);
    assert_eq!(j["allowedTools"], json!(["write_to_file", "replace_file_content"]));
    assert_eq!(j["excludeTools"], json!([]));
    assert!(j.get("tools").is_none());
    let v = view(c, ConfigScope::Project, &s.project, &[]);
    let e = cell(&v, "Edit / Write", "gemini");
    assert_eq!((e.value, e.source), (PermValue::Allow, Some(ConfigSource::Project)));
    let b = cell(&v, "Bash(*)", "gemini");
    assert_eq!((b.value, b.source), (PermValue::Deny, Some(ConfigSource::Machine)));
    assert!(v.providers.iter().find(|p| p.name == "gemini").unwrap().note.as_deref().unwrap().contains("no ask list"));
}

#[tokio::test]
async fn codex_is_unsupported_and_mcp_rows_follow_the_servers() {
    let s = setup().await;
    let c = &s.hub.config;
    write(&s.ah.path().join("codex/config.toml"), "approval_policy = \"on-request\"\n");
    let v = view(c, ConfigScope::Machine, &s.project, &["github".to_string()]);
    assert_eq!(v.providers.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["codex", "claude", "gemini"]);
    let codex = v.providers.iter().find(|p| p.name == "codex").unwrap();
    assert!(codex.note.as_deref().unwrap().contains("approval_policy = \"on-request\""), "{:?}", codex.note);
    for row in &v.rows {
        let cell = row.cells.iter().find(|c| c.provider == "codex").unwrap();
        assert_eq!((cell.value, cell.source), (PermValue::Unsupported, None));
    }
    let err = apply(c, ConfigScope::Machine, &s.project, &[change("Bash(*)", "codex", Some(PermValue::Deny))]).unwrap_err();
    assert!(format!("{err:#}").contains("codex has no setting"), "{err:#}");
    assert!(apply(c, ConfigScope::Machine, &s.project, &[change("Bash(rm *)", "claude", Some(PermValue::Deny))]).is_err(), "unknown rule");
    assert!(apply(c, ConfigScope::Machine, &s.project, &[change("Bash(*)", "copilot", Some(PermValue::Deny))]).is_err(), "not a provider MAP runs");

    // The MCP row: Claude's `mcp__github`; agy has no list for it.
    let row = v.rows.iter().find(|r| r.rule == "mcp__github__*").unwrap();
    assert_eq!(row.mcp_server.as_deref(), Some("github"));
    assert_eq!(row.cells.iter().find(|c| c.provider == "gemini").unwrap().value, PermValue::Unsupported);
    apply(c, ConfigScope::Machine, &s.project, &[change("mcp__github__*", "claude", Some(PermValue::Allow))]).unwrap();
    assert_eq!(json_of(&s.ah.path().join("claude/settings.json"))["permissions"]["allow"], json!(["mcp__github"]));
    let v = view(c, ConfigScope::Machine, &s.project, &["github".to_string()]);
    assert_eq!(cell(&v, "mcp__github__*", "claude").value, PermValue::Allow);
}

#[tokio::test]
async fn an_invalid_file_is_reported_and_never_written() {
    let s = setup().await;
    let c = &s.hub.config;
    let local = s.project.join(".claude/settings.local.json");
    write(&local, "{ not json");
    let v = view(c, ConfigScope::Project, &s.project, &[]);
    let f = v.files.iter().find(|f| f.path == local.display().to_string()).unwrap();
    assert!(f.error.is_some() && !f.writable);
    assert!(apply(c, ConfigScope::Project, &s.project, &[change("Bash(*)", "claude", Some(PermValue::Deny))]).is_err());
    assert_eq!(std::fs::read_to_string(&local).unwrap(), "{ not json");
}
