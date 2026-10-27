create table sessions (
	id integer primary key autoincrement,
	user_id integer,
	token text unique not null
);

