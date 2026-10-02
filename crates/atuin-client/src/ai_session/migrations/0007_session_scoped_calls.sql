-- A model call is counted once across every session holding it only when its `turn_id` is one the
-- harness gave it (see `linkable_turn!`). An id capture derived from a line's content (Codex
-- `token_count:`/`thread:`, Pi `line:`, opencode `<millis>:`) can be shared by unrelated sessions,
-- so such a call is counted once per group instead: once among the sessions holding it that share
-- a root (`sessions.root_harness`/`root_session_id`: a fork copying its parent's history is grouped
-- under it), and separately in each unrelated group.
--
-- `scope` tells those apart: empty for a harness-given id, else the group's root as
-- `<root_harness>:<root_session_id>` (see `attribute_call`).
DROP TABLE calls;

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

-- Usage stored before was attributed by the old rule. `AiSessionDatabase::migrate` attributes it
-- afresh from the stored rows, in Rust with the same code rows arriving later go through, and
-- clears this once it has.
ALTER TABLE projection_state ADD COLUMN recount_calls INTEGER NOT NULL DEFAULT 0;
UPDATE projection_state SET recount_calls = 1;

-- The sessions whose root changed since `AiSessionDatabase` last attributed afresh the
-- content-derived calls they hold, which are scoped to that root: filled by the trigger, wherever
-- sessions are regrouped, and emptied in the same transaction.
CREATE TABLE regrouped (session INTEGER PRIMARY KEY);

CREATE TRIGGER sessions_regrouped AFTER UPDATE OF root_harness, root_session_id ON sessions
WHEN OLD.root_harness IS NOT NEW.root_harness OR OLD.root_session_id IS NOT NEW.root_session_id
BEGIN
    INSERT OR IGNORE INTO regrouped (session) VALUES (NEW.id);
END;
