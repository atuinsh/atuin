-- Keep interactive prefix-ranked fuzzy search fast on live history rows.
create index if not exists idx_history_command_nocase_timestamp_not_deleted on history(
    command collate nocase,
    timestamp desc
) where deleted_at is null;

create index if not exists idx_history_command_timestamp_not_deleted on history(
    command,
    timestamp desc
) where deleted_at is null;
