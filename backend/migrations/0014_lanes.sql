-- Lanes (lanes.rs): tmux sessions someone else owns that MAP watches.
-- The dashboard's (and /watch's) changes to a lane, over its map.toml block (JSON, lanes::LaneOverride).
CREATE TABLE lane_overrides (
    lane TEXT PRIMARY KEY,
    data TEXT NOT NULL
);
-- What MAP remembers per lane session across restarts (JSON, lanes::SessionMem); session '' is the lane itself.
CREATE TABLE lane_memory (
    lane TEXT NOT NULL,
    session TEXT NOT NULL,
    data TEXT NOT NULL,
    PRIMARY KEY (lane, session)
);
-- Queued instructions (POST /api/lanes/{name}/tell); one per lane, session and verb (a new one replaces it).
CREATE TABLE lane_tells (
    lane TEXT NOT NULL,
    session TEXT NOT NULL,
    verb TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    mins INTEGER,
    at TEXT NOT NULL,
    PRIMARY KEY (lane, session, verb)
);
-- What MAP did or noticed in each lane.
CREATE TABLE lane_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    lane TEXT NOT NULL,
    session TEXT,
    kind TEXT NOT NULL,
    message TEXT NOT NULL,
    at TEXT NOT NULL
);
CREATE INDEX lane_history_lane ON lane_history (lane, id);
