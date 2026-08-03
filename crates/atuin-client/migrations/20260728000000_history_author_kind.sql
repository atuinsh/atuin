-- Records whether `author` names an agent (1) or a human/nobody (0/null).
-- Set at capture time rather than inferred from the name, so a human whose
-- username matches an agent name is never filtered out (GH #3472). Existing
-- rows default to null and are treated as non-agent.
alter table history add column author_kind integer;
