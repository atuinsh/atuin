create table records (
	id text primary key,            -- remember to use uuidv7 for happy indices <3
  client_id text not null,        -- I am too uncomfortable with the idea of a client-generated primary key
	host text not null,             -- a unique identifier for the host
	parent text default null,       -- the ID of the parent record, bearing in mind this is a linked list
	timestamp integer not null,      -- not a timestamp type, as those do not have nanosecond precision
	version text not null,
	tag text not null,              -- what is this? history, kv, whatever. Remember clients get a log per tag per host
	data text not null,            -- store the actual history data, encrypted. I don't wanna know!
	cek text not null,            

	user_id integer not null,        -- allow multiple users
	created_at timestamp not null default current_timestamp
);

