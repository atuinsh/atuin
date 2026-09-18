-- The agent `parent_session_id` belongs to, when it is a different one: set on turns an agent
-- added to a session Atuin handed it. NULL means the same agent (an OpenCode subagent session).
ALTER TABLE messages ADD COLUMN parent_agent INTEGER;
