-- The timestamp of the row each context column and the preview came from, so rows arriving out
-- of timestamp order (a sync interleaving hosts, an import after live capture) still leave the
-- newest context and the oldest preview whatever the insert order.
ALTER TABLE sessions ADD COLUMN cwd_at INTEGER;
ALTER TABLE sessions ADD COLUMN git_branch_at INTEGER;
ALTER TABLE sessions ADD COLUMN model_at INTEGER;
ALTER TABLE sessions ADD COLUMN preview_at INTEGER;

-- Backfill from the stored messages: each context column from the newest row that reported it,
-- and the preview's timestamp as the oldest user row's, the earliest a preview can come from.
-- Without this, an upgraded session's NULL timestamps would let the next row appended win the
-- comparison whatever its order.
UPDATE sessions SET
    cwd = (SELECT m.cwd FROM messages m WHERE m.harness = sessions.harness
        AND m.session_id = sessions.session_id AND m.cwd IS NOT NULL
        ORDER BY m.timestamp DESC LIMIT 1),
    cwd_at = (SELECT MAX(m.timestamp) FROM messages m WHERE m.harness = sessions.harness
        AND m.session_id = sessions.session_id AND m.cwd IS NOT NULL),
    git_branch = (SELECT m.git_branch FROM messages m WHERE m.harness = sessions.harness
        AND m.session_id = sessions.session_id AND m.git_branch IS NOT NULL
        ORDER BY m.timestamp DESC LIMIT 1),
    git_branch_at = (SELECT MAX(m.timestamp) FROM messages m WHERE m.harness = sessions.harness
        AND m.session_id = sessions.session_id AND m.git_branch IS NOT NULL),
    model = (SELECT m.model FROM messages m WHERE m.harness = sessions.harness
        AND m.session_id = sessions.session_id AND m.model IS NOT NULL
        ORDER BY m.timestamp DESC LIMIT 1),
    model_at = (SELECT MAX(m.timestamp) FROM messages m WHERE m.harness = sessions.harness
        AND m.session_id = sessions.session_id AND m.model IS NOT NULL),
    preview_at = IIF(preview IS NULL, NULL, (SELECT MIN(m.timestamp) FROM messages m
        WHERE m.harness = sessions.harness AND m.session_id = sessions.session_id
        AND m.role = '"User"'));
