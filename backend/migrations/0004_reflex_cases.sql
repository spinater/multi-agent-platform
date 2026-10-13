-- Reflex screen: a project's labeled calibration cases, with Reflex's score from the last time each was run, and
-- the user's verdict on a logged decision.
CREATE TABLE reflex_cases (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL REFERENCES projects (slug) ON DELETE CASCADE,
    question_id TEXT NOT NULL,
    text TEXT NOT NULL,
    -- JSON: the correct option text, or a boolean for a yes/no question.
    expected TEXT NOT NULL,
    created_at TEXT NOT NULL,
    -- Reflex's answer (JSON), confidence and verdict when the case was last scored; NULL until then.
    answer TEXT,
    confidence REAL,
    correct INTEGER,
    scored_at TEXT
);

CREATE INDEX reflex_cases_project ON reflex_cases (project, id);

-- 1 = the answer was right, 0 = wrong, NULL = no feedback.
ALTER TABLE decisions ADD COLUMN feedback INTEGER;
