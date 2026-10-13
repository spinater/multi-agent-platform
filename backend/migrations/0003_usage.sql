-- Plan-usage readings per provider window, for the pace projection (rate over the last hour of samples).
-- Times are Unix milliseconds. `resets_at_ms` tells window instances apart: a new value means the window reset.
CREATE TABLE usage_samples (
    provider TEXT NOT NULL,
    window_id TEXT NOT NULL,
    at_ms INTEGER NOT NULL,
    used_pct REAL NOT NULL,
    resets_at_ms INTEGER,
    PRIMARY KEY (provider, window_id, at_ms)
);

CREATE INDEX usage_samples_at ON usage_samples (at_ms);
