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
-- by it instead, and is never a link's target. Grouping follows the parent, else this link.
ALTER TABLE sessions ADD COLUMN copy_of_session_id TEXT;

UPDATE sessions SET copy_of_session_id = (
    SELECT t.session_id
    -- CROSS JOIN keeps this order: the session's own rows first, then who else holds each call.
    FROM messages m
    CROSS JOIN messages o ON o.harness = m.harness AND o.turn_id = m.turn_id
        AND o.session_id <> m.session_id
    CROSS JOIN sessions t ON t.harness = o.harness AND t.session_id = o.session_id
    WHERE m.harness = sessions.harness AND m.session_id = sessions.session_id
        AND m.turn_id IS NOT NULL AND t.parent_session_id IS NULL
        AND (t.started_at, t.session_id) < (sessions.started_at, sessions.session_id)
    ORDER BY t.started_at, t.session_id
    LIMIT 1
)
WHERE parent_session_id IS NULL;

CREATE INDEX sessions_copy_of ON sessions (harness, copy_of_session_id)
WHERE copy_of_session_id IS NOT NULL;

-- Regroup every session, as 0003 did, following the parent else the copy link.
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
    WHERE c.depth < 64 AND NOT (p.harness = c.harness AND p.session_id = c.session_id)
)
SELECT harness, session_id, anc_harness, anc_session_id, max(depth) AS depth
FROM chain GROUP BY harness, session_id;

UPDATE sessions SET root_harness = r.anc_harness, root_session_id = r.anc_session_id
FROM temp.ai_session_roots r
WHERE r.harness = sessions.harness AND r.session_id = sessions.session_id;

DROP TABLE temp.ai_session_roots;
