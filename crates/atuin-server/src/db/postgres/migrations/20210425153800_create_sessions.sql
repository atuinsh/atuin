-- Add migration script here
create table sessions (
	id bigserial primary key,
	user_id bigserial,
	token varchar(128) unique not null
);
