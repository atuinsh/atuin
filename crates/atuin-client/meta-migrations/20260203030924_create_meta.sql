create table if not exists meta (
    key text not null primary key,
    value text not null,
    updated_at integer not null default (strftime('%s', 'now'))
);
