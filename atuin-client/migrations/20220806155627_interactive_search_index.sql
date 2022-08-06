-- Interactive search filters by command then by the max(timestamp) for that
-- command. Create an index that covers those
create index if not exists idx_history_command_timestamp on history(
	command,
	timestamp
);
