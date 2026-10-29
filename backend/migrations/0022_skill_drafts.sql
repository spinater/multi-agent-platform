-- Finished tasks MAP has already looked at for a skill (skill_drafts.rs): one row each, with the plan it drafted.
CREATE TABLE skill_drafts (
    task_id INTEGER PRIMARY KEY,
    project TEXT NOT NULL,
    at TEXT NOT NULL,
    plan_id INTEGER
);
