use super::*;
use crate::config::SkillsConfig;

fn put_file(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read_to(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// A project scope on a fresh folder with the default context files (claude, codex, copilot, gemini).
fn project_scope(folder: &Path) -> Scope {
    Scope {
        project: Some("app".into()),
        folder: Some(folder.canonicalize().unwrap()),
        context_files: SkillsConfig::default().context_files,
        machine: vec![],
        extras: vec![],
    }
}

/// CLAUDE.md imports AGENTS.md on a line of its own; AGENTS.md points at docs/style.md inside a sentence.
fn sample() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    put_file(&dir.path().join("CLAUDE.md"), "@AGENTS.md\nExtra for Claude.\n");
    put_file(&dir.path().join("AGENTS.md"), "# Rules\nSee @docs/style.md for style.\n```\n@not/an/import.md\n```\n");
    put_file(&dir.path().join("docs/style.md"), "Use tabs.\n");
    dir
}

fn file<'a>(o: &'a RulesOverview, path: &str) -> &'a RuleFile {
    o.files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path} not listed: {:?}", o.files.iter().map(|f| &f.path).collect::<Vec<_>>()))
}

fn reader<'a>(f: &'a RuleFile, provider: &str) -> &'a RuleReader {
    f.reader_options.iter().find(|r| r.provider == provider).unwrap()
}

#[test]
fn imports_are_whole_lines_or_inline_and_never_in_code_fences() {
    let text = "@AGENTS.md\n@./docs/a.md\nsee @docs/b.md and @mention\nmail me@x.org\n```\n@docs/c.md\n```\n";
    let got = imports(text);
    assert_eq!(
        got,
        [
            Import { target: "AGENTS.md".into(), whole_line: true },
            Import { target: "docs/a.md".into(), whole_line: true },
            Import { target: "docs/b.md".into(), whole_line: false },
        ]
    );
    let (kept, removed) = remove_import_lines(text, Path::new(""), Path::new("docs/a.md"));
    assert_eq!((kept.as_str(), removed), ("@AGENTS.md\nsee @docs/b.md and @mention\nmail me@x.org\n```\n@docs/c.md\n```\n", 1));
    // An inline import is not a line MAP removes.
    assert_eq!(remove_import_lines(text, Path::new(""), Path::new("docs/b.md")).1, 0);
    assert_eq!(import_path(Path::new(".claude"), Path::new("docs/a.md")), "../docs/a.md");
    assert_eq!(import_path(Path::new(""), Path::new("docs/a.md")), "docs/a.md");
}

#[test]
fn a_project_lists_roots_imports_and_who_reads_each() {
    let dir = sample();
    let o = list(&project_scope(dir.path()));
    let paths: Vec<&str> = o.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(paths, ["CLAUDE.md", "AGENTS.md", "docs/style.md", ".github/copilot-instructions.md", "GEMINI.md"]);

    let agents = file(&o, "AGENTS.md");
    assert_eq!((agents.exists, agents.lines, agents.source), (true, 5, ConfigSource::Project));
    assert_eq!(agents.readers, ["claude", "codex"]);
    assert_eq!(agents.root_of, ["codex"]);
    assert_eq!(agents.imports, ["docs/style.md"]);
    assert_eq!((reader(agents, "claude").how.as_str(), reader(agents, "claude").fixed), ("via CLAUDE.md", false));
    assert_eq!((reader(agents, "codex").how.as_str(), reader(agents, "codex").fixed), ("own file", true));
    let gemini = reader(agents, "gemini");
    assert!(!gemini.reads && !gemini.fixed && gemini.note.contains("@AGENTS.md"), "{gemini:?}");
    assert!(reader(agents, "copilot").fixed && !reader(agents, "copilot").reads);

    // Claude reaches the style guide through AGENTS.md; Codex does not follow imports.
    let style = file(&o, "docs/style.md");
    assert_eq!(style.readers, ["claude"]);
    assert_eq!((reader(style, "claude").how.as_str(), reader(style, "claude").fixed), ("via AGENTS.md", true));
    assert_eq!(style.body, "Use tabs.\n");
    let claude = file(&o, "CLAUDE.md");
    assert_eq!((claude.readers.clone(), claude.imports.clone()), (vec!["claude".to_string()], vec!["AGENTS.md".to_string()]));
    assert!(!file(&o, "GEMINI.md").exists);
}

#[test]
fn readers_add_and_remove_only_import_lines() {
    let dir = sample();
    let s = project_scope(dir.path());
    let all = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let done = set_readers(&s, "AGENTS.md", &all(&["claude", "codex", "gemini"])).unwrap();
    assert_eq!(done, ["+gemini via GEMINI.md"]);
    assert_eq!(read_to(&dir.path().join("GEMINI.md")), "@AGENTS.md\n");
    assert_eq!(file(&list(&s), "AGENTS.md").readers, ["claude", "codex", "gemini"]);

    set_readers(&s, "AGENTS.md", &all(&["codex", "gemini"])).unwrap();
    assert_eq!(read_to(&dir.path().join("CLAUDE.md")), "Extra for Claude.\n", "only the import line went");
    assert_eq!(read_to(&dir.path().join("AGENTS.md")), "# Rules\nSee @docs/style.md for style.\n```\n@not/an/import.md\n```\n", "the file itself is untouched");

    // Fixed readers: Codex reads only its own AGENTS.md; an import inside a sentence is not MAP's to remove.
    let err = format!("{:#}", set_readers(&s, "AGENTS.md", &all(&["gemini"])).unwrap_err());
    assert!(err.contains("codex"), "{err}");
    let err = format!("{:#}", set_readers(&s, "docs/style.md", &all(&["codex"])).unwrap_err());
    assert!(err.contains("codex reads only its own AGENTS.md"), "{err}");
    assert!(set_readers(&s, "AGENTS.md", &all(&["codex", "gemini", "aider"])).is_err(), "no context file for aider");
    // Nothing was written by the refused calls.
    assert_eq!(read_to(&dir.path().join("GEMINI.md")), "@AGENTS.md\n");
}

#[test]
fn a_new_rule_file_counts_as_read_only_once_imported() {
    let dir = sample();
    let s = project_scope(dir.path());
    assert_eq!(write(&s, "docs/rules/testing.md", "Write tests first.\n").unwrap().as_deref(), Some("docs/rules/testing.md"));
    assert_eq!(write(&s, "GEMINI.md", "Be brief.\n").unwrap(), None, "a root file is listed anyway");
    let s = Scope { extras: vec!["docs/rules/testing.md".into()], ..s };
    let f = file(&list(&s), "docs/rules/testing.md").clone();
    assert!(f.exists && f.readers.is_empty(), "{f:?}");

    set_readers(&s, "docs/rules/testing.md", &["claude".to_string(), "gemini".to_string()]).unwrap();
    assert_eq!(read_to(&dir.path().join("CLAUDE.md")), "@AGENTS.md\nExtra for Claude.\n@docs/rules/testing.md\n");
    assert_eq!(read_to(&dir.path().join("GEMINI.md")), "Be brief.\n@docs/rules/testing.md\n");
    assert_eq!(file(&list(&s), "docs/rules/testing.md").readers, ["claude", "gemini"]);

    // Deleting it takes its import lines along, and nothing else.
    let edited = delete(&s, "docs/rules/testing.md").unwrap();
    assert_eq!(edited, ["CLAUDE.md", "GEMINI.md"]);
    assert!(!dir.path().join("docs/rules/testing.md").exists());
    assert_eq!(read_to(&dir.path().join("CLAUDE.md")), "@AGENTS.md\nExtra for Claude.\n");
    assert_eq!(read_to(&dir.path().join("GEMINI.md")), "Be brief.\n");
    assert!(!list(&s).files.iter().any(|f| f.path == "docs/rules/testing.md"));
}

#[test]
fn paths_outside_the_project_folder_are_refused() {
    let dir = sample();
    let outside = tempfile::tempdir().unwrap();
    put_file(&outside.path().join("secret.md"), "outside\n");
    std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret.md"), dir.path().join("docs/out.md")).unwrap();
    let s = project_scope(dir.path());
    for bad in ["../x.md", "/etc/x.md", "~/x.md", "docs/../../x.md", ".git/x.md", "notes.txt", "", "docs/"] {
        assert!(write(&s, bad, "x").is_err(), "{bad} was accepted");
        assert!(delete(&s, bad).is_err(), "{bad} was accepted");
    }
    for escape in ["link/evil.md", "docs/out.md", "link/secret.md"] {
        let err = format!("{:#}", write(&s, escape, "x").unwrap_err());
        assert!(err.contains("outside the project folder"), "{escape}: {err}");
        assert!(delete(&s, escape).is_err());
        assert!(set_readers(&s, escape, &[]).is_err());
    }
    assert_eq!(read_to(&outside.path().join("secret.md")), "outside\n");
    assert!(!outside.path().join("evil.md").exists());
    // An import that leads out is not listed either.
    put_file(&dir.path().join("CLAUDE.md"), "@docs/out.md\n@../escape.md\n");
    assert!(list(&s).files.iter().all(|f| f.path != "docs/out.md" && !f.path.contains("escape")));
    // Without a project only machine files can be named.
    let machine = Scope { project: None, folder: None, ..s };
    assert!(format!("{:#}", write(&machine, "CLAUDE.md", "x").unwrap_err()).contains("not a machine rule file"));
}

#[test]
fn bodies_are_capped_both_ways() {
    let dir = sample();
    let s = project_scope(dir.path());
    assert!(write(&s, "big.md", &"x".repeat(MAX_BYTES + 1)).is_err());
    std::fs::write(dir.path().join("AGENTS.md"), format!("{}\n", "y".repeat(MAX_BYTES + 10))).unwrap();
    let agents = file(&list(&s), "AGENTS.md").clone();
    assert!(agents.truncated && agents.body.len() == MAX_BYTES && agents.bytes == (MAX_BYTES + 11) as u64);
}

#[tokio::test]
async fn machine_files_are_each_clis_own_file_in_the_isolated_home() {
    let (hub, _, dir) = crate::hooks::tests::hub("", "").await;
    let s = scope(&hub, None).await.unwrap();
    let homes: Vec<(&str, &Path)> = s.machine.iter().map(|m| (m.provider.as_str(), m.abs.as_path())).collect();
    assert_eq!(homes.len(), 3, "{homes:?}");
    for (provider, abs) in &homes {
        assert!(abs.starts_with(dir.path()), "{provider}: {} is outside the temp dir", abs.display());
    }
    assert!(homes[0].1.ends_with("claude/CLAUDE.md") && homes[1].1.ends_with("codex/AGENTS.md") && homes[2].1.ends_with(".gemini/GEMINI.md"), "{homes:?}");

    let claude = s.machine[0].display.clone();
    let o = put(&hub, None, claude.clone(), "Always answer briefly.\n".into()).await.unwrap();
    assert_eq!(read_to(&s.machine[0].abs), "Always answer briefly.\n");
    let f = file(&o, &claude);
    assert_eq!((f.source, f.exists, f.readers.clone()), (ConfigSource::Machine, true, vec!["claude".to_string()]));
    assert!(f.reader_options.iter().all(|r| r.fixed));
    assert!(o.files.iter().all(|f| f.source == ConfigSource::Machine), "machine scope lists machine files only");
    assert!(readers(&hub, None, claude.clone(), vec!["claude".into(), "codex".into()]).await.is_err());
    remove(&hub, None, claude).await.unwrap();
    assert!(!s.machine[0].abs.exists());
}

#[tokio::test]
async fn a_project_scope_lists_its_files_then_the_machine_files_and_remembers_new_ones() {
    let (hub, _, dir) = crate::hooks::tests::hub("", "").await;
    let slug = crate::hooks::tests::project(&hub, &dir.path().join("app"), "App").await;
    put_file(&dir.path().join("app/AGENTS.md"), "Rules.\n");
    let o = put(&hub, Some(&slug), "docs/rules/api.md".into(), "REST only.\n".into()).await.unwrap();
    let sources: Vec<(&str, ConfigSource)> = o.files.iter().filter(|f| f.exists || f.source == ConfigSource::Machine).map(|f| (f.path.as_str(), f.source)).collect();
    assert_eq!(&sources[..2], [("AGENTS.md", ConfigSource::Project), ("docs/rules/api.md", ConfigSource::Project)]);
    assert_eq!(sources[2..].iter().filter(|(_, s)| *s == ConfigSource::Machine).count(), 3);
    assert!(o.files.iter().position(|f| f.source == ConfigSource::Machine).unwrap() > o.files.iter().rposition(|f| f.source == ConfigSource::Project).unwrap());
    // Listed again later because MAP made it, before anything imports it.
    let again = overview(&hub, Some(&slug)).await.unwrap();
    assert!(file(&again, "docs/rules/api.md").readers.is_empty());
    let after = remove(&hub, Some(&slug), "docs/rules/api.md".into()).await.unwrap();
    assert!(!after.files.iter().any(|f| f.path == "docs/rules/api.md"));
    assert_eq!(hub.store.runtime_setting(&extras_key(&slug)).await.unwrap().as_deref(), Some("[]"));
}
