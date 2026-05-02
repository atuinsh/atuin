create table users (
	id bigserial primary key,               -- also store our own ID
	username varchar(32) not null unique,   -- being able to contact users is useful
	email varchar(128) not null unique,     -- being able to contact users is useful
	password varchar(128) not null unique
);

-- the prior index is case sensitive :(
CREATE UNIQUE INDEX email_unique_idx on users (LOWER(email));
CREATE UNIQUE INDEX username_unique_idx on users (LOWER(username));
