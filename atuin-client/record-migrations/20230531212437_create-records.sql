-- Add migration script here
create table if not exists records (
  id text primary key,
  parent text unique, -- null if this is the first one
  host text not null,

  timestamp integer not null,
  tag text not null,
  version text not null,
  data blob not null
);

create index host_idx on records (host);
create index tag_idx on records (tag);
create index host_tag_idx on records (host, tag);
