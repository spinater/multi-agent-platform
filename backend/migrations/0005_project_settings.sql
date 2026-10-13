-- A project's own settings (source, providers, verify, audit, paused) as `ProjectSettings` JSON.
-- '{}' is every field at its default: an empty source and the global orchestrator settings.
ALTER TABLE projects ADD COLUMN settings TEXT NOT NULL DEFAULT '{}';
