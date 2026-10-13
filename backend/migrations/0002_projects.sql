-- A project owns a code repo, a corpus repo and a Reflex engine under <data_dir>/projects/<slug>/.
CREATE TABLE projects (
    slug TEXT PRIMARY KEY NOT NULL,
    name TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    port INTEGER NOT NULL UNIQUE,
    threshold REAL,
    fallback TEXT NOT NULL,
    model TEXT,
    questions TEXT NOT NULL DEFAULT '[]',
    created_at TEXT NOT NULL
);

CREATE TABLE decisions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL REFERENCES projects (slug) ON DELETE CASCADE,
    at TEXT NOT NULL,
    state TEXT NOT NULL,
    question TEXT NOT NULL,
    answer TEXT NOT NULL,
    lane TEXT NOT NULL,
    confidence REAL,
    reflex_answer TEXT NOT NULL,
    ms INTEGER NOT NULL,
    error TEXT
);

CREATE INDEX decisions_project ON decisions (project, id DESC);

ALTER TABLE tasks ADD COLUMN project TEXT;
ALTER TABLE sessions ADD COLUMN project TEXT;
