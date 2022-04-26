create index concurrently if not exists "history_idx" on history using btree (user_id, timestamp);
