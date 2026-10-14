//! The project folder: where project folders may be (`[projects] path`, or several named `roots`), checked on the
//! server for every way a project gets a folder (an existing one, a new one, a clone target), and the dashboard's
//! folder picker (`/api/fs/*`), which lists the sub-folders of a folder inside a root and nothing else.
//!
//! A path is judged by where it really is: the canonical path of the folder, or for one that does not exist yet,
//! the canonical path of its nearest existing ancestor plus the missing names. So a symlink cannot lead out of a
//! root, and `..` is refused outright. What an agent may read or run once it is started is not decided here: that
//! is the CLI's own permissions or sandbox (or, in Docker, the container's mounts).

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

use crate::config::ProjectRoot;
use crate::model::{FsEntry, FsEntryKind, FsListing, FsRoot};

/// Most folders one listing returns (`truncated` says when there were more).
pub const MAX_ENTRIES: usize = 500;
/// Entries looked at to tell whether a folder has sub-folders; past this it is assumed to.
const CHILD_SCAN: usize = 256;

/// Lexical clean-up only (drops `.` and repeated or trailing slashes); never touches the disk. `..` is kept, for
/// callers to refuse.
pub fn normalize(p: &Path) -> PathBuf {
    p.components().collect()
}

/// The root's real folder; `None` when it does not exist or is not a folder.
pub fn canonical_root(root: &ProjectRoot) -> Option<PathBuf> {
    root.path.canonicalize().ok().filter(|p| p.is_dir())
}

/// Where `path` really is: its canonical path, or for a path that does not exist yet, the canonical path of its
/// nearest existing ancestor joined with the missing names. Refuses relative paths, `..` and broken symlinks.
pub fn real_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("{} is not an absolute path", path.display());
    }
    if path.components().any(|c| c == Component::ParentDir) {
        bail!("{}: `..` is not allowed in a folder path", path.display());
    }
    let mut existing = normalize(path);
    let mut missing: Vec<OsString> = vec![];
    loop {
        match std::fs::symlink_metadata(&existing) {
            Ok(meta) => {
                if meta.file_type().is_symlink() && !existing.exists() {
                    bail!("{} is a broken symlink", existing.display());
                }
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name() else { bail!("{} does not exist", existing.display()) };
                missing.push(name.to_os_string());
                existing.pop();
            }
            Err(e) => bail!("{}: {e}", existing.display()),
        }
    }
    let mut real = existing.canonicalize()?;
    for name in missing.iter().rev() {
        real.push(name);
    }
    Ok(real)
}

/// The root a real path is inside (the root folder itself counts), with the root's own real path.
pub fn root_of<'a>(roots: &'a [ProjectRoot], real: &Path) -> Option<(&'a ProjectRoot, PathBuf)> {
    roots.iter().find_map(|r| canonical_root(r).filter(|c| real.starts_with(c)).map(|c| (r, c)))
}

/// The root a project's folder is in, by name; `None` when it is outside every root.
pub fn root_name(roots: &[ProjectRoot], folder: &Path) -> Option<String> {
    let real = real_path(folder).ok()?;
    root_of(roots, &real).map(|(r, _)| r.name.clone())
}

fn shown(r: &ProjectRoot) -> String {
    match &r.host_path {
        Some(h) => format!("{} (on the host {})", r.path.display(), h.display()),
        None => r.path.display().to_string(),
    }
}

/// The allowed roots for an error message: "the project folder /x", or "the project folders A (/x), B (/y)".
pub fn describe(roots: &[ProjectRoot]) -> String {
    match roots {
        [one] => format!("the project folder {}", shown(one)),
        many => format!("the project folders {}", many.iter().map(|r| format!("{} ({})", r.name, shown(r))).collect::<Vec<_>>().join(", ")),
    }
}

/// Checks a folder a project is about to use (existing or not): it must be inside a root, and not the root itself
/// unless that root sets `allow_root_itself`. Returns its real path and the root.
pub fn check_project_folder<'a>(roots: &'a [ProjectRoot], path: &Path) -> Result<(PathBuf, &'a ProjectRoot)> {
    let real = real_path(path)?;
    let Some((root, canon)) = root_of(roots, &real) else {
        let missing: Vec<String> = roots.iter().filter(|r| canonical_root(r).is_none()).map(|r| r.path.display().to_string()).collect();
        let missing = if missing.is_empty() { String::new() } else { format!(" ({} does not exist yet)", missing.join(", ")) };
        let shown_path = if real == path { path.display().to_string() } else { format!("{} (really {})", path.display(), real.display()) };
        bail!(
            "{shown_path} is outside {}{missing}: projects can only be made in its sub-folders. Pick one there, or change [projects] in map.toml and restart MAP",
            describe(roots)
        );
    };
    if real == canon && !root.allow_root_itself {
        bail!("{} is the project folder itself ({}): pick or create a sub-folder in it", path.display(), root.name);
    }
    Ok((real, root))
}

/// `real` (inside `canon_root`) as the host sees it, when the root has a `host_path`.
pub fn host_path_of(root: &ProjectRoot, canon_root: &Path, real: &Path) -> Option<String> {
    let host = root.host_path.as_ref()?;
    let rel = real.strip_prefix(canon_root).ok()?;
    Some(if rel.as_os_str().is_empty() { host.clone() } else { host.join(rel) }.display().to_string())
}

/// `GET /api/fs/roots`.
pub fn list_roots(roots: &[ProjectRoot], default: &str) -> Vec<FsRoot> {
    roots
        .iter()
        .map(|r| FsRoot {
            name: r.name.clone(),
            path: r.path.display().to_string(),
            host_path: r.host_path.as_ref().map(|h| h.display().to_string()),
            exists: canonical_root(r).is_some(),
            allow_root_itself: r.allow_root_itself,
            is_default: r.name == default,
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum BrowseError {
    /// 400: an absolute path, `..`, a symlink out of the root, MAP's data folder.
    Bad(String),
    /// 404: no such root or folder.
    NotFound(String),
}

fn has_children(dir: &Path, hidden: bool) -> bool {
    let Ok(read) = std::fs::read_dir(dir) else { return false };
    for (i, e) in read.flatten().enumerate() {
        if i >= CHILD_SCAN {
            return true;
        }
        if !hidden && e.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if e.file_type().is_ok_and(|t| t.is_dir() || (t.is_symlink() && e.path().is_dir())) {
            return true;
        }
    }
    false
}

fn rel_string(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")
}

/// The sub-folders of `rel` (relative to the root named `root`). `projects` are the projects' real folders by slug,
/// for the "used by" badges; `data_dir` (MAP's own) is never listed or entered.
pub fn browse(roots: &[ProjectRoot], data_dir: &Path, projects: &[(String, PathBuf)], root: &str, rel: &str, hidden: bool) -> Result<FsListing, BrowseError> {
    let Some(r) = roots.iter().find(|r| r.name == root) else {
        return Err(BrowseError::NotFound(format!("no project folder named `{root}`")));
    };
    if rel.starts_with('/') || rel.contains('\0') {
        return Err(BrowseError::Bad("give the path relative to the project folder".into()));
    }
    let segments: Vec<&str> = rel.split('/').filter(|s| !s.is_empty()).collect();
    if segments.iter().any(|s| *s == ".." || *s == ".") {
        return Err(BrowseError::Bad("`..` and `.` are not allowed in the path".into()));
    }
    let Some(canon) = canonical_root(r) else {
        return Err(BrowseError::NotFound(format!("{} does not exist on this machine", r.path.display())));
    };
    let dir = segments.iter().fold(canon.clone(), |p, s| p.join(s));
    let real = dir.canonicalize().map_err(|_| BrowseError::NotFound(format!("no folder `{rel}` in {}", r.name)))?;
    if !real.starts_with(&canon) {
        return Err(BrowseError::Bad(format!("`{rel}` leads out of {} through a symlink", r.name)));
    }
    if !real.is_dir() {
        return Err(BrowseError::Bad(format!("`{rel}` is not a folder")));
    }
    let data = data_dir.canonicalize().ok();
    if data.as_ref().is_some_and(|d| real.starts_with(d)) {
        return Err(BrowseError::Bad("MAP's data folder cannot be browsed".into()));
    }

    let read = std::fs::read_dir(&real).map_err(|e| BrowseError::Bad(format!("cannot read `{rel}`: {e}")))?;
    let (mut files, mut hidden_dirs) = (0u64, 0u64);
    let mut dirs: Vec<(String, PathBuf, FsEntryKind)> = vec![];
    for entry in read.flatten() {
        // Names that are not UTF-8 cannot be sent back as a path, so they are not offered.
        let (Ok(name), Ok(ft)) = (entry.file_name().into_string(), entry.file_type()) else { continue };
        let (target, kind) = if ft.is_dir() {
            (entry.path(), FsEntryKind::Dir)
        } else if ft.is_symlink() {
            match entry.path().canonicalize() {
                Ok(t) if t.is_dir() && t.starts_with(&canon) => (t, FsEntryKind::Symlink),
                // A symlink to a folder outside this root is left out.
                Ok(t) if t.is_dir() => continue,
                _ => {
                    files += 1;
                    continue;
                }
            }
        } else {
            files += 1;
            continue;
        };
        if data.as_ref().is_some_and(|d| target.starts_with(d)) {
            continue;
        }
        if name.starts_with('.') && !hidden {
            hidden_dirs += 1;
            continue;
        }
        dirs.push((name, target, kind));
    }
    dirs.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    let truncated = dirs.len() > MAX_ENTRIES;
    dirs.truncate(MAX_ENTRIES);
    let entries = dirs
        .into_iter()
        .map(|(name, target, kind)| FsEntry {
            is_git: target.join(".git").exists(),
            is_project: projects.iter().find(|(_, p)| *p == target).map(|(s, _)| s.clone()),
            contains_project: projects.iter().find(|(_, p)| *p != target && p.starts_with(&target)).map(|(s, _)| s.clone()),
            has_children: has_children(&target, hidden),
            hidden: name.starts_with('.'),
            kind,
            name,
        })
        .collect();
    let rel_path = real.strip_prefix(&canon).map(rel_string).unwrap_or_default();
    Ok(FsListing {
        root: r.name.clone(),
        parent: (real != canon).then(|| real.parent().and_then(|p| p.strip_prefix(&canon).ok()).map(rel_string).unwrap_or_default()),
        abs_path: real.display().to_string(),
        host_path: host_path_of(r, &canon, &real),
        is_root: real == canon,
        in_project: projects.iter().find(|(_, p)| real.starts_with(p)).map(|(s, _)| s.clone()),
        path: rel_path,
        entries,
        truncated,
        files,
        hidden_dirs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn root(name: &str, path: &Path) -> ProjectRoot {
        ProjectRoot { name: name.into(), path: path.to_path_buf(), host_path: None, allow_root_itself: false }
    }

    /// `<tmp>/projects` (the root, with `app/` and a file) next to `<tmp>/outside/` and `<tmp>/data/`.
    fn layout() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let projects = base.join("projects");
        std::fs::create_dir_all(projects.join("app")).unwrap();
        std::fs::create_dir_all(base.join("outside/secret")).unwrap();
        std::fs::create_dir_all(base.join("data")).unwrap();
        std::fs::write(projects.join("notes.txt"), "x").unwrap();
        (dir, base, projects)
    }

    #[test]
    fn folders_must_be_inside_a_root_and_not_the_root_itself() {
        let (_dir, base, projects) = layout();
        let roots = vec![root("Projects", &projects)];
        // Existing and new folders inside, including under sub-folders that do not exist yet.
        assert_eq!(check_project_folder(&roots, &projects.join("app")).unwrap().0, projects.join("app"));
        assert_eq!(check_project_folder(&roots, &projects.join("new/deeper/app")).unwrap().0, projects.join("new/deeper/app"));
        assert_eq!(check_project_folder(&roots, &projects.join("./app/")).unwrap().0, projects.join("app"));
        // Outside, the root itself, `..`, relative paths.
        let err = check_project_folder(&roots, &base.join("outside")).unwrap_err().to_string();
        assert!(err.contains("is outside the project folder") && err.contains(&projects.display().to_string()), "{err}");
        let err = check_project_folder(&roots, &projects).unwrap_err().to_string();
        assert!(err.contains("is the project folder itself"), "{err}");
        assert!(check_project_folder(&roots, &projects.join("../outside")).unwrap_err().to_string().contains("`..`"));
        assert!(check_project_folder(&roots, &projects.join("app/../../outside/x")).is_err());
        assert!(check_project_folder(&roots, Path::new("projects/app")).is_err());
        // The root itself, when allowed.
        let roots_itself = vec![ProjectRoot { allow_root_itself: true, ..root("Projects", &projects) }];
        assert!(check_project_folder(&roots_itself, &projects).is_ok());
        // Several roots: the message names each.
        let two = vec![root("Projects", &projects), root("Data", &base.join("nope"))];
        let err = check_project_folder(&two, &base.join("outside")).unwrap_err().to_string();
        assert!(err.contains("Projects (") && err.contains("Data (") && err.contains("does not exist yet"), "{err}");
        assert_eq!(check_project_folder(&two, &projects.join("app")).unwrap().1.name, "Projects");
    }

    #[test]
    fn symlinks_cannot_lead_out_of_a_root() {
        let (_dir, base, projects) = layout();
        let roots = vec![root("Projects", &projects)];
        symlink(base.join("outside"), projects.join("escape")).unwrap();
        symlink(projects.join("app"), projects.join("alias")).unwrap();
        symlink(base.join("missing"), projects.join("broken")).unwrap();
        let err = check_project_folder(&roots, &projects.join("escape")).unwrap_err().to_string();
        assert!(err.contains("really") && err.contains("outside"), "{err}");
        assert!(check_project_folder(&roots, &projects.join("escape/new")).is_err(), "a new folder under an escaping link");
        assert!(check_project_folder(&roots, &projects.join("broken")).unwrap_err().to_string().contains("broken symlink"));
        assert_eq!(check_project_folder(&roots, &projects.join("alias")).unwrap().0, projects.join("app"), "a link inside resolves");
        // A root reached through a symlink: its real folder is what counts.
        symlink(&projects, base.join("link-to-projects")).unwrap();
        let linked = vec![root("Linked", &base.join("link-to-projects"))];
        assert!(check_project_folder(&linked, &projects.join("app")).is_ok());
        assert!(check_project_folder(&linked, &base.join("link-to-projects/new")).is_ok());
        assert_eq!(root_name(&linked, &projects.join("app")).as_deref(), Some("Linked"));
        assert_eq!(root_name(&linked, &base.join("outside")), None);
    }

    #[test]
    fn browse_lists_folders_only_with_badges() {
        let (_dir, base, projects) = layout();
        let roots = vec![ProjectRoot { host_path: Some("/home/me/project".into()), ..root("Projects", &projects) }];
        std::fs::create_dir_all(projects.join("repo/.git")).unwrap();
        std::fs::create_dir_all(projects.join("Beta/inner")).unwrap();
        std::fs::create_dir_all(projects.join(".cache")).unwrap();
        std::fs::create_dir_all(projects.join("mono/svc")).unwrap();
        symlink(projects.join("repo"), projects.join("repo-link")).unwrap();
        symlink(base.join("outside"), projects.join("escape")).unwrap();
        symlink(projects.join("notes.txt"), projects.join("notes-link")).unwrap();
        let used = vec![("app".to_string(), projects.join("app")), ("svc".to_string(), projects.join("mono/svc"))];
        let data = base.join("data");

        let l = browse(&roots, &data, &used, "Projects", "", false).unwrap();
        let names: Vec<&str> = l.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["app", "Beta", "mono", "repo", "repo-link"], "sorted, folders only, no dot-folders, no escaping link");
        assert_eq!((l.path.as_str(), l.parent.as_deref(), l.is_root, l.files, l.hidden_dirs, l.truncated), ("", None, true, 2, 1, false));
        assert_eq!(l.host_path.as_deref(), Some("/home/me/project"));
        let e = |n: &str| l.entries.iter().find(|e| e.name == n).unwrap();
        assert!(e("repo").is_git && !e("app").is_git);
        assert_eq!(e("repo-link").kind, FsEntryKind::Symlink);
        assert_eq!(e("app").is_project.as_deref(), Some("app"));
        assert_eq!(e("mono").contains_project.as_deref(), Some("svc"));
        assert!(e("Beta").has_children && !e("app").has_children && !e("repo").has_children, "a .git alone is not a sub-folder");

        let hidden = browse(&roots, &data, &used, "Projects", "", true).unwrap();
        assert!(hidden.entries.iter().any(|e| e.name == ".cache" && e.hidden) && hidden.hidden_dirs == 0);

        let inner = browse(&roots, &data, &used, "Projects", "Beta/", false).unwrap();
        assert_eq!((inner.path.as_str(), inner.parent.as_deref(), inner.is_root), ("Beta", Some(""), false));
        assert_eq!(inner.host_path.as_deref(), Some("/home/me/project/Beta"));
        let svc = browse(&roots, &data, &used, "Projects", "mono/svc", false).unwrap();
        assert_eq!((svc.parent.as_deref(), svc.in_project.as_deref()), (Some("mono"), Some("svc")));
        assert_eq!(browse(&roots, &data, &used, "Projects", "repo-link", false).unwrap().path, "repo", "a link inside resolves");

        // Traversal, absolute paths, escaping links, files, unknown roots.
        for bad in ["..", "app/../..", "/etc", "./app", "escape", "notes.txt"] {
            assert!(matches!(browse(&roots, &data, &used, "Projects", bad, false), Err(BrowseError::Bad(_))), "{bad}");
        }
        assert!(matches!(browse(&roots, &data, &used, "Projects", "nope", false), Err(BrowseError::NotFound(_))));
        assert!(matches!(browse(&roots, &data, &used, "Other", "", false), Err(BrowseError::NotFound(_))));
    }

    #[test]
    fn browse_caps_entries_and_hides_the_data_dir() {
        let (_dir, base, projects) = layout();
        for i in 0..(MAX_ENTRIES + 5) {
            std::fs::create_dir(projects.join(format!("d{i:04}"))).unwrap();
        }
        let roots = vec![root("Projects", &projects)];
        let l = browse(&roots, &base.join("data"), &[], "Projects", "", false).unwrap();
        assert_eq!((l.entries.len(), l.truncated), (MAX_ENTRIES, true));
        assert_eq!(l.entries[0].name, "app");

        // MAP's data dir inside a root is never listed or entered.
        let data = projects.join("map-data");
        std::fs::create_dir_all(data.join("git")).unwrap();
        let l = browse(&roots, &data, &[], "Projects", "", false).unwrap();
        assert!(l.entries.iter().all(|e| e.name != "map-data"));
        assert!(matches!(browse(&roots, &data, &[], "Projects", "map-data/git", false), Err(BrowseError::Bad(_))));
    }

    #[test]
    fn roots_listing_reports_existence_and_default() {
        let (_dir, base, projects) = layout();
        let roots = vec![root("Projects", &projects), root("Later", &base.join("later"))];
        let listed = list_roots(&roots, "Projects");
        assert_eq!(listed.iter().map(|r| (r.name.as_str(), r.exists, r.is_default)).collect::<Vec<_>>(), [("Projects", true, true), ("Later", false, false)]);
    }
}
