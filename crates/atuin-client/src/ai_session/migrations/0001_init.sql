CREATE TABLE messages (
    id BLOB PRIMARY KEY,
    harness INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    parent_harness INTEGER,
    parent_session_id TEXT,
    parent_source_id TEXT,
    thread TEXT,
    timestamp INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    content_z BLOB,
    cwd TEXT,
    git_branch TEXT,
    model TEXT,
    usage_input INTEGER NOT NULL DEFAULT 0,
    usage_output INTEGER NOT NULL DEFAULT 0,
    usage_cache_read INTEGER NOT NULL DEFAULT 0,
    usage_cache_write INTEGER NOT NULL DEFAULT 0,
    stop_reason TEXT,
    UNIQUE (harness, session_id, source_id)
);

CREATE INDEX messages_harness_session_timestamp ON messages (harness, session_id, timestamp);

CREATE TABLE sessions (
    harness INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    parent_harness INTEGER,
    parent_session_id TEXT,
    cwd TEXT,
    git_branch TEXT,
    model TEXT,
    started_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    message_count INTEGER NOT NULL DEFAULT 0,
    usage_input INTEGER NOT NULL DEFAULT 0,
    usage_output INTEGER NOT NULL DEFAULT 0,
    usage_cache_read INTEGER NOT NULL DEFAULT 0,
    usage_cache_write INTEGER NOT NULL DEFAULT 0,
    title TEXT,
    preview TEXT,
    PRIMARY KEY (harness, session_id)
);

CREATE TABLE checkpoints (
    harness INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    "offset" INTEGER NOT NULL,
    PRIMARY KEY (harness, session_id)
);
