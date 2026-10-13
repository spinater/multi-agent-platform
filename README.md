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
- **Projects.** A project is a code repo, a corpus repo and its own [Reflex](https://reflex.gist.rs) decision engine.
  - **Repos.** Both are bare git repos served at `/git/<slug>/code.git` and `/git/<slug>/corpus.git`: clone, push,
    or edit files from the dashboard. Tasks and sessions of a project run in a checkout of its code repo, which the
    orchestrator fast-forwards before each task.
  - **Decide before calling an LLM.** `POST /api/projects/<slug>/decide` asks Reflex first (local, no model, a few
    milliseconds). Answers at or above the project's confidence threshold are final; only the rest go to the
    project's fallback: the Anthropic API, or any provider run headless (`claude -p`, `codex exec`, `agy -p`).
    Agents started in a project get the endpoint as `$MAP_DECIDE_URL`, and the code repo's `CLAUDE.md` tells them
    to use it.
  - **Corpus.** One top-level folder per Reflex domain (1–8), prose `.md` files inside. A push or an edit restarts
    the project's Reflex on the new corpus.
  - **Import.** A project's code repo starts empty (README and CLAUDE.md), or is imported from a git URL or a local
    repo with `git clone --bare` (one branch, renamed `main`). Creating a project returns check lines: repos created,
    commits imported, workspace checked out, Reflex started or why not, each provider found and signed in.
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
| `backend/src/projects.rs` | Creating (new or imported) and deleting projects; repo and corpus path rules |
| `backend/src/project_settings.rs` | Per-project providers, verify, audit and pause, as the orchestrator asks for them |
| `backend/src/login.rs` | Provider sign-in state from credential files, the helper login session, URL and code parsing |
| `backend/src/repo.rs` | Bare repos written with git plumbing, imports (`clone --bare`), corpus export, the workspace checkout |
| `backend/src/reflex.rs` | One Reflex process per project: export the corpus, start, health-check, restart |
| `backend/src/usage.rs` | Plan-usage readers (Claude endpoint, Codex logs), token scans, pace, the skip line |
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
cp .env.example .env                        # your uid/gid, user, home, Node dir, MAP_DIR, port
mkdir -p ~/map-deploy/data/map && git init ~/map-deploy/data/workspace
cp deploy/map.docker.toml ~/map-deploy/map.toml
#   set api_token (openssl rand -hex 32), the PATH in [workspace.env], and allowed_hosts for your tunnel's name
docker compose up -d --build
docker compose logs -f map                  # open http://127.0.0.1:4400 and paste the token once
```

- Mounted from the host: `~/.claude`, `~/.codex`, `~/.gemini` (read-write: sign-ins, history, transcripts for Usage
  and Skills) and the CLIs in `~/.local/bin`, `~/.local/share/claude` and the Node install (read-only). Adjust
  `docker-compose.yml` if yours live elsewhere.
- Claude Code in the container uses `CLAUDE_CONFIG_DIR=~/.claude`, so its settings file is `~/.claude/.claude.json`
  (the host rewrites `~/.claude.json` by renaming over it, which a single-file mount cannot follow). Set
  `"hasCompletedOnboarding": true` in that file once, or the first interactive session stops at the setup screens.
  Each new folder still asks "trust this folder?" once; answer it in the session console.
- State lives in `MAP_DIR/data` (database, run files, project repos, workspaces); config in `MAP_DIR/map.toml`.
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
`#command/sessions/new`, `#projects/<slug>`. Every layout number lives in `TUNING` in
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

The **Reflex** screen (`#reflex`, `frontend/components/reflex/`) is the current project's decision engine: how it
works with the last 24 h per lane, a set-up checklist that ticks itself (install, corpus, questions, calibration,
playground), a playground with feedback, calibration on a log-scale scatter with a draggable threshold, and the
decision log. Confidence is always shown on a log axis (1e-5 … 1e-3, widened by whole decades when a corpus scores
outside it), never as a percentage.

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
| `decision.launch.started` / `.error` | launch → watch / retry | `start` (`hub.launch`, project workspace sync) |
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
| POST | `/projects/inspect` | `{"kind": "git"\|"local", "location", "branch"}`: branches, commits and top-level files before an import |
| GET/POST | `/providers/{name}/login` | Sign-in state (parsed URL, one-time code, credential check), or start the helper login session |
| GET | `/projects/{slug}/repos/{code\|corpus}` | Files and recent commits |
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
| GET | `/usage` | The latest usage report: windows, pace, tokens today, by project, skip state per provider |
| POST | `/usage/refresh` | `{}`: read every provider now and return the report |
| GET | `/ws` | Pushes `ServerMessage` JSON (see `frontend/lib/generated/ServerMessage.ts`) |

Git: `git clone http://127.0.0.1:4400/git/<slug>/code.git` (or `corpus.git`). With `api_token` set, git asks for a
password: use the token (any user name).

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
in-memory terminal.

## Roadmap

- Git worktree per task, so `max_parallel > 1` workers cannot edit the same files.
- Fail over a rate-limited task to another provider when its reset is far away.
- A usage reader for `agy` once it exposes its quota locally.
- Agent-to-agent messages: a supervisor session that creates and reviews tasks through the API.
- Activity feed parsed from agent panes (prompts, tool calls, replies), as the watcher's live feed did.
