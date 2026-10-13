CREATE TABLE sessions (
    name TEXT PRIMARY KEY NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    role TEXT NOT NULL,
    task_id INTEGER,
    state TEXT NOT NULL,
    detail TEXT,
    rate_limit_reset_at TEXT,
    wakeups INTEGER NOT NULL DEFAULT 0,
    last_line TEXT,
    attached INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    last_activity_at TEXT NOT NULL
);

CREATE TABLE tasks (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    title TEXT NOT NULL,
    prompt TEXT NOT NULL,
    depends_on TEXT NOT NULL DEFAULT '[]',
    provider TEXT,
    model TEXT,
    status TEXT NOT NULL,
    session TEXT,
    attempts INTEGER NOT NULL DEFAULT 0,
    result TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    at TEXT NOT NULL,
    level TEXT NOT NULL,
    kind TEXT NOT NULL,
    session TEXT,
    task_id INTEGER,
    message TEXT NOT NULL
);

CREATE INDEX events_at ON events (id DESC);
CREATE INDEX tasks_status ON tasks (status);
