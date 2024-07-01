-- using group by and max is slow. as the store grows, latency is creeping up.
-- get a handle on it!

create table store_idx_cache(
  id bigserial primary key, 
  user_id bigint,

  host uuid,
  tag text,
  idx bigint
);


create or replace function cache_store_idx()
returns trigger as 
$func$
begin
	if (TG_OP='INSERT') then
		update store_idx_cache set idx = (select max(idx) from store where user_id=new.user_id and host=new.host and tag=new.tag) where user_id=new.user_id and host=new.host and tag=new.tag;

		if not found then
			insert into store_idx_cache(user_id, host, tag, idx) 
			values (
				new.user_id, 
        new.host,
        new.tag,
				(select max(idx) from store where user_id = new.user_id and host=new.host and tag=new.tag)
			);
		end if;
	end if;

	return NEW; -- this is actually ignored for an after trigger, but oh well
end;
$func$
language plpgsql volatile -- pldfplplpflh
cost 100; -- default value

create or replace trigger tg_cache_store_idx
	after insert on store
	for each row 
	execute procedure cache_store_idx();
