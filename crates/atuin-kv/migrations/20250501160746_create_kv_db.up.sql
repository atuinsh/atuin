-- Add up migration script here
CREATE TABLE
  kv (
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    inserted_at INTEGER NOT NULL DEFAULT (strftime ('%s', 'now'))
  );

CREATE INDEX idx_kv_namespace ON kv (namespace);

CREATE UNIQUE INDEX idx_kv ON kv (namespace, key);
