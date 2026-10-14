-- Knowledge (knowledge.rs): each project's indexed docs, a trigram full-text index over them (substring search that
-- works for Thai as well as English), when each project was last indexed, and the searches agents and the owner ran.
CREATE TABLE knowledge_docs (
    project TEXT NOT NULL,
    path TEXT NOT NULL,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    lines INTEGER NOT NULL,
    bytes INTEGER NOT NULL,
    modified TEXT,
    sources TEXT NOT NULL,
    in_index INTEGER NOT NULL,
    PRIMARY KEY (project, path)
);
CREATE VIRTUAL TABLE knowledge_fts USING fts5(project UNINDEXED, path UNINDEXED, kind UNINDEXED, title, body, tokenize = 'trigram');
CREATE TABLE knowledge_meta (
    project TEXT PRIMARY KEY,
    indexed_at TEXT NOT NULL
);
CREATE TABLE knowledge_queries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    project TEXT NOT NULL,
    at TEXT NOT NULL,
    query TEXT NOT NULL,
    hits INTEGER NOT NULL,
    by_whom TEXT NOT NULL
);
CREATE INDEX knowledge_queries_project ON knowledge_queries (project, id);
