-- Add migration script here
create table sessions (
	id INTEGER primary key,
	user_id INTEGER,
	token varchar(128) unique not null
);
