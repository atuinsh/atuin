-- `total_history_count_user` was a denormalised per-user history counter,
-- maintained by a trigger on `history`, built to serve an old `count(1)`
-- endpoint. That endpoint is long gone: nothing reads the table, and the server
-- no longer inserts into `history`, so the trigger never even fires. It was also
-- Postgres-only (SQLite and MySQL never had it), the sole reason `delete_user`
-- differed across backends. Drop the whole apparatus.
drop trigger if exists tg_user_history_count on history;
drop function if exists user_history_count();
drop table if exists total_history_count_user;
