-- the old version of this function used NEW in the delete part when it should
-- use OLD

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
		update total_history_count_user set total = total - 1 where user_id = old.user_id;

		if not found then
			insert into total_history_count_user(user_id, total) 
			values (
				old.user_id, 
				(select count(1) from history where user_id = old.user_id)
			);
		end if;
	end if;

	return NEW; -- this is actually ignored for an after trigger, but oh well
end;
$func$
language plpgsql volatile -- pldfplplpflh
cost 100; -- default value
