-- Add migration script here
create table if not exists records (
  id text primary key,
  host text not null,
  timestamp integer not null,

  tag text not null,
  version text not null,
  data blob not null,
);
