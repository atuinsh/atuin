-- The host each row was captured on (the record's host id, hyphenated), for the host filter and
-- to tell a session recorded elsewhere (viewable, not resumable) from a local one. A session's is
-- its first row's. NULL until the reproject below fills it in.
ALTER TABLE messages ADD COLUMN host_id TEXT;
ALTER TABLE sessions ADD COLUMN host_id TEXT;

-- The top-most stored ancestor, following parent links: forks and subagents group under it. A
-- session whose parent is not stored (yet) is its own root, and is regrouped once it arrives.
ALTER TABLE sessions ADD COLUMN root_harness INTEGER;
ALTER TABLE sessions ADD COLUMN root_session_id TEXT;

CREATE TEMP TABLE ai_session_roots AS
WITH RECURSIVE chain (harness, session_id, anc_harness, anc_session_id, depth) AS (
    SELECT harness, session_id, harness, session_id, 0 FROM sessions
    UNION ALL
    SELECT c.harness, c.session_id, p.harness, p.session_id, c.depth + 1
    FROM chain c
    JOIN sessions a ON a.harness = c.anc_harness AND a.session_id = c.anc_session_id
    JOIN sessions p ON p.harness = a.parent_harness AND p.session_id = a.parent_session_id
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

-- host_id comes from the record envelope, so filling it needs every record replayed: forget the
-- reproject watermark to force a full reproject on the next daemon start.
DELETE FROM reproject_watermark;
