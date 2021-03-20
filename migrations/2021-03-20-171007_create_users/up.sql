-- Your SQL goes here
create table users (
	id bigserial primary key,
	email text not null unique,  -- being able to contact users is useful
	api text not null unique,    -- the users API key
	key text not null unique     -- the users public key
);
