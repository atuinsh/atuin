DROP TABLE history_uploads;
CREATE TABLE history_uploads (
    user_id TEXT NOT NULL,
    history_id TEXT NOT NULL,
    record_id BLOB,
    PRIMARY KEY (user_id, history_id)
) WITHOUT ROWID;

DROP TABLE history_deletions;
CREATE TABLE history_deletions (
    user_id TEXT NOT NULL,
    history_id TEXT NOT NULL,
    PRIMARY KEY (user_id, history_id)
) WITHOUT ROWID;
