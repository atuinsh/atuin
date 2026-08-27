-- The host filter compares lower(hostname), because the reported casing of
-- the same machine changes over time (e.g. "Ellies-MBP" vs "ellies-mbp").
-- Index the same expression so the filter is sargable rather than a scan.
create index if not exists idx_history_hostname_timestamp on history(lower(hostname), timestamp)
where deleted_at is null;
