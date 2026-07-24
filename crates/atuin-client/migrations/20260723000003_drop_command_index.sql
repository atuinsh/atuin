-- idx_history_command (from the original 2021 schema) has been redundant
-- since idx_history_command_timestamp was added: it shares the leading
-- column, so every query that could use it plans identically onto the
-- (command, timestamp) index. Dropping it saves ~9MB on a 100MB database
-- and removes an index from the per-command insert path.
drop index if exists idx_history_command;
