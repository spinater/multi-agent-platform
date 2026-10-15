-- Lanes added from the dashboard to watch an existing tmux session (lanes/watch.rs; POST
-- /api/projects/{slug}/lanes/watch). map.toml is never edited, so they live here and are merged with the
-- [[lanes]] blocks (a block with the same name wins). MAP only reads and (when allowed) types into the session;
-- removing the lane never touches it.
-- `session_match` is a JSON array of globs; `socket` '' is tmux's default server; `path` defaults to the project
-- folder; `data` holds the other [[lanes]] keys (actions, watch, status_prompt, clear_when, ...) as JSON.
CREATE TABLE watch_lanes (
    name TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    role TEXT NOT NULL,
    socket TEXT NOT NULL DEFAULT '',
    session_match TEXT NOT NULL,
    path TEXT,
    data TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL
);
