create table if not exists total_history_count_user(
  id integer primary key autoincrement,
  user_id int unique,
  total int,
  foreign key (user_id) references users(id)
);


drop trigger if exists tg_user_history_count;
create trigger tg_user_history_count 
  after insert on history
begin
  insert into total_history_count_user (user_id, total)
  values (
           new.user_id,
           (select count(1) from history where user_id = new.user_id)
         )
  on conflict(user_id) do update set total = total + 1;
end;
