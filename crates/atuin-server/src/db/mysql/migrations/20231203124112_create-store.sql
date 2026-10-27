create table store (
	id VARBINARY(16) primary key,            -- remember to use uuidv7 for happy indices <3
  client_id VARBINARY(16) not null,        -- I am too uncomfortable with the idea of a client-generated primary key, even though it's fine mathematically
	host VARBINARY(16) not null,             -- a unique identifier for the host
	idx bigint not null,       -- the index of the record in this store, identified by (host, tag)
	timestamp bigint not null,      -- not a timestamp type, as those do not have nanosecond precision
	version longtext not null,
	tag varchar(256) character set utf8mb4 collate utf8mb4_bin not null,              -- what is this? history, kv, whatever. Remember clients get a log per tag per host
	data longtext not null,            -- store the actual history data, encrypted. I don't wanna know!
	cek longtext not null,

	user_id bigint not null,        -- allow multiple users
	created_at datetime not null default current_timestamp
);

create unique index record_uniq ON store(user_id, host, tag, idx);

