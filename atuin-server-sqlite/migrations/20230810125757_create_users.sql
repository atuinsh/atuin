create table users (
	id integer primary key autoincrement,               -- also store our own ID
	username varchar(32) not null unique,   -- being able to contact users is useful
	email varchar(128) not null unique,     -- being able to contact users is useful
	password varchar(128) not null unique,
    created_at int not null default (unixepoch())
);

CREATE UNIQUE INDEX email_unique_idx on users (LOWER(email));
CREATE UNIQUE INDEX username_unique_idx on users (LOWER(username));

