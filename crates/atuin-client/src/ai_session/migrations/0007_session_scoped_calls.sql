-- A model call is counted once across every session holding it only when its `turn_id` is one the
-- harness gave it (see `linkable_turn!`). An id capture derived from a line's content (Codex
-- `token_count:`/`thread:`, Pi `line:`, opencode `<millis>:`) can be shared by unrelated sessions,
-- so such a call is counted once per lineage instead: once among the sessions holding it that
-- descend one from another (a fork copying its parent's history), and separately in each
-- unrelated session.
--
-- `scope` tells those apart: empty for a harness-given id, else the session heading the lineage
-- (see `attribute_call`).
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
