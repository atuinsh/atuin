-- How a session relates to its parent (`ParentKind`: 0 subagent, 1 fork, 2 continuation), so
-- readers can tell a subagent's fragment from a conversation a person carried on. NULL when the
-- records predate it or the harness did not say.
ALTER TABLE sessions ADD COLUMN parent_kind INTEGER;
