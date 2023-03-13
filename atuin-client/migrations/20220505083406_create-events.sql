create table if not exists events (
	id text primary key,
	timestamp integer not null,
	hostname text not null,
	event_type text not null,

	history_id text not null
);

-- Ensure there is only ever one of each event type per history item
create unique index history_event_idx ON events(event_type, history_id);
