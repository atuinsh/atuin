-- CLIENT

drop table events; -- we will rewrite it anyway

create table if not exists events (
	id text primary key,
	timestamp integer not null,
	hostname text not null,
	event_type text not null,

	data blob not null,
	checksum text not null,
	previous text not null
);
