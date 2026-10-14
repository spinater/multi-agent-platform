-- The parallel analyst's verdict per task (model::TaskScope, JSON), and settings changed from the dashboard at runtime
-- (e.g. orchestrator.max_parallel), which replace the map.toml value until cleared.
ALTER TABLE tasks ADD COLUMN scope TEXT;
CREATE TABLE runtime_settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
