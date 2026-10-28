-- Sessions get an INTEGER PRIMARY KEY, and messages point at theirs by it rather than repeating
-- (harness, session_id) in the table and every index. Both tables have one so VACUUM cannot
-- renumber rowids: messages_fts is keyed by messages.rowid, and messages.session by sessions.id.
CREATE TABLE sessions (
    id INTEGER PRIMARY KEY,
    harness INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    -- The session's id across hosts and harnesses (`AtuinSessionId`): the smallest any of its
    -- rows carries.
    atuin_id BLOB NOT NULL,
    parent_harness INTEGER,
    parent_session_id TEXT,
    -- How the session relates to its parent (`ParentKind`: 0 subagent, 1 fork, 2 continuation),
    -- so readers can tell a subagent's fragment from a conversation a person carried on. NULL
    -- when the harness did not say.
    parent_kind INTEGER,
    -- The top-most stored ancestor, following parent links else the copy link: forks, subagents
    -- and copies group under it. A session whose parent is not stored (yet) is its own root, and
    -- is regrouped once it arrives.
    root_harness INTEGER,
    root_session_id TEXT,
    -- The session a parentless one was copied from, inferred from the model calls they share:
    -- Claude Code's `--fork-session` (and a `--resume` it turns into a fork) copies the history
    -- into a new session with only `sessionId` rewritten, naming the original nowhere, so capture
    -- has no parent to record. A call's `turn_id` is the harness's own id for it, so one held by
    -- two sessions was copied from one into the other. Only an id the harness gave the call
    -- counts, not one capture derived from a line's content, which unrelated sessions can share
    -- (see `linkable_turn!`).
    --
    -- It points at the lowest-ranked (earliest start, then id) other parentless session sharing a
    -- call, when that ranks below this one: so links only ever go down in rank and cannot cycle,
    -- the group's root is the session that owns the shared calls (see `attribute_call`), and the
    -- result depends only on the rows stored. A session with a parent (a subagent, a `/branch`
    -- fork) groups by it instead, and is never a link's target.
    copy_of_session_id TEXT,
    cwd TEXT,
    git_branch TEXT,
    model TEXT,
    -- The host the session was captured on: its earliest row's (see `messages.host`).
    host_id TEXT,
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
    -- The session's most recent assistant reply, so a listing can say how each session ended
    -- without the reader opening every transcript. `last_reply_at` orders replies that arrive out
    -- of order (a reprojection replays records in record order, not message order).
    last_reply TEXT,
    last_reply_at INTEGER,
    UNIQUE (harness, session_id)
);

-- Sessions are listed newest first, often for one harness and only those active since a given
-- time; without these every listing reads and sorts the whole table.
CREATE INDEX sessions_harness_updated_at ON sessions (harness, updated_at);
CREATE INDEX sessions_updated_at ON sessions (updated_at);
CREATE INDEX sessions_root ON sessions (root_harness, root_session_id);
CREATE INDEX sessions_parent ON sessions (parent_harness, parent_session_id);
CREATE INDEX sessions_copy_of ON sessions (harness, copy_of_session_id)
WHERE copy_of_session_id IS NOT NULL;

-- Text that many message rows repeat (roles, directories, branches, models, hosts), stored once:
-- a session's rows mostly share one of each, so a row refers to its text by id. Rows here are
-- never deleted; there are only ever a few hundred.
CREATE TABLE interned (
    id INTEGER PRIMARY KEY,
    value TEXT NOT NULL UNIQUE
);

CREATE TABLE messages (
    seq INTEGER PRIMARY KEY,
    id BLOB NOT NULL UNIQUE,
    harness INTEGER NOT NULL,
    session INTEGER NOT NULL,
    source_id TEXT NOT NULL,
    parent_harness INTEGER,
    parent_session_id TEXT,
    parent_source_id TEXT,
    timestamp INTEGER NOT NULL,
    -- The role as its JSON (`interned`).
    role INTEGER NOT NULL,
    content TEXT NOT NULL,
    content_z BLOB,
    -- These and `host` are `interned` ids.
    cwd INTEGER,
    git_branch INTEGER,
    model INTEGER,
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
    -- The host the row was captured on (the record's host id, hyphenated), for the host filter
    -- and to tell a session recorded elsewhere from a local one.
    host INTEGER,
    UNIQUE (session, source_id)
);

CREATE INDEX messages_session_timestamp ON messages (session, timestamp);
-- Rows of one model call, across sessions (usage accounting) and within one (reasoning counts).
CREATE INDEX messages_harness_turn_session ON messages (harness, turn_id, session);

-- One model call, counted once whichever sessions its rows were copied into: the field-wise max
-- of every row reporting it, attributed to one owning session.
--
-- A call is counted once across every session holding it only when its `turn_id` is one the
-- harness gave it (see `linkable_turn!`). An id capture derived from a line's content (Codex
-- `token_count:`/`thread:`, Pi `line:`, opencode `<millis>:`) can be shared by unrelated sessions,
-- so such a call is counted once per group instead: once among the sessions holding it that share
-- a root (a fork copying its parent's history is grouped under it), and separately in each
-- unrelated group.
--
-- `scope` tells those apart: empty for a harness-given id, else the group's root as
-- `<root_harness>:<root_session_id>` (see `attribute_call`).
CREATE TABLE calls (
    harness INTEGER NOT NULL,
    turn_id TEXT NOT NULL,
    scope TEXT NOT NULL,
    session_id TEXT NOT NULL,
    usage_input INTEGER NOT NULL,
    usage_output INTEGER NOT NULL,
    usage_cache_read INTEGER NOT NULL,
    usage_cache_write INTEGER NOT NULL,
    usage_reasoning INTEGER NOT NULL,
    PRIMARY KEY (harness, turn_id, scope)
);

-- The sessions whose root changed since `AiSessionDatabase` last attributed afresh the
-- content-derived calls they hold, which are scoped to that root: filled by the trigger, wherever
-- sessions are regrouped, and emptied in the same transaction.
CREATE TABLE regrouped (session INTEGER PRIMARY KEY);

CREATE TRIGGER sessions_regrouped AFTER UPDATE OF root_harness, root_session_id ON sessions
WHEN OLD.root_harness IS NOT NEW.root_harness OR OLD.root_session_id IS NOT NEW.root_session_id
BEGIN
    INSERT OR IGNORE INTO regrouped (session) VALUES (NEW.id);
END;

CREATE TABLE checkpoints (
    harness INTEGER NOT NULL,
    session_id TEXT NOT NULL,
    "offset" INTEGER NOT NULL,
    -- The digest of the item the checkpoint was taken after, so a transcript rewritten under it
    -- or an event log reset since is noticed on resume.
    digest INTEGER NOT NULL,
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

-- How far the sidecar has been reprojected from each record series (one host's records of one
-- tag): every record at or below `idx` is projected. `record_id` is the id of the record at `idx`,
-- so a series rewritten under the watermark (a reset, deleted or re-keyed store) is noticed and
-- replayed from the start. No row means replay that series from the start.
CREATE TABLE reproject_watermark (
    host TEXT NOT NULL,
    tag TEXT NOT NULL,
    idx INTEGER NOT NULL,
    record_id TEXT NOT NULL,
    PRIMARY KEY (host, tag)
);

-- What the reproject watermarks were made against, in a single row.
--
-- `generation` counts invalidations: whatever clears watermarks or deletes projected rows bumps it
-- in the same transaction, and a reprojection moves a watermark only while the generation is still
-- the one it started from. So an invalidation landing mid-reprojection (a maintenance command, or
-- a replay forgetting a host) is noticed even for a series that had no watermark to lose.
--
-- `key_id` is the PASERK id (a public key identifier, not the key) of the encryption key the
-- watermarks were made with. A reprojection under a different key clears every watermark first:
-- what the old key projected, or held back, is no guide to what the new one can read.
CREATE TABLE projection_state (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    generation INTEGER NOT NULL,
    key_id TEXT
);

INSERT INTO projection_state (id, generation, key_id) VALUES (0, 0, NULL);
