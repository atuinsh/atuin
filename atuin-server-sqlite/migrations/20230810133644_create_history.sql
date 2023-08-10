create table history (
	id integer primary key autoincrement,
	client_id text not null unique, -- the client-generated ID
	user_id int not null,     -- allow multiple users
	hostname text not null,         -- a unique identifier from the client (can be hashed, random, whatever)
	timestamp int not null,   -- one of the few non-encrypted metadatas
	data blob not null,    -- store the actual history data, encrypted. I don't wanna know!
	created_at int not null default (unixepoch()),
    deleted_at int
);

-- queries will all be selecting the ids of history for a user, that has been deleted
create index if not exists history_deleted_index on history(client_id, user_id, deleted_at);
