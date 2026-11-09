# Multi-Agent Platform: working notes for agents

Rust backend (`backend/`, axum + SQLite + tmux) and a Next.js dashboard (`frontend/`, Bun, static export served by the backend).
Read `README.md` for the architecture.

## Commands

- Backend: `cd backend && cargo test && cargo clippy --all-targets` (tests regenerate `frontend/lib/generated/*.ts`; commit them).
- Frontend: `cd frontend && bun run lint && bunx next typegen && bunx tsc --noEmit && bun run build`.
- Run: `cd backend && cargo run -- serve -c ../map.toml` (copy `map.example.toml` first).

## Rules

- `backend/src/model.rs` is the API contract. Change it there, run `cargo test`, then use the regenerated types in the frontend; never hand-edit `lib/generated`.
- All state changes go through `Hub` (`put_session`, `put_task`, `event`) so the database and the WebSocket stay in sync.
- Never act on tmux sessions outside `workspace.session_prefix`, and never end a session someone is attached to or typing in (`detect::end_decision`), unless the user forces it.
- Lanes (`lanes.rs`) are the one exception: sessions declared in `[[lanes]]` are read, and typed into only through `LaneTerm` (list, capture, type a line) when idle with nobody attached; never create, kill, rename or resize one. The one exception to idle-and-unattended is the owner's own chat (`lane_chat.rs`): a message or key the owner sends from the dashboard is typed at once (never into an open dialog or over unsent text). MAP's own lanes (a project's manager, persistent lanes; `manager.rs`) are MAP sessions on its socket, started and closed through `Hub` like any other.
- Never log or return the Telegram token; pass Bot API errors through `telegram::redact`.
- Paths, commands, session names and providers come from `map.toml`, not code.
- The frontend's Next.js is newer than most training data: read `frontend/node_modules/next/dist/docs/` before using an unfamiliar API.
