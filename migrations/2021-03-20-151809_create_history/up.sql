-- Your SQL goes here
-- lower case SQL please, this isn't a shouting match
create table history (
	id bigserial primary key,
	client_id text not null unique, -- the client-generated ID
	user_id bigserial not null,     -- allow multiple users
	timestamp timestamp not null,   -- one of the few non-encrypted metadatas

	data varchar(8192) not null     -- store the actual history data, encrypted. I don't wanna know!
);
