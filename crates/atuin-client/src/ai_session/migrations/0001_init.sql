CREATE TABLE messages (
    id BLOB PRIMARY KEY,
    harness INTEGER NOT NULL,
    session_id TEXT NOT NULL,
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
    UNIQUE (harness, session_id, source_id)
);

CREATE INDEX messages_harness_session_timestamp ON messages (harness, session_id, timestamp);
-- Rows of one model call, across sessions (usage accounting) and within one (reasoning counts).
CREATE INDEX messages_harness_turn_session ON messages (harness, turn_id, session_id);

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
    -- Usage attributed to this session: its rows without a model call, plus every call in
    -- `calls` it owns.
    usage_input INTEGER NOT NULL DEFAULT 0,
    usage_output INTEGER NOT NULL DEFAULT 0,
    usage_cache_read INTEGER NOT NULL DEFAULT 0,
    usage_cache_write INTEGER NOT NULL DEFAULT 0,
    usage_reasoning INTEGER NOT NULL DEFAULT 0,
    -- The newest row's title and where it came from (TitleSource: 0 summary, 1 generated,
    -- 2 named, 3 agent), so a resumed capture keeps ranking titles.
    title TEXT,
    title_source INTEGER,
    preview TEXT,
    PRIMARY KEY (harness, session_id)
);

-- One model call, counted once whichever sessions its rows were copied into: the field-wise max
-- of every row reporting it, attributed to one owning session.
CREATE TABLE calls (
    harness INTEGER NOT NULL,
    turn_id TEXT NOT NULL,
    session_id TEXT NOT NULL,
    usage_input INTEGER NOT NULL,
    usage_output INTEGER NOT NULL,
    usage_cache_read INTEGER NOT NULL,
    usage_cache_write INTEGER NOT NULL,
    usage_reasoning INTEGER NOT NULL,
    PRIMARY KEY (harness, turn_id)
);

CREATE TABLE checkpoints (
    harness INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    "offset" INTEGER NOT NULL,
    -- The digest of the item the checkpoint was taken after, so a transcript rewritten under it
    -- or an event log reset since is noticed on resume. A checkpoint without one is ignored: its
    -- session is read again from the start.
    digest INTEGER,
    PRIMARY KEY (harness, session_id)
);

-- Contentless (content=''): a full-content fts5 table would keep an uncompressed
-- copy of every searchable body in its `_content` shadow table, duplicating (and
-- dwarfing) the zstd-compressed `messages.content_z`. With content='' only the
-- inverted index remains; previews are derived in Rust from the stored message
-- content instead of via the fts5 aux functions (which need stored content).
-- contentless_delete=1 keeps plain DELETE / INSERT OR REPLACE working.
CREATE VIRTUAL TABLE messages_fts USING fts5(
    title,
    body,
    tokenize = 'unicode61',
    content = '',
    contentless_delete = 1
);
