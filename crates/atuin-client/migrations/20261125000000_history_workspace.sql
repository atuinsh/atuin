-- The repository that the command ran in, shared by all of its checkouts: the
-- git common dir or the jj repo dir. Null outside a repository, and for
-- commands recorded before this column existed.
alter table history add column workspace text;

-- The workspace filter matches `workspace = ?` before it falls back to a cwd
-- prefix, so it gets the same partial index as the cwd filter.
create index if not exists idx_history_workspace_timestamp on history(workspace, timestamp)
where deleted_at is null;
