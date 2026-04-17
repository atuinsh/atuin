-- Mirrors the postgres migration of the same date. Supports the
-- `atuin store repair` UPDATE path which matches rows by (user_id, client_id).
create index if not exists store_user_client on store(user_id, client_id);
