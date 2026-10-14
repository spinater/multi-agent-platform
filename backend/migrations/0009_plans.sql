-- Plans: a requirement broken into tasks by a project's planner agent, waiting for the owner's approval.
-- Approving one creates its tasks; nothing runs before that.
CREATE TABLE plans (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    -- The project's slug; NULL is the default workspace (workspace.dir).
    project TEXT,
    title TEXT NOT NULL,
    summary TEXT NOT NULL DEFAULT '',
    -- draft | approved | rejected | superseded
    status TEXT NOT NULL,
    -- 1 for a first draft; a draft that replaces another is that one's revision + 1.
    revision INTEGER NOT NULL DEFAULT 1,
    -- The plan this one revises.
    replaces INTEGER,
    -- The planner session that submitted it.
    session TEXT,
    -- Why it was rejected.
    reason TEXT,
    created_at TEXT NOT NULL,
    -- When it was approved, rejected or superseded.
    decided_at TEXT
);

CREATE INDEX plans_project ON plans (project, id DESC);

CREATE TABLE plan_tasks (
    plan_id INTEGER NOT NULL,
    -- Order as submitted.
    position INTEGER NOT NULL,
    -- The planner's own id for the task ("t1"), unique in the plan; `depends_on` names these.
    key TEXT NOT NULL,
    title TEXT NOT NULL,
    prompt TEXT NOT NULL,
    -- JSON array of keys.
    depends_on TEXT NOT NULL DEFAULT '[]',
    provider TEXT,
    model TEXT,
    -- The task approving the plan created.
    task_id INTEGER,
    PRIMARY KEY (plan_id, key)
);
