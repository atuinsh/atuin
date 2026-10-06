CREATE TABLE output_uploads (
    user_id TEXT NOT NULL,
    history_id TEXT NOT NULL,
    queued_at INTEGER NOT NULL DEFAULT (unixepoch()),
    PRIMARY KEY (user_id, history_id)
) WITHOUT ROWID;
