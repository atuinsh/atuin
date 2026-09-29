-- Sessions are listed newest first, often for one harness and only those active since a given
-- time; without this every listing reads and sorts the whole table.
CREATE INDEX sessions_harness_updated_at ON sessions (harness, updated_at);
