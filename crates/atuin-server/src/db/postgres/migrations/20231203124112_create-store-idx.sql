-- Add migration script here
create unique index record_uniq ON store(user_id, host, tag, idx);
