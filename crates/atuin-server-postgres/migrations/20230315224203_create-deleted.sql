-- Add migration script here
alter table history add column if not exists deleted_at timestamp;

-- queries will all be selecting the ids of history for a user, that has been deleted
create index if not exists history_deleted_index on history(client_id, user_id, deleted_at);
