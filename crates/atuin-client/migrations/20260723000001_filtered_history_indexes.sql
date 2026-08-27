-- The session, directory and workspace filters (`session = ?`, `cwd = ?`,
-- `cwd like 'x%'`) had no index, so the timestamp-ordered dedup scan in
-- search() and list() read the whole table per keystroke when matches were
-- sparse. Partial indexes over live history keep that a range scan, at a
-- fraction of the size of a full index.
create index if not exists idx_history_session_timestamp on history(session, timestamp)
where deleted_at is null;

create index if not exists idx_history_cwd_timestamp on history(cwd, timestamp)
where deleted_at is null;
