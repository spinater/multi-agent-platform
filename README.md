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
  `rate_limited`, `needs_auth`, `needs_trust`, `stalled`, `error`, `exited`.
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
  - **Folder trust.** The CLIs ask once per folder whether to trust it, and an unattended worker would
    wait there forever. MAP marks each folder trusted before agents run in it, and shows any question that still
    appears as `needs_trust`, with a button to answer it. See [Folder trust](#folder-trust).
  - **Reflex gates.** A project can let its own Reflex answer two repeated decisions instead of an LLM: whether a
    finished change needs the LLM audit, and which model tier a task needs. See [Reflex gates](#reflex-gates).
- **Projects.** A project is a folder on the machine MAP is installed on, a corpus repo and its own
  [Reflex](https://reflex.gist.rs) decision engine.
  - **Folder.** Tasks and sessions of a project run in its folder, in place: an existing folder (a git repo or not),
    a new empty one MAP creates, or a new folder MAP clones a repo into (see [Git access](#git-access)). MAP never
    copies or hosts a project's code, runs git there only when you ask (clone, fetch, pull, push), and adds no files
    to it unless you init it with a template (see [Init a project](#init-a-project)); deleting a project leaves the
    folder alone. Two projects cannot share a folder or nest. The folder must be a sub-folder of the
    [project folder](#project-folder) (`[projects] path`), picked in the wizard's folder browser.
  - **Corpus.** A bare git repo of the platform's own, served at `/git/<slug>/corpus.git`: clone, push, or edit
    files from the dashboard.
  - **Decide before calling an LLM.** `POST /api/projects/<slug>/decide` asks Reflex first (local, no model, a few
    milliseconds). Answers at or above the project's confidence threshold are final; only the rest go to the
    project's fallback: an LLM API (Anthropic's, or an OpenAI-compatible one such as OpenRouter: `[reflex]
    api_format`, `api_url`), or any provider run headless (`claude -p`, `codex exec`, `agy -p`, `copilot -p`, …).
    Agents started in a project get the endpoint as `$MAP_DECIDE_URL`, and their prompt tells them to use it.
    With `api_token` set they also get `$MAP_PROJECT_TOKEN` (and `$MAP_API_URL`): a token derived from the API token
    that opens only that project's decide/feedback/decision-log endpoints and its corpus (files and `corpus.git`).
  - **Corpus content.** One top-level folder per Reflex domain (1–8), prose `.md` files inside. A push or an edit
    restarts the project's Reflex on the new corpus.
  - **Create.** Creating a project returns check lines: the folder used (its git branch and commits) or created,
    the corpus repo, Reflex started or why not, each provider found and signed in.
  - **Settings.** Each project can set its own provider order (with a model and a session limit per provider), verify
    commands and audit (on, off, or a given auditor), the [Reflex gates](#reflex-gates), and can be paused so the
    orchestrator starts none of its pending tasks. Unset fields fall back to `[orchestrator]` in `map.toml`. They live
    in the database, not in a file.
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
    `codex_auth` is missing).
  - **OpenRouter**: the API key's own credit limit and the free models' daily cap (`GET openrouter.ai/api/v1/key`
    with `OPENROUTER_API_KEY`, sent nowhere else). Other CLIs (`agy`, Cursor, Copilot) have no usage source MAP can
    read, so they show "no reader" rather than a guess.
  - **Tokens** today and by project (from each session's working directory), from the CLIs' session logs.
  - **Pace.** Readings are stored, and the last hour's rate projects the use at reset.
  - **Skip line.** No new tasks for a provider while any of its windows is at or above `skip_pct` (default 90),
    until that window resets. It shows as the provider's cooldown.
    it got right), and Reflex's own calibrator learns from `feedback`. Every decision is logged with its lane and a
    one-line `reason` (threshold comparison, shadow mode, Thai input…). Cases are stored per project and scored by
    Reflex as they are added; marking a logged decision right or wrong adds it as a case and tells Reflex's calibrator
    whether Reflex's own answer was right.
  - **Not calibrated** (no threshold): shadow mode. Reflex scores every question, but the fallback answers.
  - **Thai input** (any character in U+0E00–U+0E7F). Reflex tokenises English words only, so MAP glosses Thai first
    (`backend/src/thai.rs`): the text is split by longest match against a glossary (Thai has no spaces) and each word
    replaced by its English; English words, code and numbers stay. The glossary is built in
    (`backend/templates/thai-glossary.tsv`, ~800 words: software, business, people, time, particles) plus the
    project's own words (Reflex screen → **Thai words**: the Thai fragments recent decisions did not cover, a live
    preview of what Reflex reads, `PUT /api/projects/{slug}/thai/words`). The questions' prompts and options are
    glossed too. When at least 60% of the Thai is known, Reflex reads the English (the reason says "Thai read through
    the glossary (82% known)"); below that it goes straight to the fallback as before. Calibration cases in Thai are
    glossed the same way. Local and deterministic: no LLM translates.
  - **More than 8 domains** pauses Reflex. Archiving moves domains to `_archive/` in the corpus repo (one commit),
    which Reflex never loads; the dashboard's Reflex screen offers to archive the smallest.
  - **Savings** (`#reflex/savings`): LLM tokens Reflex saved against what the fallbacks spent, per day and per
    project. Every decision records its fallback prompt's length; spent tokens are the API's reported `usage`, else
    ~4 chars per token (headless CLIs report nothing; `[reflex] fallback_overhead_tokens` adds their own system
    prompt). A reflex answer is priced at the project's measured tokens per call when 3 or more API calls in 30 days
    reported usage, else `ceil(prompt chars / 4) + 16`. Thai decisions the glossary could not read are counted apart.
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
| `backend/src/roots.rs` | The project folder: where project folders may be (real paths, no `..`, no symlink out), and the folder picker's listing |
| `backend/src/templates.rs` | Project templates (built-in and `[templates] dirs`), the create/skip/append plan, laying one down, the init agent's task |
| `backend/src/plans.rs` | The planner session, plan validation, drafts, revisions, approval into tasks |
| `backend/src/project_settings.rs` | Per-project providers, verify, audit and pause, as the orchestrator asks for them |
| `backend/src/trust.rs` | Folder trust: marking folders trusted in Claude Code's and Codex's config, answering the trust question |
| `backend/src/login.rs` | Provider sign-in state from credential files, the helper login session, URL and code parsing |
| `backend/src/repo.rs` | The corpus repo written with git plumbing, corpus export, a read-only look at a project's folder |
| `backend/src/git_access.rs` | MAP's SSH key, pinned known_hosts, git identity, the gitconfig agents get as `GIT_CONFIG_GLOBAL`, URL and host checks |
| `backend/src/project_git.rs` | Git in a project's folder: status, fetch, ff-only pull, push, connect a remote, clone, ls-remote |
| `backend/src/settings.rs` | The Settings screen's read-only facts: server settings, run mode (container, systemd, launchd), agent binaries, gh/glab and their token variables (set or not) |
| `backend/src/reflex.rs` | One Reflex process per project: export the corpus, start, health-check, restart |
| `backend/src/usage.rs` | Plan-usage readers (Claude endpoint, Codex logs), token scans, pace, the skip line |
| `backend/src/updates.rs` | CLI update checks (version command vs. latest source), update helper sessions |
| `backend/src/skills.rs`, `agents.rs` | The Skills screen's scan; custom sub-agents read and written in each CLI's format |
| `backend/src/decide.rs` | The decision gate (Reflex, threshold, fallback), answer parsing, threshold fitting |
| `backend/src/savings.rs` | Tokens Reflex saved vs spent by the fallbacks (pure arithmetic over the decision log) |
| `backend/src/gates.rs` | The orchestrator's Reflex gates (`needs_audit`, `tier`): their state text, the ask, the `orchestration` seed |
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
deploy/install.sh                       # everything under ~/MAP (same as --home ~/MAP)
deploy/install.sh --home /root/project/MAP
deploy/install.sh --config-dir DIR --projects-dir DIR   # the two folders chosen one by one
deploy/install.sh --mode system         # a VM just for MAP: your own sign-ins and tmux, ~/.config/map, projects in ~
#   also: --port 4400 --bind 127.0.0.1 --prefix DIR --config FILE --agent-login isolated|shared --no-service
deploy/install.sh --uninstall           # removes the service and app/; keeps config/ (with data/) and workspace/
```

An install is laid out around two folders you choose, the **config folder** and the **project folder**:

```
~/MAP/                    --home DIR
├── app/                  --prefix: map, the dashboard, Reflex; replaced on every install or upgrade
├── config/               --config-dir: never touched by an upgrade
│   ├── map.toml          written once (never overwritten)
│   ├── templates/        your project templates ([templates] dirs)
│   ├── agents/           init-agent.md here replaces the built-in init agent's instructions
│   ├── agent-home/       MAP's own agent logins and history (claude/, codex/, gemini-home/), isolated mode
│   └── data/             database, run files, corpus repos, MAP's git key, logs, the default workspace
└── workspace/            --projects-dir: the project folder; projects can only be its sub-folders
```

It builds the server and dashboard into `app/`, adds a pinned Reflex (checked by SHA256) unless one is on PATH,
creates the folders, writes `config/map.toml` once with a fresh `api_token`, your current PATH (so the service finds
the same `claude`, `codex`, `agy`), `data_dir`, the default workspace `config/data/workspace` (a git repo inside the
data folder, so it is never a project), `[projects] path` = the project folder and `[agents_home] mode` (`isolated`
unless `--agent-login shared`; see [Agent logins](#agent-logins)), then starts a systemd user service
(`map.service`) or a launchd agent (`com.multi-agent-platform.map`) that runs `map serve -c <config>/map.toml`, and
prints the layout. Restarting the service leaves agent sessions in tmux running. Requirements: Rust, Bun, tmux, git,
curl, openssl. On Linux, `sudo loginctl enable-linger $USER` keeps it running after you log out.

- **Two modes.** `--mode isolated` (the default, the layout above) keeps MAP to its own folder: its agents sign in
  with their own accounts (`agent-home/`) and its sessions live on their own tmux server (`tmux_socket = "map"`,
  attach with `tmux -L map attach -t <session>`). `--mode system` is for a machine or VM provisioned for MAP, where
  MAP is one with the system: config in `~/.config/map/map.toml`, app and data in `~/.local/share/map`, `map` linked
  into `~/.local/bin`, your own CLI sign-ins (`~/.claude`, `~/.codex`, `~/.gemini`), tmux's default server (plain
  `tmux attach -t map-…`) and projects anywhere in your home folder (`--projects-dir` narrows it; MAP's data folder
  is never a project). The config records it as `[server] mode`, shown in Settings → Server.
- **Existing installs.** Without flags, an install made before this layout (`~/.config/map/map.toml`, app in
  `~/.local/share/map`) keeps its config and prefix; `--config FILE` still picks a config file. An existing config is
  never rewritten: when it sets no project folder, the installer prints the `[projects] path = "…"` lines to add.
- **Changing a folder later.** Run the installer again with the new flags (a config that exists is kept, so edit its
  `[projects] path` too), or edit `map.toml` and restart MAP. See [Project folder](#project-folder).

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
cp .env.example .env                        # uid/gid, user, home, Node dir, MAP_CONFIG_DIR, MAP_PROJECTS_DIR, port
mkdir -p ~/MAP/config/data ~/MAP/workspace && git init ~/MAP/config/data/workspace
cp deploy/map.docker.toml ~/MAP/config/map.toml
#   set the two folders' paths, api_token (openssl rand -hex 32), the PATH in [workspace.env], allowed_hosts
docker compose up -d --build
docker compose logs -f map                  # open http://127.0.0.1:4400 and paste the token once
```

- **Agent logins.** `deploy/map.docker.toml` sets `[agents_home] mode = "isolated"`: the agents' sign-ins and
  history live in `<MAP_CONFIG_DIR>/agent-home`, inside the config mount, so the container needs no `~/.claude`,
  `~/.codex` or `~/.gemini` mounts (remove them from `docker-compose.yml`); sign each provider in once under
  Settings → Agent CLIs. With `mode = "shared"` keep those mounts (read-write: sign-ins, history, transcripts for
  Usage and Skills, user-wide agents) to share the host's logins. See [Agent logins](#agent-logins).
- Mounted from the host: the CLIs in `~/.local/bin`, `~/.local/share/claude` and the Node install
  (read-only). Adjust `docker-compose.yml` if yours live elsewhere. Because the CLIs are read-only there, the update
  bell still shows new releases but its Update button is disabled: update them on the host.
- In shared mode, Claude Code in the container uses `CLAUDE_CONFIG_DIR=~/.claude`, so its settings file is `~/.claude/.claude.json`
  (the host rewrites `~/.claude.json` by renaming over it, which a single-file mount cannot follow). Set
  `"hasCompletedOnboarding": true` in that file once, or the first interactive session stops at the setup screens.
  MAP marks the default workspace and each project's folder trusted in that same file (see
  [Folder trust](#folder-trust)), so the "trust this folder?" question does not stop the workers.
- **Two folders, each mounted at the same path as on the host.** `MAP_CONFIG_DIR` is the config folder: `map.toml`
  (read-only on top), `templates/`, `agents/` and `data/` (database, run files, corpus repos, the default workspace,
  and MAP's git key and gitconfig under `data/git`). `MAP_PROJECTS_DIR` is the project folder: set `[projects] path`
  to the same path, and projects can be created only in its sub-folders (see [Project folder](#project-folder)).
  Same paths mean a project's folder reads the same in the container, on the host and in the dashboard. To mount the
  project folder elsewhere in the container, use `[projects] roots = [{ name, path = "<container path>", host_path =
  "<host path>" }]` instead, and the dashboard shows both paths. Every folder agents should reach needs a mount.
  Git needs no `~/.ssh` mount: create MAP's key under Settings → Git & SSH and add it to GitHub or GitLab (see
  [Git access](#git-access)).
- **Upgrading from `MAP_DIR`.** Older setups mounted `MAP_DIR/data` at `/data`. Set `MAP_CONFIG_DIR` to that
  `MAP_DIR`, and in its `map.toml` change `data_dir = "/data/map"` to `"<MAP_DIR>/data/map"` and `workspace.dir` to
  `"<MAP_DIR>/data/workspace"`, so the existing database is used.
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
- **Right:** "Needs you" (sessions waiting for input, signed out, asking to trust their folder, stalled or in error,
  and failed tasks), sessions with "+ New", and the event log. A session at the trust question shows the folder, and
  its card has **Trust this folder**.
- **Live activity** (`frontend/lib/activity.ts`, from the WebSocket's `task` and `session` messages; a full load only
  sets the baseline): when the orchestrator starts a worker, its links draw out from the task to the new session,
  rings burst from both nodes, its row pops in and a toast says `Task #N → claude (sonnet) spawned`. A working
  session's dot beats and each new pane line (`last_line`, pushed by the watcher; no extra polling) slides in and
  blips its node. Tasks show a run · verify · audit · land · done bar; a phase change ripples in its colour, done
  settles in green, failed shakes and flashes red. The "Live activity" list on the right and a ticker over the canvas
  show the newest spawns, phase changes and results. With `prefers-reduced-motion` nothing moves, only colour and
  opacity change; the canvas's "motion" toggle does the same for the canvas.
- **All projects:** the providers column becomes one hub per project, and IDs read `<project> #<id>`. The left
  column lists projects with "Open →", and cards offer only "Open in <project> →".

Deep links: `#command/tasks`, `#command/tasks/<id>`, `#command/tasks/new`, `#command/sessions/<name>`,
`#command/sessions/new`, `#projects/<slug>`, `#projects/init/<slug>`, `#lanes`, `#lanes/<name>`, `#settings/git`,
`#settings/agents`, `#settings/telegram`, `#settings/server`. Every layout number lives in `TUNING` in
`frontend/components/graph/layouts.ts`.

### Lanes (`#lanes`, `#lanes/<name>`)

One card per [lane](#lanes). Each card shows the state of every session and its why, context %, the rate-limit line
and reset, stall alerts and pauses. It also shows the status file (card, state, `need_clear`, note, age), the card
counts and a git line. It has toggles for watch, actions and auto `/clear`, the last actions and the queued tells
(cancel). **Tell** queues a clear, an ask with a note, or a pause of N minutes. The screen tail folds open, and the
status prompt can be edited. **Import watcher settings** reads an agent-watcher `config.json`. Context shows as
`81.6k / 200k (41%)` when it comes from the Claude transcript. MAP's own lanes (a project's [manager](#project-manager)
and its [persistent lanes](#persistent-lanes)) are grouped by project above the `[[lanes]]` blocks; a persistent lane
whose session is gone shows **stopped** with **Start**, and has **Close lane** / **Force close**.

### Activity graph (`#command/activity`)

The command centre's **Activity** tab shows a project's task flow, live. Each task is a row, left to right: who
created it (the manager, the planner or the owner) → the task → the orchestrator's pick, with the gates that routed
it (intake verdict, Reflex tier, the provider chosen) → the agent session (provider · model) → verify → audit → land →
done or failed. A retry goes round again through a new pick and agent, and a dashed edge leads to each task it
unblocked. Lanes the manager opened hang off the manager. What each agent used is read from its CLI's own transcript
(read only, tail first; Claude Code's `--name` session `.jsonl`, Codex's rollout file): commands grouped by head
(`cargo test` ×12) with secrets masked, skills, knowledge cards and sub-agents, collapsed to counts under the agent.
It reloads on WebSocket events (no polling): touched nodes pulse, a dot runs along a hand-off in progress, running
nodes show their elapsed time. Filter by 1 h / 24 h / all; done tasks fade out unless **show done** is on. Click a
task to open its card, an agent to filter to its edges and open its console, a command for its invocations, a
knowledge card to open it. API: `GET /api/projects/{slug}/activity?since=&all=&task=` (`{nodes, edges}`; the manager
token may call it) and `GET /api/sessions/{name}/trace` (one session's raw items, owner only). Tasks record
`created_by` (`owner`, `manager`, `planner`), sessions `spawned_by` (`orchestrator`, `owner`, `map`, `manager:<slug>`).

### Done tasks (`#command/done`)

Finished tasks leave the command centre's live views (lists, graph, columns), so they show what is still moving. The
task queue and the stats line show "N done →", which opens the **Done tasks** page. It lists the current project's
done tasks, or every project's in the all-projects view: newest finished first, searchable by title or `#id`, 50 per
page. Each row shows the provider/model, when it finished, how long it took (from its first session), the result
summary and its branch/commit or PR. Clicking a row opens its card in the command centre. Failed and cancelled tasks
stay in the command centre.

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
| Cursor, Copilot | none of their own here: they load Claude Code's files | `.claude/agents` | (their own folders, not managed) |

Cursor ([subagents](https://cursor.com/docs/context/subagents)) and Copilot
([custom agents](https://docs.github.com/en/copilot/reference/cli-command-reference)) both read a project's
`.claude/agents`, so MAP writes nothing for them; when they are configured the matrix shows a column for each
(`↳ via Claude Code` when the Claude file is usable, `—` without one, `n/a` for user agents) from
`[agents.also_loaded_by]`. Model names in those files are Claude Code's aliases, which the other CLIs may map
differently.

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

### Configure: Permissions and MCP servers

Both tabs edit the provider CLIs' own files, for the machine (the CLI's state folder as MAP runs it: its isolated
agent home in `[agents_home] mode = "isolated"`) or for one project. The columns are the providers MAP runs
(`orchestrator.provider_order`, then the audit chain), because each CLI enforces its own settings.

- **Permissions** (`permissions.rs`): rows `Read / Grep`, `Edit / Write`, `Bash(git *)`, `Bash(npm *, bun *)`,
  `Bash(*)`, `WebFetch / WebSearch` and one `mcp__<server>__*` per MCP server; cells allow / ask / deny.
  Claude Code: `permissions.allow / ask / deny` in `<home>/settings.json` or `<project>/.claude/settings.local.json`
  (`Read`, `Grep`, `Glob`, `Bash(git *)`, `Bash`, `mcp__<server>`; the legacy `Bash(git:*)` reads the same; the shared
  `.claude/settings.json` is read, never written). Gemini / agy: `tools.allowed` / `tools.exclude` (or the older
  `allowedTools` / `excludeTools` a file already uses) in `<home>/settings.json` or `<project>/.gemini/settings.json`;
  it has no ask list, so ask is neither list. Codex has no per-tool rules: its cells are `unsupported`, and its column
  note shows `approval_policy` and `sandbox_mode`. In project scope a cell is what the CLI will apply: the stricter of
  the project's and the machine's value (both CLIs merge the lists, and deny wins), `override` when the project's
  stricter value wins, with a note when a project value loses. Other rules and keys in the files stay as they were.
- **MCP servers** (`mcp.rs`): one list across Claude Code (`.claude.json` top-level `mcpServers` next to its config
  folder; `<project>/.mcp.json`), Codex (`[mcp_servers.<name>]` in `config.toml`; `<project>/.codex/config.toml`,
  edited with `toml_edit`) and Gemini / agy (`mcpServers` in `settings.json`; `<project>/.gemini/settings.json`). A
  project definition replaces the machine one of the same CLI. Env and header values can be written but only their
  names and whether they are set are returned. Test connection spawns the server in its own process group (stdio) or
  POSTs to it (streamable HTTP, JSON or SSE replies), runs `initialize`, `notifications/initialized`, `tools/list`
  within 15 s, always kills the group, and redacts the server's values from errors; the last result per scope is
  kept in memory as the server's `status`.

### Knowledge (`#knowledge`, `#knowledge/<slug>`, `#knowledge/<slug>/doc/<path>`)

A project's knowledge: the Markdown and text files in its folder (knowledge cards with their index, requirements,
task sheets, rulebooks such as `CLAUDE.md`) plus what MAP remembers itself (finished tasks, approved plans, shown as
`map://tasks/<id>`, `map://plans/<id>`). Opened from the nav or a project page's **Knowledge** button
(`frontend/components/knowledge/`):

- **Header:** project picker, the folder, the index (or how the convention works when there is none), when it was
  indexed, and **Reindex**.
- **Health:** errors, warnings and info grouped by check (stale card, missing source, over or near the line cap, not
  in the index, broken link, rulebook size, scratch size, nested repo copy, no index), each with a one-line reason.
  **Create fix task** queues the issue's ready prompt as a task in the project after a confirm; **Fix all stale
  cards** queues one task for all of them.
- **Budgets:** each rulebook's bytes against its warn line and cap, and scratch folders with their size and any repo
  copies inside.
- **Search:** the same substring search agents use (Thai or English), filtered by kind; matches are highlighted.
- **Browse:** documents by kind, sortable by name, lines or modified; cards show lines against the cap, whether the
  index lists them and how many sources they have.
- **Doc viewer:** a small built-in Markdown renderer (no HTML from the file ever reaches the page); links to other
  indexed docs and the card's `sources:` open in the viewer, http(s) links in a new tab.
- **Agent searches:** the latest queries, by agents or you, with their hit counts, so a search that found nothing
  points at a gap.

Folders, the index path, rulebooks, line and byte limits and scratch folders are in the project's settings
(**Knowledge** box), saved with the other orchestrator settings.

The **Reflex** screen (`#reflex`, `frontend/components/reflex/`) is the current project's decision engine: how it
works with the last 24 h per lane, a set-up checklist that ticks itself (install, corpus, questions, calibration,
playground), a playground with feedback, calibration on a log-scale scatter with a draggable threshold, and the
decision log. Its **Savings** tab (`#reflex/savings`) shows the tokens Reflex saved over 24 h, 7 d or 30 d, for all
projects or the current one, with a note on how they are counted; the Usage screen links to it. Confidence is always shown on a log axis (1e-5 … 1e-3, widened by whole decades when a corpus scores
outside it), never as a percentage.

## Git access

One git setup for every project, so agents can clone, pull and push GitHub, GitLab or any ssh git host
(`backend/src/git_access.rs`, `project_git.rs`; [Settings → Git & SSH](#settings), also linked from **Git access** on
the Projects screen and a project's Git panel).

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
  (`StrictHostKeyChecking=accept-new`), and a test shows the fingerprint it recorded, to compare with the host's
  own. Settings lists every key, pinned or learned; a learned host can be forgotten (so its next key is recorded
  again), a pinned one cannot.
- **SSH method.** `<data_dir>/git/settings.json` (`{"ssh": "map_key"}` by default): MAP's own key as above, or
  `"system"`, this machine's own SSH setup (`~/.ssh` keys, config and known_hosts, ssh-agent). With `system` the
  gitconfig leaves `core.sshCommand` out (it keeps the identity and the includes), and **Test** runs plain
  `ssh -o BatchMode=yes -T git@<host>`, as git will. MAP never reads a private key or writes `~/.ssh`: it only lists
  the `*.pub` files there and their fingerprints. In Docker `~/.ssh` is not mounted, so `system` finds no keys
  unless you add the read-only mount shown in a comment in `docker-compose.yml`.
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
  `<project folder>/<repo name>` (missing or empty, inside the project folder). It shows `git ls-remote` first (default branch, branches, or why
  the host refused, with a hint to add MAP's key), then runs `git clone` (a normal working clone, not bare). The
  project is then an ordinary folder project (`kind = "local"`), with the URL kept as `source.remote`.
- **Sync.** A project's page has a Git panel (and its card a one-line status): branch, upstream, origin, ahead/behind,
  changed files, last commit and fetch, and **Fetch**, **Pull** (fast-forward only; refused while the folder has
  changes or the branch diverged), **Push** (the current branch, never forced; the first push sets the upstream with
  `-u origin <branch>`) and **Connect a remote** (sets or replaces `origin`, after `git init -b main` when the folder
  is not a repo yet). Each shows its commands and output and logs a `project.git.*` event. Pull, push and remote
  changes are refused while a task of the project is running; fetch and status are not.

## Project folder

Projects can be created only in sub-folders of one folder, the project folder, whether MAP runs as a service or in
Docker (`backend/src/roots.rs`):

```toml
[projects]
path = "/root/project/MAP/workspace"   # `root` is the older name; default ~/project
```

- **What it restricts:** the dashboard's folder picking and project creation. Every way a project gets a folder (an
  existing one, a new empty one, a clone target) is checked on the server: the folder's real path (for a folder that
  does not exist yet, the real path of its nearest existing ancestor plus the rest) must be inside the project folder's
  real path. `..` is refused, a symlink that leads out of it is refused, and the project folder itself cannot be a
  project, only its sub-folders. A new folder may be several levels down (`group/team/app`); the missing levels are
  created. The error names the allowed folder. The wizard's inspect makes the same check as you pick.
- **What it does not restrict:** what an agent CLI can read or run once started. That is the CLI's own permissions
  and sandbox (`--dangerously-skip-permissions` gives it the whole account), or in Docker the container's mounts.
- **The picker.** The wizard's folder step (Existing, New folder, Clone into) browses only inside it: a breadcrumb,
  the sub-folders (a `git` badge, "project X" when a MAP project uses or contains one, which disables picking it),
  **Select this folder**, and for new and clone **New folder here** with a name (one path segment, no slashes).
  Dot-folders show on request. "Type a path instead" takes a typed path, checked against the project folder too.
- **Existing projects** whose folder is outside it (made before it was set) keep working; their page says "outside
  the configured project folder", and a project cannot be created there again.
- **Checks at start:** a missing project folder is a warning (create it to pick folders in it), not an error. A
  project folder inside MAP's data dir is an error; the data dir inside the project folder is a warning, and the data
  dir is never listed or accepted. The default workspace lives in the data folder, so it never shows up as a project.
- **Several folders (advanced).** Instead of `path`, `roots = [{ name = "Projects", path = "~/project" }, { name =
  "Clients", path = "/projects", host_path = "/home/me/clients", allow_root_itself = false }]` with `default_root =
  "<name>"` for clones (default: the first). Names must be unique, paths absolute (after `~`) and not nested in each
  other. `host_path` is display only: the same folder on the host, when a container mounts it at another path. The
  picker then shows a chip per root.
- **API** (owner token only; an agent's project token cannot call them): `GET /api/fs/roots` and
  `GET /api/fs/browse?root=<name>&path=<relative>&hidden=true`, which returns folders only (never file contents, only a
  file count), sorted, at most 500 (`truncated`), with symlinks listed only when they lead to a folder inside the same
  root; an absolute path, `..` or a symlink out of the root is a 400.

## Providers

Six providers are built in; `provider_order` (which the orchestrator tries for tasks without a pinned provider) keeps
the first three, and the other three are opt-in: add them to `provider_order`, the audit `chain`, a project's
providers, or pick them for a session. A provider whose binary is not on the agents' PATH shows as not installed and
is skipped. Everything below is a default that `[providers.<name>]` in `map.toml` overrides (`map.example.toml` lists
every field).

| provider | CLI | sign-in | headless | folder trust | sub-agents | usage |
| --- | --- | --- | --- | --- | --- | --- |
| `claude` | Claude Code | `claude auth login`, or `ANTHROPIC_API_KEY` | `claude -p` | written | `.claude/agents` | account endpoint |
| `codex` | Codex | `codex login --device-auth`, or `OPENAI_API_KEY` | `codex exec` | written | `.codex/agents` | session logs |
| `gemini` | agy | `/login` in its TUI, or `GEMINI_API_KEY` | `agy -p` | asked (`needs_trust`) | `.agents/agents` | none |
| `cursor` | [Cursor CLI](https://cursor.com/docs/cli/overview) (`cursor-agent`) | `cursor-agent login` (URL printed), or `CURSOR_API_KEY` | `cursor-agent -p --output-format text --trust` | asked (`needs_trust`) | loads `.claude/agents` | none |
| `copilot` | [GitHub Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli) (`copilot`) | `copilot login --device-code`, or `COPILOT_GITHUB_TOKEN` | `copilot -p … -s --no-ask-user` | written (`trusted_folders`) | loads `.claude/agents` | none |
| `openrouter` | Claude Code via [OpenRouter](https://openrouter.ai/docs/guides/guides/coding-agents) | `OPENROUTER_API_KEY` only | `claude -p` | written | `.claude/agents` | key limit |

- **Cursor** takes its first prompt as an argument. Its model list is just `auto`: `cursor-agent --list-models` prints
  the ids your account has; add them to `models`. Where it keeps its sign-in and trusted folders is not documented,
  so the dashboard shows its sign-in as unknown (unless `CURSOR_API_KEY` is set) and its "Trust this workspace"
  question as `needs_trust`. Unattended workers need `extra_args = ["--force"]` to run commands.
- **Copilot** gets its first prompt typed in (`-i <prompt>` cannot be left empty for a session without one). Its
  sign-in sits in the system keychain, so it shows as unknown unless `COPILOT_GITHUB_TOKEN` (a fine-grained token
  with the "Copilot Requests" permission; `GH_TOKEN` and `GITHUB_TOKEN` also work, classic `ghp_` tokens do not) is
  set. MAP adds each folder to `trusted_folders` in `~/.copilot/config.json` (keeping the comment lines Copilot
  writes above the JSON). Unattended workers need `extra_args = ["--allow-all-tools"]`. Copilot also reads
  `AGENTS.md`, `CLAUDE.md` and `.github/copilot-instructions.md`, and skills in `.claude/skills` and `.agents/skills`.
- **OpenRouter** runs `claude` with `ANTHROPIC_BASE_URL=https://openrouter.ai/api`, `ANTHROPIC_API_KEY=""` and
  `ANTHROPIC_AUTH_TOKEN="$OPENROUTER_API_KEY"`, and points Claude Code's own `opus`/`sonnet`/`haiku` aliases at
  OpenRouter's `~anthropic/claude-*-latest` models. Models are OpenRouter ids (`~anthropic/claude-sonnet-latest`,
  `anthropic/claude-opus-5.5`, `openai/gpt-6.1-sol`, …); non-Anthropic models work only as well as they handle Claude
  Code's tools. It has its own Claude config folder, `~/.claude-openrouter` (isolated: `<dir>/openrouter`), in both
  modes and whatever `[workspace.env]` says, so it never uses, or signs out, the `claude` provider's account; MAP
  marks it onboarded there so the first run skips Claude Code's theme picker. It is "signed in" when
  `OPENROUTER_API_KEY` is set in MAP's environment or `[workspace.env]`; the dashboard only ever shows that boolean.
  The key is never written into a launch script: the script exports it by reference, so it must be in the tmux
  session's environment. Put it in MAP's service environment (e.g. a systemd `EnvironmentFile`) and use a private
  `workspace.tmux_socket`, whose server MAP starts with its own environment; with tmux's default server the session
  only has what that server has, and the pane says `map: OPENROUTER_API_KEY is not set` when it is missing.
- **OpenRouter as the decision fallback.** A project whose fallback is `api` can use OpenRouter (or any
  OpenAI-compatible server) instead of the Anthropic API: `[reflex] api_format = "openai"`, `api_url =
  "https://openrouter.ai/api/v1/chat/completions"`, `api_key_env = "OPENROUTER_API_KEY"`, `api_model =
  "~anthropic/claude-haiku-latest"`. The key is read from MAP's environment for each call and never stored.
- A provider's `env` table is exported for every run after `[workspace.env]`; a value that is exactly `$NAME` is
  exported by reference (`export VAR="$NAME"`), and commands MAP runs itself (headless fallback, audit) resolve it in
  memory.

## Agent logins

`[agents_home] mode` in `map.toml` decides whose accounts and history MAP's agents use.

- **`shared`** (the default for an existing `map.toml`): the CLIs' usual folders, `~/.claude`, `~/.codex`,
  `~/.gemini`. MAP's agents and your own `claude` / `codex` / `agy` in a terminal are the same login, with the same
  history, settings and rate limits; signing in or out on one side does it for the other.
- **`isolated`** (what `deploy/install.sh` and `deploy/map.docker.toml` write for a new config): each provider gets
  its own folder under `[agents_home] dir` (default `agent-home` next to `map.toml`, created with mode 0700), and
  every CLI MAP starts gets the variable that moves its state there: sessions, tasks, the planner and init agents,
  the sign-in and update helper sessions, the headless fallback and audit calls, and the version checks.

| provider | variable | folder | its files |
| --- | --- | --- | --- |
| claude | `CLAUDE_CONFIG_DIR=<dir>/claude` | `<dir>/claude` | `.claude.json`, `.credentials.json`, `settings.json`, `projects/` (transcripts), `skills/`, `agents/` |
| codex | `CODEX_HOME=<dir>/codex` | `<dir>/codex` | `auth.json`, `config.toml`, `sessions/`, `skills/`, `agents/` |
| gemini (`agy`) | `HOME=<dir>/gemini-home` | `<dir>/gemini-home/.gemini` | `jetski-standalone-oauth-token`, `config/agents/`, … |
| cursor | `HOME=<dir>/cursor-home` | `<dir>/cursor-home/.cursor` | `cli-config.json`, and whatever else it keeps under `~` |
| copilot | `COPILOT_HOME=<dir>/copilot` | `<dir>/copilot` | `config.json` (sign-in without a keychain, `trusted_folders`), `settings.json`, `agents/`, `skills/` |
| openrouter | `CLAUDE_CONFIG_DIR=<dir>/openrouter` | `<dir>/openrouter` | as claude; shared mode: `~/.claude-openrouter` |

Cursor has `CURSOR_CONFIG_DIR`, but its docs do not say that it moves the sign-in too, so like agy it gets a `HOME` of
its own (with the same `home_keep`). Copilot's `COPILOT_HOME` replaces `~/.copilot` entirely; its token may still be
in the system keychain (service `copilot-cli`), which `COPILOT_HOME` does not separate.

agy 1.3 has no variable for its `~/.gemini` (its binary only knows `JETSKI_APP_DATA_DIR`, which it passes to its own
language server), so MAP gives it a `HOME` of its own; `CARGO_HOME`, `RUSTUP_HOME` and `GOPATH` are exported next to
it so the tools an agent runs keep their real folders (`providers.gemini.home_keep`). Two consequences: agy may keep
its token in the system keyring, which a different `HOME` does not separate, and an agy self-update (or the Update
button) lands in that `HOME`, not in the `agy` on your PATH: update agy from your own shell.

Everything MAP reads from a CLI's state follows the same folder ([`Config::provider_home`](backend/src/config.rs)):
the sign-in check and Settings' credential paths, Usage (Claude's OAuth token for the usage call, its transcripts,
Codex's `sessions/` and `auth.json`), Skills (user skills, plugins, `settings.json`, transcripts), user-wide Agents,
folder trust (`.claude.json`, `config.toml`) and Claude's update channel. A variable set in `[workspace.env]` (say
`CODEX_HOME`) wins over the isolated folder. Nothing is ever copied between your folders and MAP's: after switching
to isolated, every provider shows **not signed in yet — Sign in** under Settings → Agent CLIs, which runs the
provider's own login in a helper session with MAP's folder. The per-provider fields are `home_env`, `default_home`,
`home_subdir`, `isolated_dir` and `home_keep` (see `map.example.toml`); `home_env = ""` keeps one provider shared.

**The sign-in session.** For a CLI that signs in from its TUI (`login_send`, agy's `/login`), MAP does not type
blindly after `startup_secs`: it watches the pane for up to 60 s, answers the provider's known harmless first-run
pickers with Enter (`login_onboarding`; agy's colour-scheme picker, `[Next]`, `enter Confirm`, and its login-method picker, where Enter takes Google OAuth), and types
`login_send` only once the CLI's prompt shows (`login_ready`, else `idle_markers`). A folder-trust or permission
question is never answered (`detect::trust_prompt`); it waits for you in the console. `GET
/api/providers/{name}/login` returns a `phase` (`starting`, `onboarding`, `ready`, `browser`, `waiting_for_code`,
`success`, `console`, `ended`) and a `hint` the sign-in modal shows, and a pasted code goes through `POST
…/login/code`, which types it only while the code prompt is the last line on screen. A sign-in finished anywhere
(the modal, a terminal) reaches the dashboard without a reload: the watcher compares each provider's availability,
sign-in, API key and cooldown every tick and pushes a `providers` WebSocket message when one changes.

## tmux socket

By default MAP's sessions live on tmux's default server, next to any tmux you use yourself. With
`[workspace] tmux_socket = "map"` every tmux command MAP runs uses `tmux -L map`, a private server: attach with
`tmux -L map attach -t '=<session>'` (Settings → Server shows the command, and the console's attach hint uses it).
Either way MAP removes `TMUX` and `TMUX_PANE` from the environment of its tmux commands, so a MAP started from
inside a tmux session never targets that server by accident. Changing the socket of a running install leaves its
sessions on the old server: MAP no longer sees them.

## Lanes

A lane is a tmux session someone else owns, such as an agent you started yourself on tmux's default server or on a
named socket. MAP watches it and reports it to [Telegram](#telegram). It types into a lane only when you allow it.
MAP never creates, kills, renames or resizes a lane session (`backend/src/lanes.rs`; ported from agent-watcher's
`watcher.py`). Declare lanes in `map.toml` (`[[lanes]]`, see `map.example.toml`). Change `watch`, `actions`, auto
`/clear` and `status_prompt` on the **Lanes** screen (`#lanes`) or with `/watch`; those changes are kept in the
database over `map.toml`. `POST /api/lanes/import-watcher` fills a lane's `status_prompt`, thresholds and the
Telegram chat and users from an agent-watcher `config.json` (read only, never the token, only inside your home).

Every `[lane_watch] interval_secs` (60) MAP reads each lane:

- **State**: busy (a spinner that is not `<verb> for <time>`, `esc to interrupt`, or a pane still moving between two
  captures `confirm_secs` apart, timers ignored), waiting (a dialog with a `❯` option), unsent text on the input line
  (faint suggestions do not count), idle, or rate-limited (the CLI's limit line *with* its reset time on the same or
  next line; `(UTC)` or local, a time over 15 minutes past means tomorrow).
- **Facts**: context used and the `new task? … /clear` hint, the agent's status file (`{card, state, need_clear, note,
  at}`), git (branch, last commit, uncommitted and unpushed counts, `GIT_OPTIONAL_LOCKS=0`), and `todo/` /
  `todo-human/` / `done/` counts.

**Context used.** The pane's `N% context used` line wins when the CLI prints one. The current Claude Code no longer
does, so for a Claude pane (its input box between two rules) in a lane with a `path`, MAP reads the newest
`<claude_dir>/projects/<path with every non-alphanumeric character as ->/*.jsonl` (a `/clear` starts a new file): only
its last 256 KB, the last main-thread assistant message's `message.usage`, `input_tokens + cache_read_input_tokens +
cache_creation_input_tokens`, over the lane's `context_window` (200000). `claude_dir` defaults to `~/.claude` (lanes are
your own sessions; read only); MAP's own lanes use the provider's folder MAP launched them with. The lane API returns
`context_pct`, `context_tokens`, `context_window` and `context_source` (`pane` or `transcript`), and the percentage
feeds `clear_when.context_pct`.

It types only when `actions` is on, the session is idle, nobody is attached and nothing is on the input line, and
one session per lane per tick (`max_sessions_per_round`):

| Move | When |
| --- | --- |
| `/clear` | `clear_when.enabled`; never while the limit line is on screen; not within `cooldown_mins`; with `only_when_done`, only after a fresh status file says `done`; then the agent's `need_clear` decides (true skips `idle_mins`, false wins over the CLI), else the CLI's hint or `context_pct` after `idle_mins`. MAP checks the pane afterwards (hint gone, and the context under 50%, or, when it came from the transcript, a new transcript file or fewer tokens in it) and types `status_prompt` in the same round |
| `status_prompt` | After the usage limit reset (beats the cooldown and an empty `todo/`; `resume_cooldown_mins`); when the status file says `done` (`resume_cooldown_mins`); idle and unchanged `idle_stale_mins` (`cooldown_mins`). An empty `todo/` stops the last two, never the notices |
| tells | `POST /api/lanes/{name}/tell` or `deploy/map-tell`: `clear`, `ask` (the note is typed; none: the status prompt), `pause` (`mins`, automatic moves only). They beat every rule but "idle, nobody attached", wait while the limit line is on screen, expire after 3 hours; the same verb again replaces the queued one |

A pane stuck where MAP cannot act alerts after `stall_when.stall_mins`, again every `repeat_mins`: a dialog, unsent
text, or still parked after the reset. A card done is announced once per card (card + time written). Every move is a
`lane.*` event (`lane.clear`, `lane.ask`, `lane.resume`, `lane.stall`, `lane.card_done`, `lane.rate_limited`,
`lane.tell`, `lane.watch`) and a line of the lane's history.

`deploy/map-tell` mirrors `watcher.py --tell` from inside the lane's session (it reads the session name from tmux):

```sh
map-tell --tell clear --note "card 200 done"   # /clear, then the status prompt, once idle
map-tell --tell ask --note "do card 188 next"
map-tell --tell pause --mins 90
map-tell --tell clear --session groove-agos    # another session
map-tell --tells
```

It reads `MAP_URL` and `MAP_TOKEN` (`server.api_token`) from the environment or `~/.config/map/tell.env`, and passes
the token to curl on stdin.

## Telegram

`[telegram]` (`backend/src/telegram.rs`; ported from agent-watcher) sends plain text to `chat_id`. Messages are split
at 3900 characters, each piece is tried 3 times, and link previews are off. MAP reads the token when it sends:
`token_env` from its environment, else that variable's line in `env_file`. MAP never stores, logs or returns the
token, and redacts it from every error, because a Bot API URL contains it.

- **The full report** goes out on `report_minute` (45) of every hour, and whenever 70 minutes passed without one.
  It shows every lane as the last tick saw it, with one `⏸ paused via /watch` line for a muted lane, and a line
  about MAP's tasks and waiting plans.
- **Sent at once**: a card finished, a stall alert, a lane parked at or resumed after the limit, a `/clear` or a
  prompt typed, a MAP task done, failed or held by intake (`task.held`), and a plan draft waiting for approval. A
  muted lane sends nothing.
- **Commands** (`commands = true`) run on a getUpdates long-poll, with the offset kept in the database:
  - `/watch` (or `/start`, `/menu`) shows one button per lane (✅ watching / ⏸ off); a press toggles the lane's
    `watch`.
  - `/status` sends the full report now.
  - `/ask <slug> <text>` (or a reply to a manager's message, which starts with `🤵 <slug> ·`) queues the text as an
    `ask` tell for that project's [manager](#project-manager): typed only when it is idle, nobody is attached and
    nothing is on its input line (MAP ticks that lane at once, then on every round). Once it is idle again, its last
    message (`detect::last_assistant_message`) goes back to the chat, trimmed to 3500 characters.
  - `/managers` lists the projects with a manager on and their state.

  At start MAP sets the bot's command list and clears any old reply keyboard. Commands come from
  `allowed_user_ids`, or from anyone in `chat_id` when that list is empty. MAP ignores everyone else silently.
  Telegram allows one getUpdates consumer per token. When it answers 409, MAP logs one `telegram.conflict` warning
  and backs off; stop the other consumer, or set `commands = false`.
- **Settings → Telegram** (`#settings/telegram`) shows whether the token is set (never its value), the chat, the
  users, the listener's state, the last send and error (redacted), and has a **Send test** button.

## Settings

The **Settings** screen (`#settings`, the gear in the top bar, or the last nav entry) is what someone who installs
MAP sets up once (`frontend/components/screens/settings.tsx`, `components/settings/`; `backend/src/settings.rs`).
Its sections are listed on the left (stacked on a phone) and have their own links: `#settings/git`,
`#settings/agents`, `#settings/telegram`, `#settings/server`. No secret is ever typed or shown there: only public keys, and whether a
token is set.

- **Git & SSH** (`#settings/git`): the SSH method (MAP's own key, or this machine's `~/.ssh` and ssh-agent; see
  [Git access](#git-access)), MAP's key (create, copy, replace), a card each for GitHub, GitLab.com and a
  self-hosted GitLab or other git host (host and optional port) with the steps for an account key or a repo deploy
  key with write access and a **Test** button, MAP's known hosts (forget a learned one), and the git identity. The
  Projects screen's **Git access** button and a project's Git panel link here; the New project wizard opens the same
  panel over itself, so what was typed is kept.
- **GitHub / GitLab CLI** (in Git & SSH): agents that open pull or merge requests and issues with `gh` or `glab`
  need `GH_TOKEN` (or `GITHUB_TOKEN`) and `GITLAB_TOKEN` (plus `GITLAB_HOST` for a self-hosted GitLab). Set them in
  `[workspace.env]` of `map.toml` or in the service's environment (systemd `Environment=`, launchd
  `EnvironmentVariables`, compose `environment:`), never in the dashboard. Settings shows only whether `gh` and
  `glab` are on the agents' PATH and whether each variable is set, and where (map.toml or MAP's own environment).
- **Agent CLIs** (`#settings/agents`): whose logins the agents use ([Agent logins](#agent-logins): isolated or
  shared, and the folder), then per provider the binary and where it resolves on the agents' PATH, its state folder,
  the installed and latest version from the [update check](#what-it-does) with **Check versions** and **Update** (the
  same confirmation and helper session as the bell), and the sign-in state with **Sign in** (the sign-in modal).
- **Server** (`#settings/server`), read only: first the **config folder** and the **project folder** (whether it
  exists, its host path in Docker, every root with `roots`) and how to change them (reinstall with `--config-dir` /
  `--projects-dir` / `--home`, or edit `map.toml` and restart); then the config file, data dir, bind address,
  `allowed_hosts`, whether `api_token` is set (never its value), default workspace, the tmux socket and the attach
  command ([tmux socket](#tmux-socket)), static dir, MAP's version, Reflex's binary
  and version, and how MAP runs: in a container (`/.dockerenv` or `/run/.containerenv`), as a systemd service
  (`INVOCATION_ID`), a launchd agent (`XPC_SERVICE_NAME`) or by hand. `map.toml` is not editable from the dashboard:
  edit it and restart (`docker compose restart map`, `systemctl --user restart map`, or
  `launchctl kickstart -k gui/$(id -u)/com.multi-agent-platform.map`).

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
- **Reflex corpus.** With "Draft the Reflex corpus from this description" ticked (`draft_corpus`, on by default),
  the init agent also writes the project's corpus: 1–8 domains for the decisions the description implies (routing,
  triage, approvals…), always including `orchestration` (which changes need a careful LLM audit, such as money,
  auth, schema, deploy and data deletion, and which do not, such as docs, copy, tests only and formatting; which
  tasks are light, standard or heavy work, for the orchestrator's Reflex gates). Each domain is English prose
  written like a runbook (a few hundred to ~1500 words), even when the description is in Thai, since Reflex reads
  English only. The agent writes them with `$MAP_PROJECT_TOKEN` through `PUT /api/projects/<slug>/repos/corpus/file`
  (or one push to `corpus.git`), deletes the `example/` placeholder once real domains exist, checks the result with
  `GET .../repos/corpus`, adds a line to the project's `CLAUDE.md` that states sent to `$MAP_DECIDE_URL` must be in
  English, and lists what it wrote in its summary. The prompt carries the corpus's current domains (so a re-init
  extends them rather than adding copies) and its rules. Unticked, the prompt says nothing about the corpus, and
  the check lines say `corpus: left as is`.

## Requirements → plan → tasks

Every project, and the default workspace, has a built-in **planner** agent (`backend/src/plans.rs`): you tell it
what you want, the way you would tell an engineer, and it turns that into tasks for the orchestrator, but only after
you approve them.

- **Talk to it.** "Requirements" on a project's page, or the Requirements button in the command centre
  (`#command/requirements`), starts (or reopens) an ordinary interactive session `<session_prefix>plan-<slug>`
  (`<session_prefix>plan` for the default workspace) in the project's folder, on the provider you pick (by default
  the project's first installed provider that is not signed out). Its console is shown inline; type to it there or
  `tmux attach`. The session has the supervisor role and no task, so the orchestrator never treats it as a worker;
  the watcher and the human protection apply as to any session.
- **Instructions.** One definition for the platform, `backend/templates/planner-agent.md` (or `[templates]
  planner_agent`), sent as the session's first prompt (by `prompt_mode`, like any launch) followed by "This session":
  the project, its folder, whether it keeps `tasks/` cards (and the next card number), the providers and models it
  may pin, the orchestrator's state and the plans so far. The planner talks in your language, reads the project
  first, asks only questions that change the plan (each option with a one-line scenario), restates the requirement,
  and proposes small, verifiable tasks with self-contained prompts (goal, scope and non-goals, files, acceptance
  criteria, how to verify), dependencies and an optional provider/model. It never writes code and never starts
  tasks. In a project with `tasks/README.md` (the team-process template) it also writes the cards and puts each
  card's path in its task's prompt. The Agents screen lists it as a built-in row with its instructions, read-only.
- **Draft.** The planner POSTs the plan with curl to `$MAP_PLANS_URL` with `$MAP_PROJECT_TOKEN`. MAP checks it
  (unique keys, dependencies on keys of the plan, no cycles, a title and a prompt per task, known providers and
  models, at most 50 tasks) and stores it as a `draft`; the dashboard shows a toast. Sending `replaces: <id>` makes
  a new revision: an older draft becomes `superseded`.
- **Approval gate.** Nothing runs until you approve. In the Requirements panel you read each task's prompt, edit its
  title, prompt or dependencies, then **Approve** (creates the tasks in dependency order, mapping the plan's keys
  to task ids, in the plan's project; a second approval is refused) or **Reject** with a reason the planner can
  read. The orchestrator then runs the tasks like any others, when it is running and the project is not paused.
- **Tokens.** A project's token opens only its own `GET`/`POST /projects/<slug>/plans`; the default workspace's
  planner gets a token of its own that opens only `/workspace/plans`. Approving, rejecting, editing and starting the
  planner need the API token. Events: `planner.started`, `plan.created`, `plan.updated`, `plan.superseded`,
  `plan.approved`, `plan.rejected`; every change is pushed as a `plan` WebSocket message.

## Project manager

A project can have a **manager** (`backend/src/manager.rs`): an always-on agent session `<session_prefix>mgr-<slug>`
in the project's folder that you talk to about the project, so you can steer everything without the web: "what is
running?", "why did #12 fail?", "approve plan 7", "pause the project", "open a lane for the docs".

- **On/off** in the project page's **Manager** panel (off for existing projects): provider and model (default: the
  project's first installed, signed-in provider, as the planner), state (`off`, `starting`, `running`,
  `restarting`, `stopped`, `failed`), the inline console and the "Open lane" form. The command centre shows its
  state chip. Stored in the project's settings as `manager: {enabled, provider, model}`; `PUT
  /api/projects/{slug}/manager` changes it (turning it off ends the session, 409 while someone uses it unless
  `force`).
- **Kept alive.** MAP starts it at boot and a supervisor (every 15 s) starts it again when its session is gone.
  Three failures within ten minutes stop the restarts with a `manager.stopped` warning until you press **Start
  now**. A restart resumes the conversation when the provider can: MAP starts it with `session_id_args`
  (`--session-id <uuid>` for Claude) and restarts it with `resume_args` (`--resume <uuid>`) once that transcript
  exists, with a one-line "MAP restarted this session" prompt instead of the instructions. Not `--continue`: in a
  shared folder the latest conversation may be a worker's.
- **A session like the planner's:** supervisor role, no task (the orchestrator never treats it as a worker), the
  watcher and the human protection apply. It is also a MAP-owned [lane](#lanes) (rate-limit wake, stall alerts,
  Telegram notices; never auto-cleared or nudged), and Telegram's `/ask` talks to it.
- **Instructions:** `backend/templates/manager-agent.md` (or `[templates] manager_agent`) plus "This session" (the
  project, folder, cards, providers and models, orchestrator state, open tasks, plans waiting, lanes). It talks in
  your language, answers status questions from the API (never guessing), changes things only when asked, approves,
  cancels, deletes or pushes only on your explicit words and says what it did, turns implementation work into tasks
  (or points to the planner) unless you ask it to edit directly, keeps replies short and never prints a token. The
  file carries a curl cheat-sheet for every allowed call.
- **Its token.** `$MAP_MANAGER_TOKEN` is HMAC-SHA256 of the slug under `api_token` with its own label, so it never
  equals the project token (which the session also gets). It opens that project's owner-level routes only (see
  `api::manager_scope`): the project, overview, orchestrator status, tasks (list, create, edit a pending one, cancel,
  retry, start anyway, clarify, delete), plans (list, submit, approve, reject), pause/resume, knowledge (overview with
  the suggestion, accept, dismiss, reindex, search, doc), decisions, lanes (list, open, tell, start, screen, close
  without force), git status, fetch and push. Never MAP's settings, sign-ins, the API token, Telegram, other
  projects, provider installs or updates, its own manager settings, or deleting the project: those answer 403 (a
  token that is wrong answers 401).

### Persistent lanes

`POST /api/projects/{slug}/lanes` (`{name, provider?, model?, prompt?, clear_when?, status_prompt?}`, from you or the
manager) starts an interactive session `<session_prefix>lane-<slug>-<name>` in the project's folder on MAP's tmux
server and registers it as a lane with actions on and the project's lane rules (a `[[lanes]]` block watching the same
folder, else the defaults; `clear_when` off unless given). Lanes are stored in the database (`owned_lanes`) and
registered again at boot. One whose session is gone is shown as stopped with a **Start** button (`POST
…/lanes/{name}/start`, resuming its conversation like the manager), never relaunched by itself. `DELETE
…/lanes/{name}` closes it; it is refused while someone is attached or has typed on its input line, unless `?force=true`
from the owner's token.

## Writing tasks

A task is a title and a prompt. The orchestrator appends a protocol footer that tells the worker:

- its session name and task number;
- to write `{"status": "done", "summary": "…"}` to `$MAP_RESULT_FILE` when finished, or `"failed"` with a reason;
- which verify commands will run, so it runs them first;
- on a retry, the output of the check that failed.

Pin a task to a provider (and model) or leave it open for failover. Tasks with `depends_on` wait until those tasks
are `done`. If a dependency fails or is cancelled, its dependents show as blocked.

## Folder trust

Claude Code, Codex, agy, Cursor and Copilot ask once per folder whether you trust it (Claude: "Quick safety check: Is this a project
you created or one you trust?"). Nobody answers that in an unattended worker, so the task would sit there. Two layers
handle it (`backend/src/trust.rs`):

1. **MAP marks the folder trusted in the CLI's own config** (`[workspace] trust_folders = true`, the default), never
   touching any other key:
   - **Claude Code**: `projects["<folder>"].hasTrustDialogAccepted = true` in `$CLAUDE_CONFIG_DIR/.claude.json` when
     `CLAUDE_CONFIG_DIR` is set (in `[workspace.env]`, by [isolated agent logins](#agent-logins), or in MAP's own
     environment; the Docker deploy sets it to `~/.claude`), else `~/.claude.json`. Claude rewrites that file often, so MAP reads it, writes a temporary file next
     to it and renames it over the original (keeping its mode) only if the file did not change meanwhile, reads it back,
     and tries again if Claude's own write won. A file that does not parse is never written: a warning event, and
     layer 2 takes over.
   - **Codex**: `[projects."<folder>"] trust_level = "trusted"` in `$CODEX_HOME/config.toml` (else
     `~/.codex/config.toml`), edited with `toml_edit` so comments and layout stay. A folder you marked `untrusted` is
     left as it is.
   - **agy**: it keeps its answers in `trustedWorkspaces` of `~/.gemini/antigravity-cli/settings.json`, in a format MAP
     has not confirmed, so MAP does not write it and relies on layer 2.
   - **Copilot**: the folder is added to `trusted_folders` in `$COPILOT_HOME/config.json` (else
     `~/.copilot/config.json`); a folder below a listed one counts as trusted. The `// …` lines Copilot writes above
     the JSON are kept. Layer 2 answers "Yes, and remember this folder for future sessions".
   - **OpenRouter**: as Claude Code, in its own config folder, plus `hasCompletedOnboarding = true` when that file has
     no such key.
   - **Cursor**: where it records trusted workspaces is not documented, so MAP relies on layer 2 ("Trust this
     workspace"). Headless calls pass `--trust`.

   Folders are canonical absolute paths. MAP does this when a project is created (every source: an existing folder, a
   new one, a clone once it succeeds; the result is a check line such as `trusted /srv/app for claude, codex`, or a
   warning that never stops the creation), for `workspace.dir` at server start, and again before each agent starts
   (tasks, sessions, the init task; not the sign-in and update helpers), which is a no-op when the folder is already
   trusted. The first write for a folder and provider is a `folder.trusted` event. The project page shows, per CLI,
   whether its config trusts the folder, with **Trust folder** (`POST /api/projects/<slug>/trust`) when one does not.
   The files come from each provider's `trust` setting (see `map.example.toml`), not from code.
2. **A question that still shows is `needs_trust`.** Each provider has `trust_markers` (the question's text) and
   `trust_accept` (the option that trusts the folder). The watcher ranks it with `needs_auth`: the session needs a
   person. It is listed under "Needs you" with the folder; the orchestrator keeps the task running without a retry or
   a timeout (`decision.watch.trust`). **Trust this folder** on the session's card (`POST /api/sessions/<name>/trust`)
   acts only while the question is on screen and nobody is attached or typing: it records the folder as layer 1 does
   (when `trust_folders` is on), moves the selection to the trust option (Claude: Down from "No, exit"), checks the
   pane, and presses Enter. Text that merely quotes a marker, followed by the agent's own prompt, does not count.

Set `trust_folders = false` to keep MAP out of those files and answer the CLIs' trust prompts yourself; the button
still answers the question (the CLI then records it itself).

## Orchestrator decisions

Every branch the orchestrator takes for a task is an event `decision.<node>.<outcome>` with `task_id` set and the
detail it looked at as the message (`pinned: claude → claude`, `no dependencies · slot 2/4`,
`` `bun test` failed · 1 of 3 · 2 failing ``). The Decisions screen (`#decisions`, `#decisions/<task id>`) draws them as
a graph; `backend/src/decisions.rs` holds the kinds, the edge each takes and the rule text. A decision identical to
one of the task's last two is not logged again, so a task parked at a branch does not log every tick.

| Kind | Edge | Emitted in |
| --- | --- | --- |
| `decision.intake.ready` / `.unsure` | intake → ready | `intake::run`: Reflex sure it is light or heavy, or unsure / not running (as before) |
| `decision.intake.unclear` / `.plan_first` / `.duplicate` | intake (held) | Reflex sure it is unclear or a project to plan first, or a near-copy of a done task: waits for the owner |
| `decision.intake.shared_files` | intake | Reflex sure it changes shared files: runs alone, no analyst call |
| `decision.intake.released` | intake → ready | the owner's Start anyway |
| `decision.ready.go` | ready → pick | `tick`: dependencies done, slot free |
| `decision.ready.blocked` | ready → blocked | `tick`: a dependency failed or was cancelled |
| `decision.pick.chosen` / `.none` | pick → launch / stays | `start` (`pick_provider`); `none` while no provider is available |
| `decision.pick.tier_light` / `.tier_standard` / `.tier_heavy` / `.tier_abstained` | stays (progress) | `start` (`tier_gate`): the project's tier gate, before `pick.chosen` |
| `decision.launch.started` / `.error` | launch → watch / retry | `start` (`hub.launch`) |
| `decision.watch.done` | watch → verify | `check_running`: result file says done |
| `decision.watch.failed` / `.exited` / `.auth` / `.timeout` / `.lost` | watch → retry | `check_running` |
| `decision.watch.trust` | stays (progress) | `check_running`: the worker is at its CLI's folder-trust question; it waits for the owner (no retry, no timeout) |
| `decision.verify.cmd_start` / `.cmd_pass` | stays (progress) | `finish`: each `verify` command, `` `cmd` · 2 of 4 `` |
| `decision.verify.pass` / `.fail` | verify → audit / retry | `finish` |
| `decision.audit.off` / `.pass` / `.skipped` | audit → land | `finish` (`audit.enabled`, verdict, no auditor and not required) |
| `decision.audit.skipped_reflex` | audit → land | `finish` (`audit_gate`): Reflex is sure the change needs no audit |
| `decision.audit.reflex_said_audit` / `.reflex_abstained` | stays (progress) | `audit_gate`: Reflex said yes, or was unsure or not running; the audit runs |
| `decision.audit.fail` / `.no_auditor` | audit → retry | `finish` |
| `decision.audit.ask` / `.unavailable` | stays (progress) | `audit`: each auditor of `audit.chain` |
| `decision.launch.worktree` / `.worktree_failed` / `.worktree_fatal` | stays / retry / failed | `start` (`worktrees::prepare`): the task's branch and worktree, a failed setup command, a folder that cannot have worktrees |
| `decision.land.shared` / `.nothing` / `.merged` / `.pushed` | land → post | `land`: no worktree, no commits beyond the base, base fast-forwarded, branch pushed (and a pull request) |
| `decision.land.committed` / `.rebased` | stays (progress) | `land`: MAP committed what the worker left; the base moved on, so the branch was rebased and is verified again |
| `decision.land.conflict` / `.reverify_fail` | land → retry | `land`: the rebase hit conflicts, or verify failed after it; the agent gets it back on the same branch |
| `decision.land.keep` / `.blocked` | land → review | `land`: `land = keep`, or a person must act (changes in the project folder, no remote, a failed push) |
| `decision.review.land` / `.send_back` | review → land / ready | the owner's Merge or Open pull request, or Send back with a note |
| `decision.post.cmd_pass` | stays (progress) | `finish`: each `post_done` hook |
| `decision.post.pass` / `.fail` | post → end / failed | `finish` |
| `decision.end.closed` / `.kept` | end → done | `finish` (`detect::end_decision` via `close_session`) |
| `decision.retry.again` / `.failed` / `.kept` | retry → ready / failed / watch | `retry_or_fail`, and `start` after a launch error |

The older `task.*` events (`task.started`, `task.retry`, `task.done`…) are still sent.

## Branches and worktrees

A project's **Branches** settings choose where its tasks run. **Shared folder** (the default) runs every task in the
project folder on whatever is checked out: safe one task at a time. **Branch + worktree per task** gives task N the
branch `map/task-N`, made from the base branch (`base`, or the branch checked out), in its own git worktree under
`<data_dir>/projects/<slug>/worktrees/task-N`. The worker, verify, audit and `post_done` all run there, so up to
`[orchestrator] max_parallel` tasks of one project work at once without touching each other's files.

- **setup** commands run in each new worktree before the agent starts (`bun install`); **copy** lists untracked files
  copied in from the project folder (`.env`), never committed.
- After the audit MAP commits what the worker left uncommitted (`Task #N: <title>`, trailer `MAP-Task: N`), then
  lands the branch by **land**: `merge` rebases onto the base (verifying again if the base moved on) and
  fast-forwards it, one landing at a time per project (where the base is checked out, as a `merge --ff-only` there,
  so the owner's own uncommitted edits are never overwritten: a clash parks the task in `review`); a rebase conflict
  goes back to the agent on the same branch. `pull_request` pushes the branch (`--force-with-lease`, it is MAP's) and
  opens a pull request with `gh` or a merge request with `glab`. `keep` leaves it in `review`.
- A task in `review` waits for the owner: Merge, Open pull request, Send back (with a note for the agent) or Discard.
  Retries and send-backs reuse the worktree. Landed worktrees are removed, and merged branches deleted; a pushed
  branch stays for its pull request.
- Choosing the mode shows what it costs the machine (`POST /api/projects/{slug}/worktree-estimate`): the checkout and
  the ignored build folders each worktree repeats, the agent CLIs' memory (measured from running ones), a rule of
  thumb for the verify build, CPU at peak, how many tasks at once fit and what limits it, how many rounds the open
  tasks take one at a time and N at a time, and the drawbacks: conflicts and reruns, plan limits reached N× sooner,
  builds competing for the CPU, cold worktrees, shared ports and services.

## Parallel tasks

How many tasks run at once is set at two levels, both applied at the next orchestrator tick (5 s) without a
restart:

- **Machine-wide** (`max_parallel`): `[orchestrator] max_parallel` in map.toml, or the value set in the Command centre
  (`POST /api/orchestrator {"max_parallel": N}`, kept in the database across restarts; `0` goes back to map.toml's).
- **Per project** (Settings → Parallel, `parallel.max`): the most of that project's tasks at once; empty is only the
  machine-wide limit.

Independent tasks (no `depends_on` between them) start together up to those limits. In a **worktree** project every
task has its own branch, so nothing else is needed. In a **shared-folder** project (one folder, one branch) a second
task only starts beside a running one when the **parallel analyst** has cleared all of them: an agent run headless
(the project's `parallel.analyst_provider`, else its audit provider, else the first available with a headless
command) reads the tasks and the code read-only and answers, per task, the paths it will change and whether it is
independent (no shared files such as lockfiles, migrations, generated code, shared config). Tasks are cleared when
each is independent and their paths do not overlap (a folder covers what is under it; no paths means the whole repo).
The analyst runs on its own when tasks wait for it (not again for the same tasks within 10 minutes), or with
`POST /api/projects/{slug}/parallel/analyze`; an agent can also declare scopes with its project token
(`POST /api/projects/{slug}/scopes`). A cleared worker is told its paths and not to commit, stash, reset or reformat
the repo. With `parallel.analyst = false` a shared-folder project runs one task at a time. The default workspace
follows the shared-folder rules. Decisions: `decision.ready.project_full`, `.shared_busy`, `.awaiting_analyst`,
`.cleared`.

## Knowledge

The Knowledge screen (`#knowledge/<slug>`) indexes a project's docs: the Markdown and text files under
`knowledge.dirs` (`.docs`, `docs`, `tasks` by default; `.scratch` and build folders are skipped) and the rulebooks
(`CLAUDE.md`, `AGENTS.md`), plus what MAP remembers itself: each finished or failed task (what was asked, the
outcome, what went wrong on the way: verify and audit failures, conflicts, send-backs) and each approved plan, as
`map://tasks/<id>` and `map://plans/<id>`. The index is SQLite FTS5 with the trigram tokenizer, so a search matches
substrings in Thai as well as English (terms of three characters or more; shorter ones fall back to a plain scan); it
is rebuilt when older than two minutes, or with Reindex.

**Should this project have knowledge yet?** The overview carries a suggestion (`KnowledgeOverview.suggestion`). A
project with no index is **recommended** to set one up once it has 30 or more source files (or 3000 lines; build,
dependency and scratch folders and dot-folders not counted), 3 tasks done (their lessons are only in MAP's memory),
10 docs with no index, or a rulebook over its warn budget; smaller projects get **not yet** with the numbers, since
cards would cost more than they save. The screen shows the reasons and what accepting does. **Set up knowledge**
(`POST /api/projects/{slug}/knowledge/accept`) queues one task, once, whose prompt is MAP's knowledge practice: survey
the docs, rulebooks, code layout and past tasks (MAP's memory, through `$MAP_KNOWLEDGE_URL`); write one card per area
under `.docs/knowledge/domain/` (business rules) and `ops/` (running, deploying, debugging), 40–120 lines, at most the
cap, each with `sources:`; write the index with the conventions and one row per card; point the rulebooks at the
index and move their long reference sections into cards; no code changes, no secrets, one commit. **Not now**
(`…/knowledge/dismiss`) hides it until asked again. While the task runs the suggestion says so; once the index exists
the knowledge is **in use**: every agent's prompt already tells it to search first, and the health checks below
watch it. The "no index" health finding's fix task uses the same prompt.

Health checks follow the team-process conventions (cards in `.docs/knowledge/` with an `index.md`, each card's
`sources:` frontmatter listing the files it summarises): a card is **stale** when one of its sources was committed
after it (git commit times), **missing source**, **over / near the line cap** (200 / 170), **not in the index**,
**broken relative links**, **rulebook over its byte budget** (60 KB / 52 KB: it is sent with every request),
**scratch folder** over 2 GB, and **copies of the repo inside a scratch folder** (a grep finds their stale docs). Every
finding with a fix has a ready task prompt; Create fix task queues it. All limits are per project (Settings →
Knowledge).

Agents search it too: a project task's launch script exports `MAP_KNOWLEDGE_URL`, the prompt tells the worker to
search before starting, and the project token opens `knowledge/search` and `knowledge/doc` (nothing else). Searches
are logged, by agent or owner, and shown on the screen. The team-process template seeds `.docs/knowledge/index.md`
with the conventions, and the init agent writes the first cards and their index rows.

## Reflex gates

Decisions the orchestrator makes for every task can be asked of Reflex first: local, model-free, a few milliseconds.
None of them ever calls an LLM to decide: whenever Reflex is not running, abstains, cannot read the Thai, or answers
below the gates' threshold (or there is no threshold yet), the orchestrator does exactly what it does with the gate
off. All are off by default; turn them on per project under **Settings → Checks → Reflex gates** (or in the wizard's
Checks step), stored as `settings.reflex_gates = { intake, audit, tier }`. Thai tasks are read through the project's
Thai glossary.

- **Intake gate** (before any LLM sees the task; `backend/src/intake.rs`). Once per pending task, whatever the free
  slots:
  1. A done task of the same project with 70% or more of the same text (character trigrams, so Thai and English
     alike) holds it as a **duplicate** of that task. A task it depends on does not count.
  2. One Reflex call asks `intake` (`unclear`, `plan_first`, `light`, `heavy`) and, in a shared-folder project with
     the parallel analyst on, `shared` (`shared`, `own`).
     - A confident **unclear** or **plan_first** holds the task for the owner: it stays `pending` with a "waiting for
       you" badge. **Start anyway** (`PATCH /tasks/{id}` `{"action": "start_anyway"}`), **Clarify**
       (`{"action": "clarify", "note"}` adds the note to the prompt and the gate looks again), open the planner, or
       cancel.
     - **light** and **heavy** are ready tasks, and the answer is the tier too (below).
     - A confident **shared** sets the task's scope to the whole repo, not independent (`by: reflex`): it runs alone
       in the folder, and the parallel analyst (an LLM) is never asked about it.
     - Unsure, below the threshold or not running: the task goes on as before.
  Events `decision.intake.ready | unsure | unclear | plan_first | duplicate | shared_files | released`, on the new
  first node **Intake** of the Decisions graph.
- **Model tier** (cheaper models for light work). For a task without a pinned model: a confident `light` or `heavy`
  (from the intake call when that gate is on, otherwise asked here with the same question) starts the worker on that
  tier's model for the chosen provider (`decision.pick.tier_<tier>`); anything else keeps the usual model (the
  project's model for the provider, else its default; `decision.pick.tier_abstained`). A pinned model is never
  changed, and providers without tier models are not asked. Tier models come from `map.toml`:

  ```toml
  # Built in for claude only; a [orchestrator.tiers.claude] table replaces it. A tier left out keeps the usual model.
  [orchestrator.tiers.claude]
  light = "haiku"
  standard = "sonnet"
  heavy = "opus"
  [orchestrator.tiers.codex]     # none built in for codex or gemini
  light = "gpt-6-luna"
  ```

  Each model must be in the provider's `models` list when that list is not empty.
- **Audit gate** (saves whole audit runs). When a task would be audited (audit on), the orchestrator asks
  `needs_audit` (`audit`, `no_audit`) with the task's title and prompt, the worker's summary and `git diff --stat`
  (against the task's base on its own branch, else `HEAD`; `git status --short` when there is no commit yet), each
  shortened. A confident **no_audit** accepts the task on verification and goes on to landing
  (`decision.audit.skipped_reflex`). Anything else: the audit runs as before (`decision.audit.reflex_said_audit` /
  `.reflex_abstained`).

**The gates' own Reflex.** The gates do not use the project's corpus: they have one Reflex of their own, shared by
every project, on `[reflex] gates_port` (default: the port just below `port_base`), with its corpus repo at
`<data_dir>/gates/corpus.git`, seeded on first start from `backend/templates/gates/`. Measured against the shipped
`reflex 0.2.4`: a choice question routes only between corpus domains named like its options, and a yes/no question
scored the same for every input. So the corpus is one domain per answer: `unclear`, `plan_first`, `light`, `heavy`,
`shared`, `own`, `audit`, `no_audit` (8, the most Reflex takes), each runbook prose of what such a task looks like.
Edit them in **Settings → Reflex gates** (`PUT /api/gates/file`; the domains stay the answers'); Reflex restarts on
save.

**Shadow first.** On a labelled set of 25 tasks, the seed corpus mostly abstained, and some of its confident answers
were wrong. So the gates start in shadow (`[reflex] gates_threshold` unset): every answer is logged on the project's
Reflex screen with its confidence, and none is acted on. Set a threshold (Settings → Reflex gates,
`POST /api/gates {"threshold": 0.02}`, kept across restarts; `null` goes back to map.toml's) once the logged answers
above it are right. Marking a gate decision right or wrong tells the gates Reflex's calibrator. Each gate question
is logged in the project's decision log like any `/decide` call, with route `reflex_gated`: the fallback is never
asked.

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
| POST | `/sessions/{name}/trust` | `{}`: answer the CLI's folder-trust question (409 unless it is on screen and nobody is attached or typing): records the folder, moves the selection to the trust option, presses Enter; returns `folder`, `recorded`, `keys`, `answered` |
| GET/POST | `/tasks` | List, or create (`title`, `prompt`, `depends_on`, `provider`, `model`) |
| DELETE | `/tasks/{id}` | Delete a pending or finished task |
| GET/POST | `/orchestrator` | Status, or `{"running": true}` |
| GET | `/pipeline` | `provider_order`, `verify`, audit, `post_done` and retry settings from `[orchestrator]` |
| GET/POST | `/projects` | List, or create (`name`, `slug`, `description`, `fallback`, `model`, `settings`); returns the project and `checks` |
| GET/PATCH/DELETE | `/projects/{slug}` | Info (settings, Reflex status, decision counts), change (`threshold`, `fallback`, `model`, `questions`, `settings`, `paused`…; returns the `project` and `checks`, e.g. the gates Reflex's state when a gate is turned on), delete |
| GET | `/projects/defaults` | The `map.toml` values projects inherit (provider order, verify, audit, max_parallel, post_done, tier models, `trust_folders` and the CLIs it applies to) |
| GET/POST | `/projects/{slug}/trust` | Whether each installed CLI trusts the project's folder (read from its config); `{}` marks it trusted as creating the project does, returning `checks` and the new state |
| GET | `/templates` | Templates with their files and placeholders (built in, then `[templates] dirs`) |
| POST | `/projects/{slug}/init/plan` | `{"template"}`: per file `create`, `skip` (exists) or `append` (missing lines); writes nothing |
| POST | `/projects/{slug}/init` | `{"template", "description", "provider", "model"?, "set_verify"}`: writes the plan, optionally sets the project's verify to the template's, queues the "Init project" task; returns `plan`, `checks` and `task` |
| POST | `/projects/inspect` | `{"kind": "local"\|"empty", "location"}`: the folder's git branch, branches, commits and top-level entries, or whether a new folder can go there, after the same checks as creating (inside the project folder, `root` names it); `{"kind": "clone", "location": <url>, "branch"?}`: `git ls-remote` (default branch, branches, `hint` on an auth failure, suggested `folder`) |
| GET | `/fs/roots` | The project folder (or each of `[projects] roots`): `name`, `path`, `host_path`, `exists`, `allow_root_itself`, `is_default` |
| GET | `/fs/browse?root=&path=&hidden=` | Sub-folders of a folder inside a root: `path` (relative), `parent`, `abs_path`, `host_path`, `in_project`, `entries` (`name`, `kind`, `is_git`, `is_project`, `contains_project`, `has_children`, `hidden`), `truncated`, `files` (a count); 400 for an absolute path, `..` or a symlink out |
| GET | `/git/access` | MAP's public key and fingerprint (or `null`), git identity, known hosts, gitconfig path and includes, the project folder, the SSH method (`settings`) and `system_ssh` (`~/.ssh`: which files exist, `*.pub` names and fingerprints, ssh-agent socket, container) |
| PUT | `/git/settings` | `{"ssh": "map_key" \| "system"}`: the SSH method, saved in `<data_dir>/git/settings.json`; the gitconfig is regenerated |
| DELETE | `/git/known-hosts/{host}` | Forget a host learned on first use (`git.example.com`, `[host]:2222`); 409 for a pinned host, 404 when unknown; returns the known hosts left |
| GET | `/settings` | The Settings screen's read-only facts: server settings from `map.toml` (`api_token_set`, never the token), run mode and restart command, each agent CLI's binary path, `gh` / `glab` on PATH and whether `GH_TOKEN`, `GITHUB_TOKEN`, `GITLAB_TOKEN`, `GITLAB_HOST` are set (never their values) |
| POST | `/git/key` | `{"replace"?: bool}`: create MAP's ed25519 key (409 when one exists without `replace`; the old one is moved aside) |
| POST | `/git/test` | `{"host"}` (`github.com`, `git@host:2222`): `ssh -T` with MAP's key, or plain `ssh` with the `system` method; `ok`, output, the host's recorded keys |
| PUT | `/git/identity` | `{"name", "email"}` (both empty: none) |
| GET | `/projects/{slug}/git` | The folder's `is_repo`, branch, upstream, origin, ahead/behind, dirty count, last fetch and commit, `busy` |
| POST | `/projects/{slug}/git/fetch`, `…/pull`, `…/push` | `{}`: run it; returns `ok`, commands, output, new status (409 with the reason when refused) |
| POST | `/projects/{slug}/parallel/analyze` | `{}`: run the parallel analyst now over the project's unscoped open tasks; `started`, `reason` |
| POST | `/projects/{slug}/scopes` | `{"tasks": [{"id", "paths", "independent", "reason"}]}`: declare tasks' scopes (project token allowed) |
| GET | `/projects/{slug}/knowledge` | `KnowledgeOverview`: docs, counts by kind, health issues (with fix task prompts), rulebook budgets, scratch folders, recent searches; reindexes when older than 2 minutes |
| POST | `/projects/{slug}/knowledge/reindex` | Reindex now; returns the overview |
| POST | `/projects/{slug}/knowledge/accept` | Accept the knowledge suggestion: queues the setup task once (returns the `Task`) |
| POST | `/projects/{slug}/knowledge/dismiss` | `{"dismissed": bool}`: not now (or undo); returns the overview |
| GET | `/projects/{slug}/knowledge/search?q=&kind=&limit=` | Full-text search (trigram: Thai and English substrings); logged as by agent (project token) or owner. Project token allowed |
| GET | `/projects/{slug}/knowledge/doc?path=` | One indexed document's text (only paths in the index; `map://` memory). Project token allowed |
| POST | `/projects/{slug}/worktree-estimate` | `{"parallel"?, "copy"?, "setup"?}`: what N tasks in worktrees would cost this machine (`WorktreeEstimate`) |
| PATCH | `/tasks/{id}` | `{"action": "retry" \| "cancel" \| "merge" \| "pull_request" \| "send_back" \| "discard" \| "start_anyway" \| "clarify", "note"?}`; merge / pull_request / send_back from `review` only, start_anyway for a task the intake gate holds, clarify (with the note) for a pending task (409 otherwise) |
| POST | `/projects/{slug}/git/remote` | `{"url"}`: set or replace `origin` (`git init -b main` first when not a repo) |
| GET/POST | `/providers/{name}/login` | Sign-in state (parsed URL, one-time code, credential check), or start the helper login session |
| GET | `/projects/{slug}/repos/corpus` | Files and recent commits |
| GET/PUT | `/projects/{slug}/repos/{repo}/file` | `?path=` read; `{"path", "content", "message"}` commit (`content: null` deletes) |
| POST | `/projects/{slug}/decide` | `{"state", "questions", "route": "auto"\|"reflex"\|"llm"}`; each answer has `lane`; Thai is glossed for Reflex |
| GET | `/projects/{slug}/thai` | `ThaiReport`: the project's Thai words, the built-in count, recent Thai decisions read and the unknown fragments |
| POST | `/projects/{slug}/thai/preview` | `{"text", "words"?}`: what Reflex reads for it (`ThaiPreview`: English, coverage, unknown) |
| PUT | `/projects/{slug}/thai/words` | `{"words": {thai: english \| "-"}}`: replace the project's Thai words |
| GET/POST | `/gates` | The gates' own Reflex (`GatesInfo`); POST `{"threshold": number \| null}` sets the threshold (null: map.toml's; unset: shadow) |
| POST | `/gates/restart` | Restart the gates Reflex |
| GET/PUT | `/gates/file` | `?path=` read one domain file of the gates corpus; PUT `{"path", "content"}` saves it and restarts |
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
| GET | `/reflex/savings?hours=24\|168\|720&project=` | Tokens Reflex saved vs spent by the fallbacks: totals, per project, per UTC day, method (measured/estimated/mixed); `project` empty = all |
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
| GET | `/permissions?project=` | `PermissionsView`: providers, rule rows with a cell per CLI (value, source, note), the files read; no project = machine scope |
| PUT | `/permissions` | `PermissionsUpdate` `{"project"?, "cells": [{"rule", "provider", "value": "allow" \| "ask" \| "deny" \| null}]}`: set or clear; returns the view |
| GET | `/mcp?project=` | `McpView`: MCP servers across the CLIs (env and header names only), sources, last test status, the files read |
| PUT | `/mcp` | `McpSave` `{"project"?, "server": {name, transport, command, args, url, env, headers}, "providers"}`: write for those CLIs, remove from the scope's others; returns the view |
| DELETE | `/mcp` | `{"project"?, "name"}`: remove it from every file of the scope (404 when none had it); returns the view |
| POST | `/mcp/test` | `{"project"?, "name"}`: `McpTestResult` `{ok, tools, tool_names, error, duration_ms}` (15 s limit) |
| GET | `/usage` | The latest usage report: windows, pace, tokens today, by project, skip state per provider |
| POST | `/usage/refresh` | `{}`: read every provider now and return the report |
| GET | `/updates` | The latest update check: installed and latest version, errors, update command and whether it is blocked, per provider |
| POST | `/updates/refresh` | `{}`: check every provider now and return the report |
| POST | `/providers/{name}/update` | `{}`: run the provider's `update` command in `<prefix>update-<name>` (409 while one runs or when the CLI is read-only); returns the session |
| GET/POST | `/projects/{slug}/plans`, `/workspace/plans` | The project's (default workspace's) plans, newest first; or submit a draft (`title`, `summary`, `tasks: [{key, title, prompt, depends_on, provider?, model?}]`, `replaces?`, `session?`). The planner's token works here |
| GET | `/plans?project=` | Plans; `project` absent = all, empty = default workspace |
| GET/PATCH | `/plans/{id}` | A plan; edit a draft (`title`, `summary`, `tasks` replaces the list) |
| POST | `/plans/{id}/approve` | `{}`: create the draft's tasks in dependency order; returns the plan and the tasks (409 when not a draft) |
| POST | `/plans/{id}/reject` | `{"reason"?}` |
| GET/POST | `/projects/{slug}/planner`, `/workspace/planner` | The planner's session and default provider; or start it (`{"provider"?, "model"?}`; returns the running one if live) |
| GET | `/planner/instructions` | The planner's instructions and their source (built in or `[templates] planner_agent`) |
| GET | `/lanes` | Every lane (`LaneInfo`): settings, sessions with state, context %, rate-limit line, stall, pause; status file and age, card counts, git, queued tells, history |
| PATCH | `/lanes/{name}` | `{"watch"?, "actions"?, "clear_enabled"?, "status_prompt"?}`: kept over map.toml; returns the lane |
| GET | `/lanes/{name}/screen?session=&lines=` | The last capture's tail, ANSI stripped (default: the lane's first session, 60 lines) |
| POST | `/lanes/{name}/tell` | `{"verb": "clear" \| "ask" \| "pause", "note"?, "mins"?, "session"?}`; `{name}` may be a session a lane watches (what `map-tell` sends) |
| GET | `/lanes/{name}/tells`, `/lanes/tells` | Queued tells of the lane (or session), or of every lane |
| DELETE | `/lanes/{name}/tells/{verb}?session=` | Cancel a queued tell (no session: the "any session" one) |
| POST | `/lanes/import-watcher` | `{"path"}`: fill lane settings and the Telegram chat and users from an agent-watcher `config.json` inside your home (never the token); returns `updated`, `skipped` |
| GET | `/telegram` | `enabled`, `token_set` (never the token), `token_env`, `chat_id`, `allowed_user_ids`, `commands`, `polling`, `last_send_ok`, `last_error` (redacted), last send and report |
| POST | `/telegram/test` | `{}`: send a test message; `ok`, `message` |
| GET/PUT | `/projects/{slug}/manager` | The manager's settings, state, warning, session and default provider; `PUT {enabled, provider?, model?, force?}` (owner only) |
| POST | `/projects/{slug}/manager/start` | `{}`: start it now, clearing the back-off (owner only) |
| GET | `/manager/instructions` | The manager's instructions and their source |
| GET | `/projects/{slug}/overview` | The project, orchestrator status, task counts, open tasks, recent done, draft plans, sessions, lanes and the manager |
| GET | `/projects/{slug}/orchestrator` | The orchestrator's status (read only) |
| POST | `/projects/{slug}/pause`, `/projects/{slug}/resume` | `{}`: pause or resume the project |
| GET/POST | `/projects/{slug}/tasks?status=&limit=` | The project's tasks, newest first; or create one in it (`title`, `prompt`, `depends_on`, `provider?`, `model?`) |
| GET/PATCH/DELETE | `/projects/{slug}/tasks/{id}` | A task of the project; `PATCH {action, note?}` as `/tasks/{id}` (the manager: cancel, retry, start_anyway, clarify only) or `{title?, prompt?}` to edit a pending one; delete |
| GET | `/projects/{slug}/tasks/{id}/trace` | The task's decisions |
| POST | `/projects/{slug}/plans/{id}/approve`, `…/reject` | Decide a draft of this project |
| GET/POST | `/projects/{slug}/lanes` | The project's lanes (MAP's own and `[[lanes]]` watching its folder); or open a persistent lane |
| DELETE | `/projects/{slug}/lanes/{name}?force=` | Close a persistent lane (409 while used; `force` is the owner's) |
| POST | `/projects/{slug}/lanes/{name}/start`, `…/tell` | Start a stopped persistent lane; queue a tell |
| GET | `/projects/{slug}/lanes/{name}/screen?lines=` | The lane's screen tail |
| GET | `/projects/{slug}/activity?since=&all=&task=` | The task flow graph (`nodes`, `edges`) for the Activity tab |
| GET | `/sessions/{name}/trace` | One session's activity items from its transcript (owner only) |
| GET | `/tasks/page?status=&project=&q=&limit=&offset=` | One page of tasks (`{items: [{task, started_at}], total}`), newest change first; `project` empty = default workspace, `q` a title or `#id`. `GET /tasks` takes the same filters; with none it returns every task as before |
| POST | `/providers/{name}/login/code` | `{"code"}`: type the pasted authorization code into the sign-in session, only while its pane asks for one (409 otherwise) |
| GET | `/ws` | Pushes `ServerMessage` JSON (see `frontend/lib/generated/ServerMessage.ts`) |

Git: `git clone http://127.0.0.1:4400/git/<slug>/corpus.git`. With `api_token` set, git asks for a password: use the
token (any user name). Only corpus repos are served; a project's code stays in its folder.

## Security

- Binds to `127.0.0.1` by default and warns if it binds wider without a token.
- The `Host` header must be loopback, the bound address or a name in `allowed_hosts`. This blocks DNS rebinding.
- Browsers do not apply CORS to WebSockets, so `/api/ws` checks `Origin` itself.
- State-changing routes take JSON, which a cross-site form cannot send without a CORS preflight.
- Prompts are typed with `send-keys -l`, so their text is never read as key names. Only allow-listed keys can be sent.
- Agent tokens are scoped: a project token opens its decisions, corpus, plans and knowledge search; a manager token
  its project's owner-level routes (see [Project manager](#project-manager)); either answers 403 elsewhere.
- Only sessions under `session_prefix` are managed. Outside it, MAP reads only the `[[lanes]]` you declare, types into
  them only as [Lanes](#lanes) describes, and never creates, kills, renames or resizes them.

Agents run with whatever permissions their flags grant (e.g. `--dangerously-skip-permissions`). Run the platform as
a user, and against a workspace, you are comfortable letting agents change.

## Tests

```sh
cd backend && cargo test && cargo clippy --all-targets   # also regenerates frontend/lib/generated
cd frontend && bun run lint && bunx next typegen && bunx tsc --noEmit && bun run build
```

Backend tests cover folder trust (Claude's JSON and Codex's TOML in temporary `CLAUDE_CONFIG_DIR` / `CODEX_HOME`
folders only; tests refuse to write anywhere else), reset-time parsing, pending-input detection (faint suggestions and the cursor are not typing), the
wake-up cycle, the orchestrator's dependency order, retries and human protection, its Reflex gates (against a fake
Reflex on a local port), and the store, all against an in-memory terminal. Git access tests make a key in a temporary
data dir (skipped without `ssh-keygen`) and clone, fetch, pull and push only against local bare repos over `file://`;
nothing contacts GitHub or GitLab. Settings tests check the SSH method in `settings.json` and a gitconfig without
`sshCommand`, that a pinned known host cannot be removed and a learned one can, that `/api/settings` never carries
the API token or a variable's value, and the container check against marker files in a temporary folder. Project
folder tests cover `path`/`root`/`roots` parsing and validation, inside/outside checks (symlink escapes, `..`, the
folder itself, new folders under missing sub-folders), the picker's listing (folders only, dot-folders, git and
project badges, the cap, traversal refused), creation refused outside it for every source, and an older project
outside it still loading with the warning. Agent-login tests resolve every provider's folder in both modes, check
that launch scripts export `CLAUDE_CONFIG_DIR` / `CODEX_HOME` / `HOME` in isolated mode and nothing extra in shared
mode, and that credentials, usage, skills, agents and trust paths follow the folder (all in temporary folders); the
tmux backend's command builder is tested for `-L <socket>` and for dropping `TMUX` / `TMUX_PANE`, without running tmux.
Lane tests port agent-watcher's `selftest-ratelimit.py` cases (reset times, the limit line, resume, stalls, never
clearing while parked) and cover the clear and prompt rules, tells, card notices once per card and the watcher import.
One test runs a fake Claude pane on a private tmux server (`tmux -L map-lanes-test-<pid>`, killed afterwards; skipped
without tmux) and checks that `/clear` and the status prompt are typed only once it is idle. Telegram tests use a
fake Bot API on a local port: chunking, retries, token redaction, the allowed-user filter, the offset, `/watch` and
the 409 back-off.

## Roadmap

- Git worktree per task, so `max_parallel > 1` workers cannot edit the same files.
- Fail over a rate-limited task to another provider when its reset is far away.
- A usage reader for `agy` once it exposes its quota locally.
- Agent-to-agent messages: a supervisor session that creates and reviews tasks through the API.
- Activity feed parsed from agent panes (prompts, tool calls, replies), as the watcher's live feed did.
