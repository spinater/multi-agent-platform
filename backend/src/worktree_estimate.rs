//! What running a project's tasks side by side in worktrees would cost this machine (`model::WorktreeEstimate`):
//! the checkout and the ignored build folders each worktree repeats, the memory of the agent CLIs (measured from
//! running ones when there are any) plus a rule of thumb for the verify build, the CPU the builds want, how many
//! tasks at once fit, how much sooner the project's open tasks would finish, and what the speed costs.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::hub::Hub;
use crate::model::{AgentMemory, Drawback, Level, MachineSpec, Project, SizedPath, TaskStatus, WorktreeEstimate};
use crate::worktrees::Git;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;

/// Ignored folders a fresh worktree does not have, which setup or the first build makes again.
const REBUILT: &[&str] = &["node_modules", "target", ".venv", "venv", ".next", "dist", "build", "vendor", ".gradle", "Pods", ".turbo", "__pycache__"];

pub async fn estimate(hub: &Hub, project: &Project, parallel: Option<u32>, copy: &[String], setup: &[String]) -> WorktreeEstimate {
    let folder = hub.workspace_of(project);
    let parallel = parallel.unwrap_or(hub.config.orchestrator.max_parallel as u32).clamp(1, 64);
    let machine = machine(&hub.project_dir(&project.slug)).await;

    let (checkout_bytes, tracked_files) = checkout(hub, &folder).await;
    let mut extra = rebuilt_folders(hub, &folder).await;
    for c in copy.iter().filter(|c| crate::worktrees::validate_copy(c).is_ok()) {
        let path = folder.join(c);
        if path.exists() {
            let bytes = size_of(path, Duration::from_secs(2)).await;
            extra.push(SizedPath { path: c.clone(), bytes, copied: true });
        }
    }
    let per_task_disk = checkout_bytes + extra.iter().map(|e| e.bytes).sum::<u64>();

    let providers: Vec<String> = if project.settings.providers.is_empty() {
        hub.config.orchestrator.provider_order.clone()
    } else {
        project.settings.providers.iter().map(|p| p.name.clone()).collect()
    };
    let agents = agent_memory(hub, &providers).await;
    let kind = BuildKind::of(&folder);
    let verify = project.settings.verify.clone().unwrap_or_else(|| hub.config.orchestrator.verify.clone());
    let (build_memory, build_note) = if verify.is_empty() {
        (128 * MIB, "no verify commands: the agent's own tool runs only".to_string())
    } else {
        (kind.memory(), kind.note().to_string())
    };
    let agent_max = agents.iter().map(|a| a.bytes).max().unwrap_or(400 * MIB);
    let per_task_memory = agent_max + build_memory;
    let total_disk = per_task_disk * parallel as u64;
    let total_memory = per_task_memory * parallel as u64;
    let threads_per_task = if verify.is_empty() { 1 } else { kind.threads(machine.cpus) };
    let peak_threads = threads_per_task.saturating_mul(parallel);

    // What fits: memory and disk with headroom, and CPU by how heavy one task's build is.
    let memory_room = machine.memory_available.unwrap_or(machine.memory_total * 6 / 10) * 8 / 10;
    let by_memory = (memory_room / per_task_memory.max(1)) as u32;
    let by_disk = machine.disk_free.map_or(u32::MAX, |free| (free * 8 / 10 / per_task_disk.max(1)) as u32);
    let by_cpu = if verify.is_empty() { machine.cpus * 2 } else { (machine.cpus as f64 / kind.cpu_cost()).floor() as u32 };
    let (recommended_parallel, limited_by) = [
        (by_memory, format!("memory: about {} free for MAP, {} per task", human(memory_room), human(per_task_memory))),
        (by_disk, format!("disk: {} free, {} per worktree", human(machine.disk_free.unwrap_or(0)), human(per_task_disk))),
        (by_cpu, format!("CPU: {} threads, {}", machine.cpus, kind.cpu_note())),
    ]
    .into_iter()
    .min_by_key(|(n, _)| *n)
    .map(|(n, why)| (n.clamp(1, 16), why))
    .unwrap_or((1, String::new()));

    let (open_tasks, waves_serial, waves_parallel) = waves(hub, &project.slug, parallel).await;
    let drawbacks = drawbacks(hub, &folder, &Facts {
        parallel,
        machine: &machine,
        total_memory,
        total_disk,
        peak_threads,
        kind,
        extra: &extra,
        setup,
        open_tasks,
        waves_serial,
        waves_parallel,
    });

    WorktreeEstimate {
        machine,
        parallel,
        checkout_bytes,
        tracked_files,
        extra,
        per_task_disk,
        agents,
        build_memory,
        build_note,
        per_task_memory,
        total_disk,
        total_memory,
        peak_threads,
        recommended_parallel,
        limited_by,
        open_tasks,
        waves_serial,
        waves_parallel,
        drawbacks,
    }
}

// ---- the machine

async fn machine(where_: &Path) -> MachineSpec {
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get() as u32);
    let (memory_total, memory_available) = memory().await;
    let mut dir = where_.to_path_buf();
    while !dir.exists() && dir.pop() {}
    MachineSpec { cpus, memory_total, memory_available, disk_free: disk_free(&dir), disk_path: dir.display().to_string() }
}

async fn memory() -> (u64, Option<u64>) {
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        let field = |name: &str| {
            text.lines().find(|l| l.starts_with(name)).and_then(|l| l.split_whitespace().nth(1)).and_then(|k| k.parse::<u64>().ok()).map(|k| k * 1024)
        };
        return (field("MemTotal:").unwrap_or(0), field("MemAvailable:"));
    }
    // macOS: the total from sysctl; what is free is not one number there.
    let total = tokio::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().await.ok().and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok());
    (total.unwrap_or(0), None)
}

fn disk_free(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // SAFETY: `path` is a valid C string and `stat` a zeroed out-parameter statvfs fills.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    #[allow(clippy::unnecessary_cast)]
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

// ---- the project

/// Bytes and number of the files git checks out.
async fn checkout(hub: &Hub, folder: &Path) -> (u64, u32) {
    let Some(out) = Git::new(hub, folder).text(&["ls-tree", "-r", "-l", "HEAD"]).await else { return (0, 0) };
    let mut bytes = 0;
    let mut files = 0;
    for line in out.lines() {
        let mut f = line.split_whitespace();
        if f.nth(1) == Some("blob") {
            files += 1;
            bytes += f.nth(1).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        }
    }
    (bytes, files)
}

/// The ignored build folders in the project folder (top level and one level down, for monorepos).
async fn rebuilt_folders(hub: &Hub, folder: &Path) -> Vec<SizedPath> {
    let mut found: Vec<String> = vec![];
    let mut look = |dir: &Path, prefix: &str| {
        for name in REBUILT {
            if dir.join(name).is_dir() {
                found.push(format!("{prefix}{name}"));
            }
        }
    };
    look(folder, "");
    if let Ok(entries) = std::fs::read_dir(folder) {
        for e in entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())) {
            let name = e.file_name().to_string_lossy().to_string();
            if !name.starts_with('.') && !REBUILT.contains(&name.as_str()) {
                look(&e.path(), &format!("{name}/"));
            }
        }
    }
    let git = Git::new(hub, folder);
    let mut out = vec![];
    for path in found.into_iter().take(12) {
        // Only what git ignores is missing from a fresh checkout.
        if git.run(&["check-ignore", "-q", &path], Duration::from_secs(5)).await.is_ok_and(|r| r.ok) {
            let bytes = size_of(folder.join(&path), Duration::from_secs(3)).await;
            out.push(SizedPath { path, bytes, copied: false });
        }
    }
    out
}

/// The size of a file or folder, counted for at most `limit` (a huge folder is undercounted rather than slow).
async fn size_of(path: PathBuf, limit: Duration) -> u64 {
    tokio::task::spawn_blocking(move || {
        let start = Instant::now();
        let mut total = 0;
        let mut stack = vec![path];
        while let Some(p) = stack.pop() {
            if start.elapsed() > limit {
                break;
            }
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if meta.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&p) {
                    stack.extend(entries.flatten().map(|e| e.path()));
                }
            } else {
                total += meta.len();
            }
        }
        total
    })
    .await
    .unwrap_or(0)
}

#[derive(Clone, Copy, PartialEq)]
enum BuildKind {
    Rust,
    Node,
    Go,
    Python,
    Other,
}

impl BuildKind {
    /// The heaviest build in the folder or one level down (a monorepo's `backend/Cargo.toml`).
    fn of(folder: &Path) -> BuildKind {
        let mut dirs = vec![folder.to_path_buf()];
        if let Ok(entries) = std::fs::read_dir(folder) {
            dirs.extend(entries.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir()) && !e.file_name().to_string_lossy().starts_with('.')).map(|e| e.path()));
        }
        dirs.iter().map(|d| Self::here(d)).max_by_key(|k| k.weight()).unwrap_or(BuildKind::Other)
    }

    fn weight(self) -> u8 {
        match self {
            BuildKind::Rust => 4,
            BuildKind::Node | BuildKind::Go => 3,
            BuildKind::Python => 2,
            BuildKind::Other => 0,
        }
    }

    fn here(folder: &Path) -> BuildKind {
        let has = |f: &str| folder.join(f).exists();
        if has("Cargo.toml") {
            BuildKind::Rust
        } else if has("package.json") {
            BuildKind::Node
        } else if has("go.mod") {
            BuildKind::Go
        } else if has("pyproject.toml") || has("requirements.txt") {
            BuildKind::Python
        } else {
            BuildKind::Other
        }
    }

    fn memory(self) -> u64 {
        match self {
            BuildKind::Rust => 2 * GIB,
            BuildKind::Node | BuildKind::Go => GIB,
            BuildKind::Python => 512 * MIB,
            BuildKind::Other => 256 * MIB,
        }
    }

    fn note(self) -> &'static str {
        match self {
            BuildKind::Rust => "Rust (Cargo.toml): a cargo build and test takes about 1–3 GB and every core it can get",
            BuildKind::Node => "Node/Bun (package.json): a build or test run takes about 0.5–1.5 GB",
            BuildKind::Go => "Go (go.mod): a build and test takes about 0.5–1.5 GB",
            BuildKind::Python => "Python: tests take about 0.2–1 GB",
            BuildKind::Other => "no known build file: assuming light checks",
        }
    }

    /// Threads one task's verify wants at its peak.
    fn threads(self, cpus: u32) -> u32 {
        match self {
            BuildKind::Rust | BuildKind::Go => cpus,
            BuildKind::Node => 2.min(cpus),
            _ => 1,
        }
    }

    /// Cores per task for the machine to stay responsive (agents mostly wait on the model; builds do not).
    fn cpu_cost(self) -> f64 {
        match self {
            BuildKind::Rust => 2.0,
            BuildKind::Go | BuildKind::Node => 1.0,
            _ => 0.5,
        }
    }

    fn cpu_note(self) -> &'static str {
        match self {
            BuildKind::Rust => "about 2 per task for the Rust builds",
            BuildKind::Go | BuildKind::Node => "about 1 per task for the builds",
            _ => "about half of one per task",
        }
    }
}

/// Memory of one CLI per provider: the average resident size of its running processes, else a typical figure.
async fn agent_memory(hub: &Hub, providers: &[String]) -> Vec<AgentMemory> {
    let ps = tokio::process::Command::new("ps").args(["-eo", "rss=,args="]).output().await.map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    providers
        .iter()
        .map(|provider| {
            let binary = hub.spec(provider).map(|s| s.binary.clone()).unwrap_or_else(|_| provider.clone());
            let name = Path::new(&binary).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(binary);
            let rss: Vec<u64> = ps
                .lines()
                .filter_map(|line| {
                    let mut f = line.split_whitespace();
                    let kb = f.next()?.parse::<u64>().ok()?;
                    // The CLI itself, or an interpreter running it (`node …/claude`).
                    let runs = f.take(2).any(|a| Path::new(a).file_name().is_some_and(|n| n.to_string_lossy() == name));
                    runs.then_some(kb * 1024)
                })
                .collect();
            if rss.is_empty() {
                let typical = match provider.as_str() {
                    "claude" => 450 * MIB,
                    "codex" => 200 * MIB,
                    "gemini" => 350 * MIB,
                    _ => 300 * MIB,
                };
                AgentMemory { provider: provider.clone(), bytes: typical, measured: 0 }
            } else {
                AgentMemory { provider: provider.clone(), bytes: rss.iter().sum::<u64>() / rss.len() as u64, measured: rss.len() as u32 }
            }
        })
        .collect()
}

/// The project's open tasks, and the rounds they take one at a time and `parallel` at a time (a task waits for
/// its open dependencies; finished ones count as done).
async fn waves(hub: &Hub, slug: &str, parallel: u32) -> (u32, u32, u32) {
    let tasks = hub.store.tasks().await.unwrap_or_default();
    let open: Vec<_> = tasks.iter().filter(|t| t.project.as_deref() == Some(slug) && !t.status.is_terminal() && t.status != TaskStatus::Review).collect();
    let n = open.len() as u32;
    let mut done: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let ids: std::collections::HashSet<i64> = open.iter().map(|t| t.id).collect();
    let mut rounds = 0;
    while done.len() < open.len() {
        let ready: Vec<i64> = open.iter().filter(|t| !done.contains(&t.id) && t.depends_on.iter().all(|d| !ids.contains(d) || done.contains(d))).map(|t| t.id).take(parallel as usize).collect();
        if ready.is_empty() {
            break; // a cycle: nothing more can start
        }
        done.extend(ready);
        rounds += 1;
    }
    (n, n, rounds)
}

// ---- what it costs

struct Facts<'a> {
    parallel: u32,
    machine: &'a MachineSpec,
    total_memory: u64,
    total_disk: u64,
    peak_threads: u32,
    kind: BuildKind,
    extra: &'a [SizedPath],
    setup: &'a [String],
    open_tasks: u32,
    waves_serial: u32,
    waves_parallel: u32,
}

fn drawbacks(hub: &Hub, folder: &Path, f: &Facts) -> Vec<Drawback> {
    let d = |level, title: &str, detail: String| Drawback { level, title: title.into(), detail };
    let n = f.parallel;
    let mut out = vec![];
    let room = f.machine.memory_available.unwrap_or(f.machine.memory_total);
    if f.total_memory > room {
        out.push(d(Level::Error, "Not enough memory", format!("{n} tasks at once want about {} but {} is free: the machine swaps or the kernel kills agents and builds. Run fewer at once.", human(f.total_memory), human(room))));
    }
    if let Some(free) = f.machine.disk_free
        && f.total_disk > free * 8 / 10
    {
        out.push(d(Level::Error, "Not enough disk", format!("{n} worktrees take about {} and {} is free in {}.", human(f.total_disk), human(free), f.machine.disk_path)));
    }
    if n > 1 {
        out.push(d(Level::Warning, "Merge conflicts cost a rerun", "Tasks land one after another. When two change the same lines, the later one is rebased, verified again and, on a conflict, sent back to an agent: more time and more tokens. Overlapping tasks gain little.".into()));
        out.push(d(Level::Warning, "Plan limits run out sooner", format!("{n} agents use your subscriptions at the same time, so the 5-hour and weekly limits are reached about {n}× sooner; MAP then moves tasks to the next provider or waits.")));
    }
    if f.peak_threads > f.machine.cpus && f.kind != BuildKind::Other {
        out.push(d(Level::Warning, "Builds compete for the CPU", format!("{n} verify runs at once want about {} threads on {} cores, so each build and test run is slower and the machine is busy while they run.", f.peak_threads, f.machine.cpus)));
    }
    let rebuilt: Vec<&str> = f.extra.iter().filter(|e| !e.copied).map(|e| e.path.as_str()).collect();
    if !rebuilt.is_empty() {
        let setup = if f.setup.is_empty() { " No setup command is set, so the agent or the first verify has to install them.".to_string() } else { String::new() };
        out.push(d(Level::Warning, "Every worktree starts cold", format!("A fresh worktree has none of {} (git ignores them): each task installs and builds from scratch, which takes time and disk.{setup}", rebuilt.join(", "))));
    }
    let services = ["docker-compose.yml", "docker-compose.yaml", "compose.yml", "compose.yaml", "Procfile"].iter().any(|f| folder.join(f).exists());
    if n > 1 {
        let detail = if services {
            "This project starts services (compose file found): two tasks that start them collide on ports, container names and the same database."
        } else {
            "Dev servers, databases and other services two tasks start at once collide on ports and shared data; only files are separate."
        };
        out.push(d(if services { Level::Warning } else { Level::Info }, "Ports and services are shared", detail.into()));
    }
    if f.open_tasks > 0 && f.waves_parallel >= f.waves_serial && n > 1 {
        out.push(d(Level::Info, "No gain for the open tasks", format!("The {} open tasks depend on each other in a chain, so they still run one after another.", f.open_tasks)));
    } else if f.open_tasks > 1 {
        out.push(d(Level::Info, "Dependencies still run in order", format!("The {} open tasks take {} rounds one at a time and {} rounds with {n} at once; only independent tasks run together.", f.open_tasks, f.waves_serial, f.waves_parallel)));
    }
    out.push(d(Level::Info, "Untracked files are not there", "Files git does not track (.env, local config, data) are missing in a worktree unless listed under copy; copied files are never committed.".into()));
    let max = hub.config.orchestrator.max_parallel as u32;
    if n > max {
        out.push(d(Level::Info, "MAP runs fewer at once today", format!("[orchestrator] max_parallel = {max} in map.toml caps all tasks; set it to {n} and restart MAP to run {n} at once.")));
    }
    out
}

pub fn human(bytes: u64) -> String {
    let b = bytes as f64;
    if bytes >= GIB {
        format!("{:.1} GB", b / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.0} MB", b / MIB as f64)
    } else {
        format!("{:.0} KB", b / 1024.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_as_people_say_them() {
        assert_eq!(human(3 * GIB / 2), "1.5 GB");
        assert_eq!(human(300 * MIB), "300 MB");
        assert_eq!(human(2048), "2 KB");
    }

    #[test]
    fn build_kind_follows_the_build_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(BuildKind::of(dir.path()) == BuildKind::Other);
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        assert!(BuildKind::of(dir.path()) == BuildKind::Node);
        std::fs::create_dir(dir.path().join("backend")).unwrap();
        std::fs::write(dir.path().join("backend/Cargo.toml"), "").unwrap();
        assert!(BuildKind::of(dir.path()) == BuildKind::Rust, "one level down counts, the heaviest wins");
    }

    #[tokio::test]
    async fn the_machine_is_read() {
        let m = machine(Path::new("/tmp/does/not/exist")).await;
        assert!(m.cpus >= 1 && m.memory_total > 0);
        assert!(m.disk_free.is_some_and(|f| f > 0), "{m:?}");
    }
}
