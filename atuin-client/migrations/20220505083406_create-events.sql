create table if not exists events (
	id text primary key,
	timestamp integer not null,
	hostname text not null,
	event_type text not null,

	history_id text not null
);
