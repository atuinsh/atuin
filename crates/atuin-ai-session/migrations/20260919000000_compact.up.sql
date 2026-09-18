-- Rebuild both tables as ordinary rowid tables, and add compressed twins for the two big
-- columns.
--
-- WITHOUT ROWID keeps whole rows in the primary-key b-tree, which SQLite recommends only for
-- small rows. With tool results up to 64KB it left about a fifth of every page unused.
--
-- Tool results are ~95% of all content and compress ~3x with zstd. A result or tool input of
-- 256 bytes or more is stored in `*_z` with the text column left empty. User, assistant and
-- title text stays plain: it is small, and the session queries read it in SQL.
CREATE TABLE messages_new (
  id BLOB PRIMARY KEY NOT NULL,        -- UUIDv7, 16 bytes
  agent INTEGER NOT NULL,              -- Agent discriminant
  session_id TEXT NOT NULL,
  parent_session_id TEXT,
  thread TEXT,
  source_id TEXT NOT NULL,
  parent_source_id TEXT,
  timestamp INTEGER NOT NULL,          -- unix millis
  role INTEGER NOT NULL,               -- Role discriminant
  content TEXT NOT NULL,               -- empty when content_z is set
  content_z BLOB,                      -- zstd
  tool_use_id TEXT,
  cwd TEXT,
  git_branch TEXT,
  model TEXT,
  tokens_input INTEGER,
  tokens_output INTEGER,
  tokens_cache_read INTEGER,
  tokens_cache_write INTEGER,
  is_error INTEGER NOT NULL DEFAULT 0,
  stop_reason INTEGER                  -- StopReason discriminant
);

INSERT INTO messages_new
SELECT id, agent, session_id, parent_session_id, thread, source_id, parent_source_id, timestamp,
       role, content, NULL, tool_use_id, cwd, git_branch, model, tokens_input, tokens_output,
       tokens_cache_read, tokens_cache_write, is_error, stop_reason
FROM messages;

CREATE TABLE tool_calls_new (
  message_id BLOB NOT NULL REFERENCES messages_new (id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  name TEXT NOT NULL,
  input TEXT NOT NULL,                 -- JSON as the agent recorded it; empty when input_z is set
  input_z BLOB,                        -- zstd
  PRIMARY KEY (message_id, id)
);

INSERT INTO tool_calls_new SELECT message_id, id, name, input, NULL FROM tool_calls;

-- Children first, so dropping the parent has nothing left to cascade into.
DROP TABLE tool_calls;
DROP TABLE messages;
ALTER TABLE messages_new RENAME TO messages;
ALTER TABLE tool_calls_new RENAME TO tool_calls;

CREATE UNIQUE INDEX messages_dedupe ON messages (agent, session_id, source_id);
CREATE INDEX messages_session ON messages (agent, session_id, timestamp);
