# Multi-Agent Platform

Run, watch and orchestrate coding-agent CLIs (Claude Code, Codex, Gemini/`agy`, or any terminal agent) from one
dashboard. Each agent runs in its own tmux session, so you can always `tmux attach` and take over by hand.

It grew out of two local tools: the Anti-Gravity session watcher (rate-limit detection and automatic wake-up) and the
Nutto task orchestrator (dependency-ordered tasks, verification, an LLM audit, commit). This project generalizes both:
nothing is tied to one repository, session name or provider.

## What it does

- **Sessions.** Start an agent from the dashboard or the API with a provider, a model and an optional first prompt.
  Watch its live terminal, type to it, send keys (Ctrl-C, Esc, y/n…), and close it.
- **Watcher.** Every few seconds it reads each managed pane and sets a state: `working`, `idle`, `waiting_input`,
  `rate_limited`, `needs_auth`, `stalled`, `error`, `exited`.
  - **Rate limits.** It parses the reset time from the provider's message ("try again in 1h 5m", "resets 3pm",
    Google `RetryInfo`, ISO timestamps). At that time it types the wake prompt, but never while a person is typing.
  - **Repeats.** Only new limit messages count, so an old error left in the scrollback does not trigger again.
- **Orchestrator.** Tasks have dependencies. When the orchestrator is running, it starts each ready task on a worker
  session, choosing a provider from `provider_order` and skipping any that are missing, rate limited or signed out.
  - **Completion.** The worker reports by writing `$MAP_RESULT_FILE`.
  - **Checks.** The orchestrator then runs your `verify` commands, then an optional audit by another agent
    (`VERDICT: PASS`), then `post_done` hooks (e.g. commit).
  - **Retries.** A failed check retries the task with the failure output in the new prompt, up to `max_attempts`.
  - **Human safety.** It never kills a worker that someone is attached to, or one with typing on its input line.
- **Projects.** A project is a folder on the machine MAP is installed on, a corpus repo and its own
  [Reflex](https://reflex.gist.rs) decision engine.
  - **Folder.** Tasks and sessions of a project run in its folder, in place: an existing folder (a git repo or not),
    a new empty one MAP creates, or a new folder MAP clones a repo into (see [Git access](#git-access)). MAP never
    copies or hosts a project's code, runs git there only when you ask (clone, fetch, pull, push), and adds no files
    to it unless you init it with a template (see [Init a project](#init-a-project)); deleting a project leaves the
    folder alone. Two projects cannot share a folder or nest.
  - **Corpus.** A bare git repo of the platform's own, served at `/git/<slug>/corpus.git`: clone, push, or edit
    files from the dashboard.
  - **Decide before calling an LLM.** `POST /api/projects/<slug>/decide` asks Reflex first (local, no model, a few
    milliseconds). Answers at or above the project's confidence threshold are final; only the rest go to the
    project's fallback: the Anthropic API, or any provider run headless (`claude -p`, `codex exec`, `agy -p`).
    Agents started in a project get the endpoint as `$MAP_DECIDE_URL`, and their prompt tells them to use it.
  - **Corpus content.** One top-level folder per Reflex domain (1–8), prose `.md` files inside. A push or an edit
    restarts the project's Reflex on the new corpus.
  - **Create.** Creating a project returns check lines: the folder used (its git branch and commits) or created,
    the corpus repo, Reflex started or why not, each provider found and signed in.
  - **Settings.** Each project can set its own provider order (with a model and a session limit per provider), verify
    commands and audit (on, off, or a given auditor), and can be paused so the orchestrator starts none of its pending
    tasks. Unset fields fall back to `[orchestrator]` in `map.toml`. They live in the database, not in a file.
  - **Calibration.** The threshold is fitted from labeled cases (the ρ-quantile of Reflex's confidence on the cases
    it got right), and Reflex's own calibrator learns from `feedback`. Every decision is logged with its lane.
- **Sign-in.** Each provider shows whether it is signed in, from its credential files (`credentials` in its spec: only
  whether they exist and parse is checked, never their contents) or its API key variable (`api_key_env`). The sign-in
  modal runs the provider's own `login` command (`claude auth login`, `codex login --device-auth`, `agy` + `/login`)
  in a helper session `<session_prefix>login-<provider>`, and shows the URL and one-time code from its pane. Note that
  `codex login` signs the current account out as soon as it starts. API keys are never taken through the dashboard:
  set them in `[workspace.env]`.
- **Usage.** Plan limits per provider account, on the Usage screen and in a pill on every screen's top bar
  (`[usage]` in `map.toml`, polled every `interval_secs`).
  - **Claude**: the account endpoint the CLI's `/usage` reads, with Claude Code's own sign-in from
    `claude_credentials`. The token is sent only to api.anthropic.com; it is never logged, stored or returned.
  - **Codex**: the newest `rate_limits` snapshot in its session logs, shown with its age (and "signed out" when
    `codex_auth` is missing). Other CLIs (`agy`) have no local source, so they show "no reader" rather than a guess.
  - **Tokens** today and by project (from each session's working directory), from the CLIs' session logs.
  - **Pace.** Readings are stored, and the last hour's rate projects the use at reset.
  - **Skip line.** No new tasks for a provider while any of its windows is at or above `skip_pct` (default 90),
    until that window resets. It shows as the provider's cooldown.
    it got right), and Reflex's own calibrator learns from `feedback`. Every decision is logged with its lane and a
    one-line `reason` (threshold comparison, shadow mode, Thai input…). Cases are stored per project and scored by
    Reflex as they are added; marking a logged decision right or wrong adds it as a case and tells Reflex's calibrator
    whether Reflex's own answer was right.
  - **Not calibrated** (no threshold): shadow mode. Reflex scores every question, but the fallback answers.
  - **Thai input** (any character in U+0E00–U+0E7F) is never sent to Reflex, which tokenises English only: it goes
    straight to the fallback, with the reason in the answer.
  - **More than 8 domains** pauses Reflex. Archiving moves domains to `_archive/` in the corpus repo (one commit),
    which Reflex never loads; the dashboard's Reflex screen offers to archive the smallest.
- **Updates.** A bell in the top bar counts the agent CLIs with a newer release (`[updates]` in `map.toml`, checked
  every `interval_secs`, 6 hours by default). Each provider's `version` command (`claude --version`…, run with the
  workspace env, no stdin and a timeout) is compared with the version its `latest` source publishes (Claude's release
  channel from `autoUpdatesChannel`, npm for Codex, agy's update manifest for `{os}_{arch}`), fetched without
  credentials. A new release is announced once (an `update.available` event and a toast); the versions also show on
  the Usage screen. **Update** shows the exact command, then runs the provider's `update` command in a helper session
  `<session_prefix>update-<provider>` whose console you can watch; the version is checked again when it ends. It is
  refused while one runs, and when the CLI lives where this user cannot write (the Docker deploy mounts the CLIs
  read-only: update them on the host). Providers without these fields show "unknown".
- **History.** Sessions, tasks and events live in SQLite. Every run keeps its prompt, launch script, result file and
  final pane under `.map/runs/<session>/`.

## Architecture

```
┌──────────────── map (one Rust binary, one port) ────────────────┐
│ axum: /api/* REST · /api/ws push · static dashboard (frontend/out) │
│                                                                   │
│  Watcher loop ──┐        ┌── Orchestrator loop                    │
│                 ▼        ▼                                        │
│            Hub (state, events, broadcast) ── SQLite (.map/map.db) │
│                 │                                                 │
│        Terminal trait ── tmux (send-keys -l, capture-pane -e)     │
└─────────────────┼─────────────────────────────────────────────────┘
                  ▼
     map-task-12-1 (claude)   map-reviewer (codex)   map-plan (agy) …
```

| Path | Role |
| --- | --- |
| `backend/src/detect.rs` | Pane parsing: rate limits and reset times, auth prompts, pending human input, last agent message |
| `backend/src/watcher.rs` | Per-session state machine and automatic wake-up |
| `backend/src/orchestrator.rs` | Task scheduling, provider failover, verify, audit, retry |
| `backend/src/hub.rs` | Launching and closing sessions; the single place that writes state and broadcasts it |
| `backend/src/terminal/` | `Terminal` trait; `tmux.rs` in production, `fake.rs` in tests |
| `backend/src/projects.rs` | Creating projects on a folder (existing or new) and deleting them; folder and corpus path rules |
| `backend/src/templates.rs` | Project templates (built-in and `[templates] dirs`), the create/skip/append plan, laying one down, the init agent's task |
| `backend/src/project_settings.rs` | Per-project providers, verify, audit and pause, as the orchestrator asks for them |
| `backend/src/login.rs` | Provider sign-in state from credential files, the helper login session, URL and code parsing |
| `backend/src/repo.rs` | The corpus repo written with git plumbing, corpus export, a read-only look at a project's folder |
| `backend/src/git_access.rs` | MAP's SSH key, pinned known_hosts, git identity, the gitconfig agents get as `GIT_CONFIG_GLOBAL`, URL and host checks |
| `backend/src/project_git.rs` | Git in a project's folder: status, fetch, ff-only pull, push, connect a remote, clone, ls-remote |
| `backend/src/reflex.rs` | One Reflex process per project: export the corpus, start, health-check, restart |
| `backend/src/usage.rs` | Plan-usage readers (Claude endpoint, Codex logs), token scans, pace, the skip line |
| `backend/src/updates.rs` | CLI update checks (version command vs. latest source), update helper sessions |
| `backend/src/skills.rs`, `agents.rs` | The Skills screen's scan; custom sub-agents read and written in each CLI's format |
| `backend/src/decide.rs` | The decision gate (Reflex, threshold, fallback), answer parsing, threshold fitting |
| `backend/src/api/git.rs` | Git smart HTTP through `git http-backend`; a corpus push restarts Reflex |
| `backend/src/api/` | REST and WebSocket, token, Host and Origin checks |
| `backend/src/model.rs` | Shared types, exported to TypeScript by ts-rs |
| `frontend/` | Next.js dashboard (Bun), built as a static export |

### Why the frontend is Next.js on Bun, not Rust

The backend is Rust for a long-running, low-overhead supervisor with strong types. The dashboard is a different job,
and Next.js + Bun fits it better:

- **Ecosystem.** Terminal views, flow diagrams (the watcher's xyflow graph), and component kits like shadcn have no
  equal in Leptos, Dioxus or Yew.
- **Continuity.** The watcher's dashboard is already Next 16 + Bun, so its components and the team's habits carry over.
- **Type safety.** ts-rs generates `frontend/lib/generated/*.ts` from the Rust structs on every `cargo test`, and CI
  fails if they drift.
- **Deployment.** `next build` produces a static export that the Rust binary serves itself: one process and one port,
  without the watcher's second Node server and proxy.

## Quick start

Requirements: Rust 1.85+, Bun 1.2+, tmux, git, and the agent CLIs you want (`claude`, `codex`, `agy`). For projects'
decision engines, install Reflex (`curl -fsSL https://raw.githubusercontent.com/gist-rs/reflex/main/install.sh | sh`);
without it, projects still work and every decision goes to the fallback.

```sh
git clone git@github.com:spinater/multi-agent-platform.git && cd multi-agent-platform
cp map.example.toml map.toml        # set workspace.dir to the repository the agents should work in

(cd frontend && bun install && bun run build)
cd backend && cargo run --release -- check -c ../map.toml   # shows which agent CLIs are found
cargo run --release -- serve -c ../map.toml
# open http://127.0.0.1:4400
```

## Install as a service

On the machine whose projects the agents should work on (Linux or macOS), from a checkout:

```sh
deploy/install.sh                 # --port 4400 --bind 127.0.0.1 --prefix DIR --config FILE --no-service
deploy/install.sh --uninstall     # removes the service; keeps the config and data
```

It builds the server and dashboard, installs them under `~/.local/share/map`, adds a pinned Reflex (checked by
SHA256) unless one is on PATH, writes `~/.config/map/map.toml` once with a fresh `api_token` and your current PATH
(so the service finds the same `claude`, `codex`, `agy`), and starts a systemd user service (`map.service`) or a
launchd agent (`com.multi-agent-platform.map`). Restarting the service leaves agent sessions in tmux running.
Requirements: Rust, Bun, tmux, git, curl, openssl. On Linux, `sudo loginctl enable-linger $USER` keeps it running
after you log out.

Development, with hot reload for the dashboard:

```sh
cd backend && cargo run -- serve -c ../map.toml      # API on :4400
cd frontend && bun dev                               # dashboard on :3000, /api proxied to :4400
```

## Deploy with Docker Compose

One container runs the server, the dashboard, tmux, git and a pinned Reflex. The agent CLIs and their sign-ins stay
on the host and are mounted at the same paths, so the container and your shell share one login per account (a
token refresh on one side never logs out the other). It publishes on `127.0.0.1` only; put a tunnel or an
authenticating proxy in front to reach it from elsewhere.

```sh
cp .env.example .env                        # your uid/gid, user, home, Node dir, MAP_DIR, projects dir, port
mkdir -p ~/map-deploy/data/map && git init ~/map-deploy/data/workspace
cp deploy/map.docker.toml ~/map-deploy/map.toml
#   set api_token (openssl rand -hex 32), the PATH in [workspace.env], and allowed_hosts for your tunnel's name
docker compose up -d --build
docker compose logs -f map                  # open http://127.0.0.1:4400 and paste the token once
```

- Mounted from the host: `~/.claude`, `~/.codex`, `~/.gemini` (read-write: sign-ins, history, transcripts for Usage
  and Skills, user-wide agents) and the CLIs in `~/.local/bin`, `~/.local/share/claude` and the Node install
  (read-only). Adjust `docker-compose.yml` if yours live elsewhere. Because the CLIs are read-only there, the update
  bell still shows new releases but its Update button is disabled: update them on the host.
- Claude Code in the container uses `CLAUDE_CONFIG_DIR=~/.claude`, so its settings file is `~/.claude/.claude.json`
  (the host rewrites `~/.claude.json` by renaming over it, which a single-file mount cannot follow). Set
  `"hasCompletedOnboarding": true` in that file once, or the first interactive session stops at the setup screens.
  Each new folder still asks "trust this folder?" once; answer it in the session console.
- Projects are folders under `MAP_PROJECTS_DIR`, mounted at the same path, so a project's path is the same in the
  container and on the host; set `[projects] root` to it so "Clone from git" puts new projects there. State lives in
  `MAP_DIR/data` (database, run files, corpus repos, and MAP's git key and gitconfig under `data/map/git`); config in
  `MAP_DIR/map.toml`. Git needs no `~/.ssh` mount: create MAP's key under Projects → Git access and add it to
  GitHub or GitLab (see [Git access](#git-access)).
- On macOS, use `deploy/install.sh` instead: a Linux container cannot run the Mac's agent CLIs.
- Agent sessions are tmux sessions inside the container, so `docker compose restart` ends them (the watcher marks
  them exited and the orchestrator retries their tasks).
- **Cloudflare tunnel**: point it at `http://127.0.0.1:4400`, add the public host name to `allowed_hosts`, keep
  `api_token` set, and preferably put Cloudflare Access in front.

## Dashboard

A dark operator console. The **command centre** watches and steers the orchestrator for the current project (or
all projects, read-only):

- **Status bar:** core running/paused, watcher interval, WebSocket, database, workspace.
- **Left:** orchestrator state with Run/Pause, providers in `provider_order` with their state (ready, rate limited,
  cooldown, missing, signed out with a Sign in button), and the task queue with "+ New task".
- **Centre:** a canvas with two views. **3D orbit**: drag to orbit, wheel to zoom. **Graph**: columns ORCH ·
  PROVIDERS · SESSIONS · TASKS by dependency depth · PIPELINE (each `verify` command, the audit, each `post_done`
  hook, from `GET /api/pipeline`). ◇ task, ○ session, ⬡ stage, • run (one dot per recent event of a session).
  Active links carry moving dots. Provider chips isolate a provider, and type toggles hide node kinds.
- **Card:** clicking a node opens a card with its facts and real actions: retry, cancel or delete a task; type to a
  live session, send keys, open its full console, close it; sign in; start a session; pause the orchestrator.
- **Right:** "Needs you" (sessions waiting for input, signed out, stalled or in error, and failed tasks), sessions
  with "+ New", and the event log.
- **All projects:** the providers column becomes one hub per project, and IDs read `<project> #<id>`. The left
  column lists projects with "Open →", and cards offer only "Open in <project> →".

Deep links: `#command/tasks`, `#command/tasks/<id>`, `#command/tasks/new`, `#command/sessions/<name>`,
`#command/sessions/new`, `#projects/<slug>`, `#projects/init/<slug>`. Every layout number lives in `TUNING` in
`frontend/components/graph/layouts.ts`.

### Skills (`#skills`, `#skills/<name>`)

What each agent can load in the current project's workspace (`backend/src/skills.rs`; locations in `[skills]` of
`map.toml`, all read only):

- **Context files.** `CLAUDE.md`, `AGENTS.md`, `GEMINI.md`: line count, first prose lines, `@imports`.
- **Skills.** `<workspace>/.claude/skills` and `~/.claude/skills` (including claude.ai-synced skills, invoked as
  `anthropic-skills:<name>`), installed Claude Code plugins (every `.claude-plugin/plugin.json` below
  `~/.claude/plugins`; marketplace catalogs count only for plugins `enabledPlugins` turns on), and Codex skills
  (`<workspace>/.agents/skills`, `~/.codex/skills`). Claude loads its own natively; Codex or Gemini are listed for
  a Claude skill only when their context file points at it ("via AGENTS.md"), since they read it only if they follow
  that pointer.
- **Enable/disable** is Claude Code's own mechanism, per workspace: MAP edits `<workspace>/.claude/settings.local.json`
  (the file Claude's `/skills` menu writes), adding a `Skill(<name>)` rule to `permissions.deny` and, for
  non-plugin skills, `skillOverrides: {"<name>": "off"}`; enabling removes them. Other keys are kept, and the file is
  added to the workspace's `.git/info/exclude` so a `post_done` commit does not pick it up. A skill turned off in
  `~/.claude/settings.json` or a committed `.claude/settings.json` is shown as off but not switchable here; Codex
  skills have no per-project switch.
- **Use.** Loads are `Skill` tool calls and typed `/name` commands in Claude Code's transcripts for the workspace
  (`~/.claude/projects/<path with non-alphanumerics as ->/*.jsonl`). A transcript belongs to a MAP task when its
  session name (MAP starts Claude with `--name <tmux session>`) is a run of that task, or, for unnamed sessions, when
  exactly one Claude run of the project was alive at the time. Codex and Gemini loads are not recorded anywhere MAP
  can read, so their skills show "use not recorded".
- **Run as task** queues a task in the current project with an editable prompt that loads the skill.

### Agents (`#agents`, `#agents/<scope>/<name>`, `#agents/new`)

Custom sub-agents for the current project and user-wide, in each CLI's own format (`backend/src/agents.rs`;
folders in `[agents]` of `map.toml`):

| CLI | Format | Project | User |
| --- | --- | --- | --- |
| Claude Code | Markdown, YAML header, body = prompt | `.claude/agents/**/*.md` | `~/.claude/agents` |
| Codex | TOML (`developer_instructions`) | `.codex/agents/*.toml` | `~/.codex/agents` |
| agy (`gemini`) | Markdown, YAML header, body = prompt | `.agents/agents/<name>.md`, `<name>/agent.md` | `~/.gemini/config/agents` |

- **Matrix.** The CLIs find agents by `name`, not file name, so files are grouped by name: one row per agent and
  scope, a column per CLI (✓ with its model, ! a file the CLI may not load, — none). Files that name no agent are
  listed apart.
- **Editor.** Name, description, instructions, capabilities (read, search, edit, shell, web, or all tools), a model
  per CLI, effort, and the CLIs to write for, with a live preview of every file a save writes or removes.
  Capabilities become Claude tools (`Read`, `Grep`, `Glob`, `Edit`, `Write`, `Bash`, `WebFetch`, `WebSearch`) and agy
  tools from a fixed table (a misspelled agy tool can hang it). Codex has no tool list: no shell writes
  `[features] shell_tool = false`, and read-only adds "Do not modify files." to its instructions. Effort is
  `effort` for Claude, `model_reasoning_effort` for Codex (`max` → `xhigh`) and dropped for agy. Each lossy step is
  shown as a note.
- **Saving** rewrites the file the scanner found for the agent, or writes `<name>.md` / `<name>.toml` in the first
  configured folder; it keeps keys MAP does not edit (`permissionMode`, `mcpServers`, `sandbox_mode`…), removes the
  files of CLIs dropped from the targets, writes through a temporary file and a rename, and refuses to replace a
  file that holds another agent. Nothing is written outside the configured folders. Run one as the main agent with
  `claude --agent <name>` or `agy --agent <name>`.

The **Reflex** screen (`#reflex`, `frontend/components/reflex/`) is the current project's decision engine: how it
works with the last 24 h per lane, a set-up checklist that ticks itself (install, corpus, questions, calibration,
playground), a playground with feedback, calibration on a log-scale scatter with a draggable threshold, and the
decision log. Confidence is always shown on a log axis (1e-5 … 1e-3, widened by whole decades when a corpus scores
outside it), never as a percentage.

## Git access

One git setup for every project, so agents can clone, pull and push GitHub, GitLab or any ssh git host
(`backend/src/git_access.rs`, `project_git.rs`; **Git access** on the Projects screen).

- **MAP's SSH key.** "Create key" runs `ssh-keygen -t ed25519 -N "" -C map@<hostname>` into
  `<data_dir>/git/id_ed25519` (folder 0700, key 0600). Only the public key and its `SHA256:` fingerprint are ever
  shown or returned; the private key is never read, logged or sent. "Replace key" moves the old pair aside to
  `id_ed25519.old-<time>` instead of deleting it. Add the public key on GitHub (Settings → SSH and GPG keys, or a
  repo's Deploy keys with "Allow write access") or GitLab (Preferences → SSH Keys, or Deploy keys with write
  permission), then **Test** a host: `ssh -T git@<host>` with MAP's key alone (GitHub's "successfully
  authenticated", exit 1, and GitLab's "Welcome to GitLab" count as success). The dashboard never takes a private
  key, token or password; https URLs with a password in them are refused.
- **Host keys.** `<data_dir>/git/known_hosts` pins GitHub's and GitLab's published host keys (a constant in
  `git_access.rs`, with the source pages and date). Other hosts are trusted on first use
  (`StrictHostKeyChecking=accept-new`), and the dialog shows the fingerprint a test recorded, to compare with the
  host's own.
- **Identity.** A name and email for agents' commits (`<data_dir>/git/identity.json`), optional.
- **`GIT_CONFIG_GLOBAL`.** MAP writes `<data_dir>/git/gitconfig`, regenerated on every change: its defaults first
  (`[user]` when set, `[core] sshCommand = ssh -i <key> -o IdentitiesOnly=yes -o UserKnownHostsFile=<known_hosts>
  -o StrictHostKeyChecking=accept-new …` when the key exists), then `[include]` of your `~/.gitconfig` and
  `~/.config/git/config` when they exist, so your own settings win; a repo's own config wins over both. Every agent's
  launch script exports `GIT_CONFIG_GLOBAL` (a value in `[workspace.env]` wins), verify and `post_done` commands
  get it too, and MAP's own git commands in project folders use it, with `GIT_TERMINAL_PROMPT=0`, ssh `BatchMode`,
  no controlling terminal, only https/http/ssh/git/file transports, and a timeout (`[projects] git_timeout_secs`).
- **Docker.** The key lives in `MAP_DIR/data`, so it works in the container without mounting `~/.ssh`.
- **Clone from git.** The wizard's third source: a repo URL (`git@host:owner/repo.git`, `ssh://`, `https://`,
  `git://`, `file://`; never an option or an `ext::`/`fd::` transport), an optional branch, and the folder, by default
  `<[projects] root>/<repo name>` (missing or empty). It shows `git ls-remote` first (default branch, branches, or why
  the host refused, with a hint to add MAP's key), then runs `git clone` (a normal working clone, not bare). The
  project is then an ordinary folder project (`kind = "local"`), with the URL kept as `source.remote`.
- **Sync.** A project's page has a Git panel (and its card a one-line status): branch, upstream, origin, ahead/behind,
  changed files, last commit and fetch, and **Fetch**, **Pull** (fast-forward only; refused while the folder has
  changes or the branch diverged), **Push** (the current branch, never forced; the first push sets the upstream with
  `-u origin <branch>`) and **Connect a remote** (sets or replaces `origin`, after `git init -b main` when the folder
  is not a repo yet). Each shows its commands and output and logs a `project.git.*` event. Pull, push and remote
  changes are refused while a task of the project is running; fetch and status are not.

## Init a project

"Init" on a project (`#projects/init/<slug>`, also offered right after the wizard creates one) lays a development
process into its folder and has an init agent fill in the project-specific parts.

- **Template.** A folder with a `template.toml` (`name`, `title`, `description`, `fill`: files with `MAP:FILL`
  placeholders, `verify`: suggested verify commands, `append`, `rename`, `agents`) and the files to lay down.
  `team-process` is built in (`backend/templates/team-process/`): `CLAUDE.md` with the process (task cards, closing a
  card, the gate, reviewers by the cost of being wrong, questions with scenarios, docs, status file) and placeholders
  for what the project is, read-before-touching, invariants, stack and commands, deploy and the review path list;
  `AGENTS.md`/`GEMINI.md` pointing back to it; `tasks/{todo,todo-human,done}/` with the card template in
  `tasks/README.md`; `.docs/{knowledge/domain,knowledge/ops,design,roadmap,runbook}/`; `scripts/verify.sh` (ALL GREEN
  or RED, loud skips) and `scripts/task-move.sh`; `/.scratch/` and `/.env` for `.gitignore`; and five agents
  (code-reviewer, security-reviewer, architect, qa-tester, sa-requirements), written once and rendered as
  `.claude/agents/*.md`, `.codex/agents/*.toml` and `.agents/agents/*.md`. More templates come from `[templates] dirs`.
- **Never overwrites.** The plan shows each file as create, skip (it exists) or append (only the missing lines of
  `.gitignore`). Files are opened create-new, and nothing is written through a symlink.
- **Init agent.** One definition for the whole platform (`backend/templates/init-agent.md`, or `[templates]
  init_agent`), run with the provider and model you pick. It is queued as the task "Init project" in the project,
  pinned to that provider, so it gets the protocol footer, the verify commands (the template's, when you tick "use
  the template's verify commands"), the audit and retries like any task. Its prompt carries your description, the
  files laid down and the placeholders left. It fills every placeholder, makes `scripts/verify.sh` run the stack's
  real checks, sets up a skeleton only in a folder that was empty and only when the description asks for one, writes
  the first cards in `tasks/todo/` from 001, never rewrites existing files, and does not commit: you review first.

## Writing tasks

A task is a title and a prompt. The orchestrator appends a protocol footer that tells the worker:

- its session name and task number;
- to write `{"status": "done", "summary": "…"}` to `$MAP_RESULT_FILE` when finished, or `"failed"` with a reason;
- which verify commands will run, so it runs them first;
- on a retry, the output of the check that failed.

Pin a task to a provider (and model) or leave it open for failover. Tasks with `depends_on` wait until those tasks
are `done`. If a dependency fails or is cancelled, its dependents show as blocked.

## Orchestrator decisions

Every branch the orchestrator takes for a task is an event `decision.<node>.<outcome>` with `task_id` set and the
detail it looked at as the message (`pinned: claude → claude`, `no dependencies · slot 2/4`,
`` `bun test` failed · 1 of 3 · 2 failing ``). The Decisions screen (`#decisions`, `#decisions/<task id>`) draws them as
a graph; `backend/src/decisions.rs` holds the kinds, the edge each takes and the rule text. A decision identical to
one of the task's last two is not logged again, so a task parked at a branch does not log every tick.

| Kind | Edge | Emitted in |
| --- | --- | --- |
| `decision.ready.go` | ready → pick | `tick`: dependencies done, slot free |
| `decision.ready.blocked` | ready → blocked | `tick`: a dependency failed or was cancelled |
| `decision.pick.chosen` / `.none` | pick → launch / stays | `start` (`pick_provider`); `none` while no provider is available |
| `decision.launch.started` / `.error` | launch → watch / retry | `start` (`hub.launch`) |
| `decision.watch.done` | watch → verify | `check_running`: result file says done |
| `decision.watch.failed` / `.exited` / `.auth` / `.timeout` / `.lost` | watch → retry | `check_running` |
| `decision.verify.cmd_start` / `.cmd_pass` | stays (progress) | `finish`: each `verify` command, `` `cmd` · 2 of 4 `` |
| `decision.verify.pass` / `.fail` | verify → audit / retry | `finish` |
| `decision.audit.off` / `.pass` / `.skipped` | audit → post | `finish` (`audit.enabled`, verdict, no auditor and not required) |
| `decision.audit.fail` / `.no_auditor` | audit → retry | `finish` |
| `decision.audit.ask` / `.unavailable` | stays (progress) | `audit`: each auditor of `audit.chain` |
| `decision.post.cmd_pass` | stays (progress) | `finish`: each `post_done` hook |
| `decision.post.pass` / `.fail` | post → end / failed | `finish` |
| `decision.end.closed` / `.kept` | end → done | `finish` (`detect::end_decision` via `close_session`) |
| `decision.retry.again` / `.failed` / `.kept` | retry → ready / failed / watch | `retry_or_fail`, and `start` after a launch error |

The older `task.*` events (`task.started`, `task.retry`, `task.done`…) are still sent.

## API

All routes are under `/api`. When `server.api_token` is set, send `Authorization: Bearer <token>`.

| Method | Path | |
| --- | --- | --- |
| GET | `/overview` | Sessions, tasks, providers, orchestrator status and server info (incl. `reflex_version`) in one call |
| GET | `/events?limit=&session=` | Event log, newest first |
| GET/POST | `/sessions` | List, or start (`provider`, `model`, `name`, `prompt`, `supervisor`) |
| DELETE | `/sessions/{name}?force=` | Close a live session (409 while a person is using it), or remove an exited one |
| GET | `/sessions/{name}/pane?lines=` | Pane text (the saved final pane once exited) |
| POST | `/sessions/{name}/prompt` | Type text and press Enter (`{"text": …}`) |
| POST | `/sessions/{name}/key` | One allow-listed key (`C-c`, `Escape`, `Enter`, `Up`, `Down`, `Tab`, `BTab`, `y`, `n`) |
| GET/POST | `/tasks` | List, or create (`title`, `prompt`, `depends_on`, `provider`, `model`) |
| PATCH | `/tasks/{id}` | `{"action": "retry"}` or `{"action": "cancel"}` |
| DELETE | `/tasks/{id}` | Delete a pending or finished task |
| GET/POST | `/orchestrator` | Status, or `{"running": true}` |
| GET | `/pipeline` | `provider_order`, `verify`, audit, `post_done` and retry settings from `[orchestrator]` |
| GET/POST | `/projects` | List, or create (`name`, `slug`, `description`, `fallback`, `model`, `settings`); returns the project and `checks` |
| GET/PATCH/DELETE | `/projects/{slug}` | Info (settings, Reflex status, decision counts), change (`threshold`, `fallback`, `model`, `questions`, `settings`, `paused`…), delete |
| GET | `/projects/defaults` | The `map.toml` values projects inherit (provider order, verify, audit, max_parallel, post_done) |
| GET | `/templates` | Templates with their files and placeholders (built in, then `[templates] dirs`) |
| POST | `/projects/{slug}/init/plan` | `{"template"}`: per file `create`, `skip` (exists) or `append` (missing lines); writes nothing |
| POST | `/projects/{slug}/init` | `{"template", "description", "provider", "model"?, "set_verify"}`: writes the plan, optionally sets the project's verify to the template's, queues the "Init project" task; returns `plan`, `checks` and `task` |
| POST | `/projects/inspect` | `{"kind": "local"\|"empty", "location"}`: the folder's git branch, branches, commits and top-level entries, or whether a new folder can go there; `{"kind": "clone", "location": <url>, "branch"?}`: `git ls-remote` (default branch, branches, `hint` on an auth failure, suggested `folder`) |
| GET | `/git/access` | MAP's public key and fingerprint (or `null`), git identity, known hosts, gitconfig path and includes, projects root |
| POST | `/git/key` | `{"replace"?: bool}`: create MAP's ed25519 key (409 when one exists without `replace`; the old one is moved aside) |
| POST | `/git/test` | `{"host"}` (`github.com`, `git@host:2222`): `ssh -T` with MAP's key; `ok`, output, the host's recorded keys |
| PUT | `/git/identity` | `{"name", "email"}` (both empty: none) |
| GET | `/projects/{slug}/git` | The folder's `is_repo`, branch, upstream, origin, ahead/behind, dirty count, last fetch and commit, `busy` |
| POST | `/projects/{slug}/git/fetch`, `…/pull`, `…/push` | `{}`: run it; returns `ok`, commands, output, new status (409 with the reason when refused) |
| POST | `/projects/{slug}/git/remote` | `{"url"}`: set or replace `origin` (`git init -b main` first when not a repo) |
| GET/POST | `/providers/{name}/login` | Sign-in state (parsed URL, one-time code, credential check), or start the helper login session |
| GET | `/projects/{slug}/repos/corpus` | Files and recent commits |
| GET/PUT | `/projects/{slug}/repos/{repo}/file` | `?path=` read; `{"path", "content", "message"}` commit (`content: null` deletes) |
| POST | `/projects/{slug}/decide` | `{"state", "questions", "route": "auto"\|"reflex"\|"llm"}`; each answer has `lane` |
| POST | `/projects/{slug}/feedback` | `{"p": <Reflex's confidence>, "outcome": bool}` for Reflex's calibrator |
| POST | `/projects/{slug}/fit` | Labeled `cases`, `rho`, `apply`: fit the threshold |
| GET | `/projects/{slug}/decisions?limit=` | Decision log, newest first, with each decision's `feedback` |
| GET | `/projects/{slug}/decisions/summary?hours=24` | Count and average ms per lane, feedback ✓/✕, hourly buckets |
| POST | `/projects/{slug}/decisions/{id}/feedback` | `{"correct": bool, "expected"?, "add_case"?: true}`: verdict, calibration case, Reflex `feedback` |
| GET/POST | `/projects/{slug}/cases` | Calibration cases; add `{"cases": [{"question_id", "text", "expected"}]}` (scored at once) |
| DELETE | `/projects/{slug}/cases/{id}` | Remove a case |
| POST | `/projects/{slug}/cases/fit` | `{"rho": 0.3, "apply": bool}`: re-score the stored cases (16 or more) and fit the threshold |
| POST | `/projects/{slug}/corpus/archive` | `{"domains"?: [...], "restore"?: bool}`: move domains to or from `_archive/` (default: the smallest over 8) |
| POST | `/projects/{slug}/reflex/restart`, GET `…/reflex/log` | Restart Reflex, read its log |
| GET | `/tasks/{id}/trace?limit=` | The task's orchestrator decisions (`decision.*` events), newest first |
| GET | `/decisions/model` | The decision graph (nodes with their rules, edges, event kinds) and the config its rules read |
| GET | `/decisions/edges?since=&project=` | Times each edge was taken since `since` (default 24 h); `project` empty = default workspace, absent = all |
| GET | `/decisions/latest?project=` | The newest decision of every task |
| POST | `/orchestrator/step` | Run exactly one orchestrator tick while it is paused (409 while running); returns that tick's decisions |
| GET | `/skills?project=` | Context files, skills (project, user, plugin; Claude and Codex) and their use in the project's workspace |
| PATCH | `/skills/enabled` | `{"project", "skill": <id>, "enabled"}`: switch a Claude skill for that workspace |
| GET | `/skills/file?project=&path=<id>` | A SKILL.md, only for ids the scanner itself returned |
| GET | `/agents?project=` | Agents of the project and the user, grouped by name, with each CLI's file, unreadable files and notes |
| GET | `/agents/{name}?project=&scope=` | The agent as one definition (from its Claude file, else agy, else Codex), its files and notes |
| PUT | `/agents?dry_run=` | `{"project", "scope", "def"}`: write each target's file, remove the others'; `dry_run=true` only returns the plan |
| DELETE | `/agents/{name}?project=&scope=` | Remove every file of the agent in that scope |
| GET | `/usage` | The latest usage report: windows, pace, tokens today, by project, skip state per provider |
| POST | `/usage/refresh` | `{}`: read every provider now and return the report |
| GET | `/updates` | The latest update check: installed and latest version, errors, update command and whether it is blocked, per provider |
| POST | `/updates/refresh` | `{}`: check every provider now and return the report |
| POST | `/providers/{name}/update` | `{}`: run the provider's `update` command in `<prefix>update-<name>` (409 while one runs or when the CLI is read-only); returns the session |
| GET | `/ws` | Pushes `ServerMessage` JSON (see `frontend/lib/generated/ServerMessage.ts`) |

Git: `git clone http://127.0.0.1:4400/git/<slug>/corpus.git`. With `api_token` set, git asks for a password: use the
token (any user name). Only corpus repos are served; a project's code stays in its folder.

## Security

- Binds to `127.0.0.1` by default and warns if it binds wider without a token.
- The `Host` header must be loopback, the bound address or a name in `allowed_hosts`. This blocks DNS rebinding.
- Browsers do not apply CORS to WebSockets, so `/api/ws` checks `Origin` itself.
- State-changing routes take JSON, which a cross-site form cannot send without a CORS preflight.
- Prompts are typed with `send-keys -l`, so their text is never read as key names. Only allow-listed keys can be sent.
- Only sessions under `session_prefix` are managed. The platform never lists, types into or kills anything else.

Agents run with whatever permissions their flags grant (e.g. `--dangerously-skip-permissions`). Run the platform as
a user, and against a workspace, you are comfortable letting agents change.

## Tests

```sh
cd backend && cargo test && cargo clippy --all-targets   # also regenerates frontend/lib/generated
cd frontend && bun run lint && bunx next typegen && bunx tsc --noEmit && bun run build
```

Backend tests cover reset-time parsing, pending-input detection (faint suggestions and the cursor are not typing), the
wake-up cycle, the orchestrator's dependency order, retries and human protection, and the store, all against an
in-memory terminal. Git access tests make a key in a temporary data dir (skipped without `ssh-keygen`) and clone,
fetch, pull and push only against local bare repos over `file://`; nothing contacts GitHub or GitLab.

## Roadmap

- Git worktree per task, so `max_parallel > 1` workers cannot edit the same files.
- Fail over a rate-limited task to another provider when its reset is far away.
- A usage reader for `agy` once it exposes its quota locally.
- Agent-to-agent messages: a supervisor session that creates and reviews tasks through the API.
- Activity feed parsed from agent panes (prompts, tool calls, replies), as the watcher's live feed did.
