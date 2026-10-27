-- We do not need to run the trigger on deletes, as the only time we are deleting history is when the user
-- has already been deleted
-- This actually slows down deleting all the history a good bit!

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
	end if;

	return NEW; -- this is actually ignored for an after trigger, but oh well
end;
$func$
language plpgsql volatile -- pldfplplpflh
cost 100; -- default value

create or replace trigger tg_user_history_count 
	after insert on history 
	for each row 
	execute procedure user_history_count();
