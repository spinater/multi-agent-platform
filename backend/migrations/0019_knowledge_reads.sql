-- Document reads are logged next to searches (knowledge_log.rs), each with the tokens MAP returned, so the Knowledge
-- screen shows what the project's knowledge costs the agents' context.
ALTER TABLE knowledge_queries ADD COLUMN kind TEXT NOT NULL DEFAULT 'search';
ALTER TABLE knowledge_queries ADD COLUMN tokens INTEGER NOT NULL DEFAULT 0;
