-- One row per transcript line. Sessions are derived, never stored.
CREATE TABLE messages (
  id BLOB PRIMARY KEY NOT NULL,        -- UUIDv7, 16 bytes
  agent INTEGER NOT NULL,              -- Agent discriminant
  session_id TEXT NOT NULL,            -- agent-native; OpenCode ids are not UUIDs
  parent_session_id TEXT,
  thread TEXT,
  source_id TEXT NOT NULL,             -- agent-native message id, or byte offset
  parent_source_id TEXT,
  timestamp INTEGER NOT NULL,          -- unix millis
  role INTEGER NOT NULL,               -- Role discriminant
  content TEXT NOT NULL,
  tool_use_id TEXT,
  cwd TEXT,
  git_branch TEXT,
  model TEXT,
  tokens_input INTEGER,
  tokens_output INTEGER,
  tokens_cache_read INTEGER,
  tokens_cache_write INTEGER
) WITHOUT ROWID;

CREATE UNIQUE INDEX messages_dedupe ON messages (agent, session_id, source_id);
CREATE INDEX messages_session ON messages (agent, session_id, timestamp);

CREATE TABLE tool_calls (
  message_id BLOB NOT NULL REFERENCES messages (id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  name TEXT NOT NULL,
  input TEXT NOT NULL,                 -- JSON as the agent recorded it
  PRIMARY KEY (message_id, id)
) WITHOUT ROWID;

-- How far into each JSONL transcript ingest has read.
CREATE TABLE ingest_files (
  agent INTEGER NOT NULL,
  path TEXT NOT NULL,
  offset INTEGER NOT NULL,
  PRIMARY KEY (agent, path)
) WITHOUT ROWID;
