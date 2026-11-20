-- The Atuin install that ran the command, as a UUID blob. It's null when the
-- record's host may not be the install that ran the command, or when the row
-- hasn't been filled from its record yet. The host filter matches on it.
alter table history add column host_id blob;

create index if not exists idx_history_host_id_timestamp on history(host_id, timestamp)
where deleted_at is null;
