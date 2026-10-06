-- Messages point at their session by integer id rather than repeating (harness, session_id) in the
-- table and every index: a third of the sidecar was session ids.
--
-- Both tables get an INTEGER PRIMARY KEY so VACUUM cannot renumber rowids: messages_fts is keyed by
-- messages.rowid, and messages.session by sessions.id.

CREATE TABLE sessions_new (
    id INTEGER PRIMARY KEY,
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
    usage_reasoning INTEGER NOT NULL DEFAULT 0,
    title TEXT,
    title_source INTEGER,
    preview TEXT,
    UNIQUE (harness, session_id)
);

INSERT INTO sessions_new (
    harness, session_id, parent_harness, parent_session_id, cwd, git_branch, model, started_at,
    updated_at, message_count, usage_input, usage_output, usage_cache_read, usage_cache_write,
    usage_reasoning, title, title_source, preview
)
SELECT
    harness, session_id, parent_harness, parent_session_id, cwd, git_branch, model, started_at,
    updated_at, message_count, usage_input, usage_output, usage_cache_read, usage_cache_write,
    usage_reasoning, title, title_source, preview
FROM sessions;

CREATE TABLE messages_new (
    seq INTEGER PRIMARY KEY,
    id BLOB NOT NULL UNIQUE,
    harness INTEGER NOT NULL,
    session INTEGER NOT NULL,
    source_id TEXT NOT NULL,
    parent_harness INTEGER,
    parent_session_id TEXT,
    parent_source_id TEXT,
    timestamp INTEGER NOT NULL,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    content_z BLOB,
    cwd TEXT,
    git_branch TEXT,
    model TEXT,
    -- The usage this row reported, as the harness reported it. Every row of one model call may
    -- repeat (or grow) the same figures, so these are never summed directly: see `calls`.
    usage_input INTEGER NOT NULL DEFAULT 0,
    usage_output INTEGER NOT NULL DEFAULT 0,
    usage_cache_read INTEGER NOT NULL DEFAULT 0,
    usage_cache_write INTEGER NOT NULL DEFAULT 0,
    -- Reasoning tokens, part of output; NULL when the row did not break them out.
    usage_reasoning INTEGER,
    -- Distinguishes "no usage reported" from "zero tokens".
    usage_present INTEGER NOT NULL DEFAULT 0,
    stop_reason TEXT,
    -- The model call this row came from; groups the rows one response is split into, across
    -- every session a harness copied them into.
    turn_id TEXT,
    -- The title the row's own line set or cleared (JSON TitleChange), replayed on resume.
    title_change TEXT,
    UNIQUE (session, source_id)
);

-- Keeps each message's rowid, which its messages_fts row is keyed by.
INSERT INTO messages_new (
    seq, id, harness, session, source_id, parent_harness, parent_session_id, parent_source_id,
    timestamp, role, content, content_z, cwd, git_branch, model, usage_input, usage_output,
    usage_cache_read, usage_cache_write, usage_reasoning, usage_present, stop_reason, turn_id,
    title_change
)
SELECT
    m.rowid, m.id, m.harness, s.id, m.source_id, m.parent_harness, m.parent_session_id,
    m.parent_source_id, m.timestamp, m.role, m.content, m.content_z, m.cwd, m.git_branch, m.model,
    m.usage_input, m.usage_output, m.usage_cache_read, m.usage_cache_write, m.usage_reasoning,
    m.usage_present, m.stop_reason, m.turn_id, m.title_change
FROM messages m
JOIN sessions_new s ON s.harness = m.harness AND s.session_id = m.session_id;

DROP TABLE messages;
DROP TABLE sessions;
ALTER TABLE sessions_new RENAME TO sessions;
ALTER TABLE messages_new RENAME TO messages;

CREATE INDEX messages_session_timestamp ON messages (session, timestamp);
CREATE INDEX messages_harness_turn_session ON messages (harness, turn_id, session);
