-- Add migration script here

create table total_event_count_user(
	id bigserial primary key, 
	user_id bigserial,
	total integer -- try and avoid using keywords - hence total, not count
);

create or replace function user_event_count()
returns trigger as 
$func$
begin
	if (TG_OP='INSERT') then
		update total_event_count_user set total = total + 1 where user_id = new.user_id;

		if not found then
			insert into total_event_count_user(user_id, total) 
			values (
				new.user_id, 
				(select count(1) from events where user_id = new.user_id)
			);
		end if;
		
	elsif (TG_OP='DELETE') then
		update total_event_count_user set total = total - 1 where user_id = old.user_id;

		if not found then
			insert into total_event_count_user(user_id, total) 
			values (
				old.user_id, 
				(select count(1) from events where user_id = old.user_id)
			);
		end if;
	end if;

	return NEW; -- this is actually ignored for an after trigger, but oh well
end;
$func$
language plpgsql volatile -- pldfplplpflh
cost 100; -- default value

create trigger tg_user_event_count 
	after insert or delete on events
	for each row 
	execute procedure user_event_count();
