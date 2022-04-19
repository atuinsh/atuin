-- Prior to this, the count endpoint was super naive and just ran COUNT(1). 
-- This is slow asf. Now that we have an amount of actual traffic, 
-- stop doing that!
-- This basically maintains a count, so we can read ONE row, instead of ALL the
-- rows. Much better.
-- Future optimisation could use some sort of cache so we don't even need to hit
-- postgres at all.

create table total_history_count_user(
	id bigserial primary key, 
	user_id bigserial,
	total integer -- try and avoid using keywords - hence total, not count
);

create or replace function user_history_count()
returns trigger as 
$func$
begin
	if (TG_OP='INSERT') then
		update total_history_count_user set total = total + 1 where user_id = new.user_id;

		if not found then
			insert into total_history_count_user(user_id, total) 
			values (
				new.user_id, 
				(select count(1) from history where user_id = new.user_id)
			);
		end if;
		
	elsif (TG_OP='DELETE') then
		update total_history_count_user set total = total - 1 where user_id = new.user_id;

		if not found then
			insert into total_history_count_user(user_id, total) 
			values (
				new.user_id, 
				(select count(1) from history where user_id = new.user_id)
			);
		end if;
	end if;

	return NEW; -- this is actually ignored for an after trigger, but oh well
end;
$func$
language plpgsql volatile -- pldfplplpflh
cost 100; -- default value

create trigger tg_user_history_count 
	after insert or delete on history 
	for each row 
	execute procedure user_history_count();
