-- Supports the `atuin store repair` UPDATE path, which targets rows by
-- (user_id, client_id). Without this index, each repair POST does a full
-- table scan of the `store` table, which times out once the table grows
-- beyond a few hundred thousand rows.
create index if not exists store_user_client on store(user_id, client_id);
