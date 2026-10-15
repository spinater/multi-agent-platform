-- The activity graph (trace.rs): who made each task and session, and what MAP has read of each session's transcript.
ALTER TABLE tasks ADD COLUMN created_by TEXT NOT NULL DEFAULT 'owner';
ALTER TABLE sessions ADD COLUMN spawned_by TEXT NOT NULL DEFAULT '';
-- Per session: the transcript file, how far it was read, and the aggregated items (JSON, model::TraceItem[]).
CREATE TABLE session_traces (
    session TEXT PRIMARY KEY,
    file TEXT NOT NULL,
    offset INTEGER NOT NULL,
    data TEXT NOT NULL,
    at TEXT NOT NULL
);
