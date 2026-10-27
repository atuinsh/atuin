create table store_idx_cache(
  id bigserial primary key, 
  user_id bigint,

  host uuid,
  tag text,
  idx bigint
);
