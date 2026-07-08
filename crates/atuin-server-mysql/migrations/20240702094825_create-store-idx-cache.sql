create table store_idx_cache(
  id int auto_increment primary key,
  user_id bigint,

  host VARBINARY(16),
  tag varchar(256),
  idx bigint
);

create unique index store_idx_cache_uniq on store_idx_cache(user_id, host, tag);
