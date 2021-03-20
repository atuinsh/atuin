-- Your SQL goes here
-- lower case SQL please, this isn't a shouting match
create table history (
	id text primary key,          -- use the client-generated ID (authed ofc)
	"user" text not null,         -- allow multiple users
	mac text not null,            -- store a hashed mac address
	timestamp timestamp not null, -- one of the few non-encrypted metadatas

	data text not null,           -- store the actual history data, encrypted. I don't wanna know!
	signature text not null       -- as users are essentially public keys, these should be signed
);
