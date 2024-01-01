-- Add migration script here
create table if not exists store (
  id text primary key,   -- globally unique ID

  idx integer,           -- incrementing integer ID unique per (host, tag)
  host text not null, -- references the host row
  tag text not null,

  timestamp integer not null,
  version text not null,
  data blob not null,
  cek blob not null
);

create unique index record_uniq ON store(host, tag, idx);
