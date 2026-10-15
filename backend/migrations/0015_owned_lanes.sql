-- Persistent lanes MAP opened in a project (manager.rs; POST /api/projects/{slug}/lanes), re-registered at boot.
-- `data` is model::OwnedLaneRecord as JSON.
CREATE TABLE owned_lanes (
    lane TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    data TEXT NOT NULL
);
