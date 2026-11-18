CREATE TABLE thread_message_uploads (
    user_id TEXT NOT NULL,
    record_id BLOB NOT NULL,
    queued_at INTEGER NOT NULL DEFAULT (unixepoch()),
    -- Stored, but this build can't read it yet: a key or a record version from later on. Only a
    -- restart loads a new key or build, so it is cleared at each start.
    held INTEGER NOT NULL DEFAULT 0 CHECK (held IN (0, 1)),
    PRIMARY KEY (user_id, record_id)
) WITHOUT ROWID;

-- How far each user's upload of one host's thread records has got: the records of the ai-session
-- series of `host_id` from `next_idx` up to, not including, `until_idx` are still to go.
-- `until_idx` is NULL until it is fixed at the series' tail. The row stays once done, so a series
-- is backfilled once.
CREATE TABLE thread_backfills (
    user_id TEXT NOT NULL,
    host_id BLOB NOT NULL,
    next_idx INTEGER NOT NULL,
    until_idx INTEGER,
    PRIMARY KEY (user_id, host_id)
) WITHOUT ROWID;
