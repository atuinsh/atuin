-- Your SQL goes here
create table users (
	id bigserial primary key,               -- also store our own ID
	email varchar(128) not null unique,     -- being able to contact users is useful
	password varchar(128) not null unique
);
