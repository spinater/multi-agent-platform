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

Requirements: Rust 1.85+, Bun 1.2+, tmux, and the agent CLIs you want (`claude`, `codex`, `agy`).

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

## Writing tasks

A task is a title and a prompt. The orchestrator appends a protocol footer that tells the worker:

- its session name and task number;
- to write `{"status": "done", "summary": "…"}` to `$MAP_RESULT_FILE` when finished, or `"failed"` with a reason;
- which verify commands will run, so it runs them first;
- on a retry, the output of the check that failed.

Pin a task to a provider (and model) or leave it open for failover. Tasks with `depends_on` wait until those tasks
are `done`. If a dependency fails or is cancelled, its dependents show as blocked.

## API

All routes are under `/api`. When `server.api_token` is set, send `Authorization: Bearer <token>`.

| Method | Path | |
| --- | --- | --- |
| GET | `/overview` | Sessions, tasks, providers and orchestrator status in one call |
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
| GET | `/ws` | Pushes `ServerMessage` JSON (see `frontend/lib/generated/ServerMessage.ts`) |

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
- Usage and quota panels per provider (the watcher's `/usage` readers).
- Agent-to-agent messages: a supervisor session that creates and reviews tasks through the API.
- Activity feed parsed from agent panes (prompts, tool calls, replies), as the watcher's live feed did.
