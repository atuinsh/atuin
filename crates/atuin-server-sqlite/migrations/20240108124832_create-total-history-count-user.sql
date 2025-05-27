create table total_history_count_user(
	id integer primary key autoincrement, 
	user_id bigserial,
	total integer -- try and avoid using keywords - hence total, not count
);
