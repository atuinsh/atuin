-- Content-bearing trigram index for messages containing CJK text.
-- Rows without CJK text are never inserted here, avoiding trigram index bloat
-- for Latin-only users while enabling substring matching for Chinese/Japanese/Korean.
CREATE VIRTUAL TABLE IF NOT EXISTS messages_cjk_fts USING fts5(
    title,
    body,
    tokenize = 'trigram'
);

-- Tracks backfill state for messages_cjk_fts: 0=pending, 1=completed.
CREATE TABLE IF NOT EXISTS cjk_state (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    backfilled INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO cjk_state (id, backfilled) VALUES (0, 0);
