-- Add migration script here
create table if not exists history (
	id text primary key,
	timestamp integer not null,
	duration integer not null,
	exit integer not null,
	command text not null,
	cwd text not null,
	session text not null,
	hostname text not null,

	unique(timestamp, cwd, command)
);

create index if not exists idx_history_timestamp on history(timestamp);
create index if not exists idx_history_command on history(command);
