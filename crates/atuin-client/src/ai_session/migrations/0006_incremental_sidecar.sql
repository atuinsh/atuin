-- Incremental reprojection, the host each row was captured on, and session groups (forks,
-- subagents and copies under their root).

-- How far the sidecar has been reprojected from each record series (one host's records of one
-- tag): every record at or below `idx` is projected. `record_id` is the id of the record at `idx`,
-- so a series rewritten under the watermark (a reset, deleted or re-keyed store) is noticed and
-- replayed from the start. No row means replay that series from the start: starting with none,
-- the next daemon start replays every record, which fills in `host_id` below.
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

-- The host each row was captured on (the record's host id, hyphenated), for the host filter and
-- to tell a session recorded elsewhere from a local one. A session's is its earliest row's. NULL
-- until a replay fills it in.
ALTER TABLE messages ADD COLUMN host_id TEXT;
ALTER TABLE sessions ADD COLUMN host_id TEXT;

-- The top-most stored ancestor, following parent links else the copy link: forks, subagents and
-- copies group under it. A session whose parent is not stored (yet) is its own root, and is
-- regrouped once it arrives.
ALTER TABLE sessions ADD COLUMN root_harness INTEGER;
ALTER TABLE sessions ADD COLUMN root_session_id TEXT;

-- The session a parentless one was copied from, inferred from the model calls they share: Claude
-- Code's `--fork-session` (and a `--resume` it turns into a fork) copies the history into a new
-- session with only `sessionId` rewritten, naming the original nowhere, so capture has no parent
-- to record. A call's `turn_id` is the harness's own id for it, so one held by two sessions was
-- copied from one into the other.
--
-- It points at the lowest-ranked (earliest start, then id) other parentless session sharing a
-- call, when that ranks below this one: so links only ever go down in rank and cannot cycle, the
-- group's root is the session that owns the shared calls (see `attribute_call`), and the result
-- depends only on the rows stored. A session with a parent (a subagent, a `/branch` fork) groups
-- by it instead, and is never a link's target.
ALTER TABLE sessions ADD COLUMN copy_of_session_id TEXT;

UPDATE sessions SET copy_of_session_id = (
    SELECT t.session_id
    -- CROSS JOIN keeps this order: the session's own rows first, then who else holds each call.
    FROM messages m
    CROSS JOIN messages o ON o.harness = m.harness AND o.turn_id = m.turn_id
        AND o.session <> m.session
    CROSS JOIN sessions t ON t.id = o.session
    WHERE m.session = sessions.id
        AND m.turn_id IS NOT NULL AND t.parent_session_id IS NULL
        AND (t.started_at, t.session_id) < (sessions.started_at, sessions.session_id)
    ORDER BY t.started_at, t.session_id
    LIMIT 1
)
WHERE parent_session_id IS NULL;

CREATE TEMP TABLE ai_session_roots AS
WITH RECURSIVE chain (harness, session_id, anc_harness, anc_session_id, depth) AS (
    SELECT harness, session_id, harness, session_id, 0 FROM sessions
    UNION ALL
    SELECT c.harness, c.session_id, p.harness, p.session_id, c.depth + 1
    FROM chain c
    JOIN sessions a ON a.harness = c.anc_harness AND a.session_id = c.anc_session_id
    JOIN sessions p ON p.harness = CASE WHEN a.parent_session_id IS NULL THEN a.harness
            ELSE a.parent_harness END
        AND p.session_id = COALESCE(a.parent_session_id, a.copy_of_session_id)
    -- A parent cycle stops where it would come back round, and depth bounds any other.
    WHERE c.depth < 64 AND NOT (p.harness = c.harness AND p.session_id = c.session_id)
)
-- The deepest ancestor reached: SQLite takes the bare columns from the max(depth) row.
SELECT harness, session_id, anc_harness, anc_session_id, max(depth) AS depth
FROM chain GROUP BY harness, session_id;

UPDATE sessions SET root_harness = r.anc_harness, root_session_id = r.anc_session_id
FROM temp.ai_session_roots r
WHERE r.harness = sessions.harness AND r.session_id = sessions.session_id;

DROP TABLE temp.ai_session_roots;

CREATE INDEX sessions_root ON sessions (root_harness, root_session_id);
CREATE INDEX sessions_parent ON sessions (parent_harness, parent_session_id);
CREATE INDEX sessions_updated_at ON sessions (updated_at);
CREATE INDEX sessions_copy_of ON sessions (harness, copy_of_session_id)
WHERE copy_of_session_id IS NOT NULL;
