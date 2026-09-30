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

-- Each session's root: the last session reached walking up from it, however far that is. A walk
-- stops at a session already on it, so a cycle of parent links (which only corrupt data makes)
-- ends too; a cycle has no top, so its least member (by harness, then id) heads it and whatever
-- hangs off it, whichever member a walk entered it by. `regroup_all` runs the same query.
CREATE TEMP TABLE ai_session_roots AS
WITH RECURSIVE
-- The next session up from each: its parent, else the original it was copied from, if stored.
up (harness, session_id, up_harness, up_session_id) AS (
    SELECT a.harness, a.session_id, p.harness, p.session_id
    FROM sessions a
    JOIN sessions p ON p.harness = CASE WHEN a.parent_session_id IS NULL THEN a.harness
            ELSE a.parent_harness END
        AND p.session_id = COALESCE(a.parent_session_id, a.copy_of_session_id)
),
-- Every session reached walking up from each, with the sessions on the way (`harness:id`, the
-- harness being a number) in `path`.
chain (harness, session_id, anc_harness, anc_session_id, depth, path) AS (
    SELECT harness, session_id, harness, session_id, 0, json_array(harness || ':' || session_id)
    FROM sessions
    UNION ALL
    SELECT c.harness, c.session_id, u.up_harness, u.up_session_id, c.depth + 1,
        json_insert(c.path, '$[#]', u.up_harness || ':' || u.up_session_id)
    FROM chain c
    JOIN up u ON u.harness = c.anc_harness AND u.session_id = c.anc_session_id
    WHERE NOT EXISTS (
        SELECT 1 FROM json_each(c.path) v WHERE v.value = u.up_harness || ':' || u.up_session_id
    )
),
-- Where each walk ended: SQLite takes the bare columns from the max(depth) row.
ends AS (
    SELECT harness, session_id, anc_harness, anc_session_id, max(depth) AS depth
    FROM chain GROUP BY harness, session_id
),
-- A walk that ended with a session still above it came round to one already on it, at depth
-- `entered`: everything on it from there is the cycle.
loops AS (
    SELECT e.harness, e.session_id, c.depth AS entered
    FROM ends e
    JOIN up u ON u.harness = e.anc_harness AND u.session_id = e.anc_session_id
    JOIN chain c ON c.harness = e.harness AND c.session_id = e.session_id
        AND c.anc_harness = u.up_harness AND c.anc_session_id = u.up_session_id
),
heads AS (
    SELECT l.harness, l.session_id, c.anc_harness, c.anc_session_id,
        row_number() OVER (PARTITION BY l.harness, l.session_id
            ORDER BY c.anc_harness, c.anc_session_id) AS n
    FROM loops l
    JOIN chain c ON c.harness = l.harness AND c.session_id = l.session_id AND c.depth >= l.entered
)
SELECT e.harness, e.session_id, COALESCE(h.anc_harness, e.anc_harness) AS anc_harness,
    COALESCE(h.anc_session_id, e.anc_session_id) AS anc_session_id
FROM ends e
LEFT JOIN heads h ON h.harness = e.harness AND h.session_id = e.session_id AND h.n = 1;

UPDATE sessions SET root_harness = r.anc_harness, root_session_id = r.anc_session_id
FROM temp.ai_session_roots r
WHERE r.harness = sessions.harness AND r.session_id = sessions.session_id;

DROP TABLE temp.ai_session_roots;

CREATE INDEX sessions_root ON sessions (root_harness, root_session_id);
CREATE INDEX sessions_parent ON sessions (parent_harness, parent_session_id);
CREATE INDEX sessions_updated_at ON sessions (updated_at);
CREATE INDEX sessions_copy_of ON sessions (harness, copy_of_session_id)
WHERE copy_of_session_id IS NOT NULL;
