create table history_host_id_backfill (
    id integer primary key check (id = 1),
    generation integer not null default 0,
    complete integer not null default 0
);

insert into history_host_id_backfill (id) values (1);
