create table users (
	id integer primary key autoincrement,               -- also store our own ID
	username text not null unique,   -- being able to contact users is useful
	email text not null unique,     -- being able to contact users is useful
	password text not null unique,
  created_at timestamp not null default (datetime('now','localtime')),
  verified_at timestamp with time zone default null
);

-- the prior index is case sensitive :(
CREATE UNIQUE INDEX email_unique_idx on users (LOWER(email));
CREATE UNIQUE INDEX username_unique_idx on users (LOWER(username));
