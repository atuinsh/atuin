create table store_idx_cache(
  id integer primary key autoincrement, 
  user_id bigint,

  host uuid,
  tag text,
  idx bigint
);

create unique index store_idx_cache_uniq on store_idx_cache(user_id, host, tag);
