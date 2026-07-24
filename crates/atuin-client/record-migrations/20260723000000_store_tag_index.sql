-- all_tagged and len_tag select by tag ordered by timestamp; without an
-- index they scan and sort the entire store - over a second on a large,
-- cold store - on every kv/scripts/dotfiles load and store rebuild.
create index if not exists idx_store_tag_timestamp on store(tag, timestamp);
