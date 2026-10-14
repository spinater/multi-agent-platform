-- Reflex savings: what each decision cost, or would have cost, in LLM tokens.
-- Length in chars of the fallback prompt (`decide::prompt`), for every decision: for a reflex-lane one it is what the
-- LLM would have been sent. NULL on rows from before this migration.
ALTER TABLE decisions ADD COLUMN prompt_chars INTEGER;
-- Tokens the fallback used; NULL when it was not asked (or failed).
ALTER TABLE decisions ADD COLUMN tokens_in INTEGER;
ALTER TABLE decisions ADD COLUMN tokens_out INTEGER;
-- 1 when the provider reported them (the API's `usage`), 0 when estimated (chars / 4).
ALTER TABLE decisions ADD COLUMN tokens_measured INTEGER NOT NULL DEFAULT 0;
-- The project's fallback at the time: api, a provider name, or none.
ALTER TABLE decisions ADD COLUMN fallback TEXT;
-- 1 when the state had Thai text, so Reflex was not asked.
ALTER TABLE decisions ADD COLUMN skipped_thai INTEGER NOT NULL DEFAULT 0;

-- The savings report reads every project's decisions by time.
CREATE INDEX decisions_at ON decisions (at);
