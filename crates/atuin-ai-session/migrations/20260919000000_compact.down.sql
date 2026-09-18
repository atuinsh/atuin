-- Compressed rows cannot be restored in SQL; the store is rebuilt by `atuin ai ingest`.
DROP TABLE tool_calls;
DROP TABLE messages;
DELETE FROM ingest_files;
CREATE TABLE messages (
  id BLOB PRIMARY KEY NOT NULL, agent INTEGER NOT NULL, session_id TEXT NOT NULL,
  parent_session_id TEXT, thread TEXT, source_id TEXT NOT NULL, parent_source_id TEXT,
  timestamp INTEGER NOT NULL, role INTEGER NOT NULL, content TEXT NOT NULL, tool_use_id TEXT,
  cwd TEXT, git_branch TEXT, model TEXT, tokens_input INTEGER, tokens_output INTEGER,
  tokens_cache_read INTEGER, tokens_cache_write INTEGER, is_error INTEGER NOT NULL DEFAULT 0,
  stop_reason INTEGER
) WITHOUT ROWID;
CREATE UNIQUE INDEX messages_dedupe ON messages (agent, session_id, source_id);
CREATE INDEX messages_session ON messages (agent, session_id, timestamp);
CREATE TABLE tool_calls (
  message_id BLOB NOT NULL REFERENCES messages (id) ON DELETE CASCADE, id TEXT NOT NULL,
  name TEXT NOT NULL, input TEXT NOT NULL, PRIMARY KEY (message_id, id)
) WITHOUT ROWID;
