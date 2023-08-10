-- Add migration script here
create table records (
	id blob primary key,            -- remember to use uuidv7 for happy indices <3
    client_id blob not null,        -- I am too uncomfortable with the idea of a client-generated primary key
	host blob not null,             -- a unique identifier for the host
	parent blob default null,       -- the ID of the parent record, bearing in mind this is a linked list
	timestamp int not null,      -- not a timestamp type, as those do not have nanosecond precision
	version text not null,
	tag text not null,              -- what is this? history, kv, whatever. Remember clients get a log per tag per host
	data blob not null,            -- store the actual history data, encrypted. I don't wanna know!
	cek text not null,            
	user_id int not null,        -- allow multiple users
	created_at int not null default (unixepoch()),
    foreign key (user_id) references users(id)
);
