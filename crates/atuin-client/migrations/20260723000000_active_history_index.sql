-- Most queries only consider live history (deleted_at is null), and counting
-- it previously required a full table scan - slow on a large, cold database.
-- A partial index on timestamp covers both the count and timestamp-ordered
-- listings of live history, at a fraction of the table's size.
create index if not exists idx_history_active_timestamp on history(timestamp)
where deleted_at is null;
