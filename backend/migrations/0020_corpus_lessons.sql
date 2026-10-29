-- What a project's corpus learned (corpus_learn.rs): one row per trusted answer added to it, with what Reflex said
-- before it learned, the held-out score a first threshold is fitted from.
CREATE TABLE corpus_lessons (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL,
    at TEXT NOT NULL,
    decision_id INTEGER NOT NULL,
    domain TEXT NOT NULL,
    by_whom TEXT NOT NULL,
    reflex_answer TEXT,
    confidence REAL,
    correct INTEGER NOT NULL
);
CREATE INDEX corpus_lessons_project ON corpus_lessons (project, id);
