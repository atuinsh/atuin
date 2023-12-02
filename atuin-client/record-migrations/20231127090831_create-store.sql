-- Add migration script here
create table if not exists host(
  id integer primary key,    -- just the rowid for normalization
  host text unique not null, -- the globally unique host id (uuid)
  name text unique           -- an optional user-friendly alias
);

-- this will become more useful when we allow for multiple recipients of 
-- some data (same cek, multiple long term keys)
-- This could be a key per host rather than one global key, or separate users.
create table if not exists cek (
  id integer primary key,    -- normalization rowid
  cek text unique not null, 
);

create table if not exists store (
  id text primary key,   -- globally unique ID

  idx integer,           -- incrementing integer ID unique per (host, tag)
  host integer not null, -- references the host row
  cek integer not null,  -- references the cek row

  timestamp integer not null,
  tag text not null,
  version text not null,
  data blob not null,

  foreign key(host) references host(id),
  foreign key(cek) references cek(id)
);
