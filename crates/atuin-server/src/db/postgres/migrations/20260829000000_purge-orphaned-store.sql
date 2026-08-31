-- Postgres deployments hit the same delete_user bug (shared code path) that left
-- orphaned store rows behind. Clean them up. See the sqlite counterpart.
delete from store where user_id not in (select id from users);
