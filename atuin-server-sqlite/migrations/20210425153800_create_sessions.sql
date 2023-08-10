-- Add migration script here
create table sessions (
	id integer primary key autoincrement,
	user_id int not null,
	token varchar(128) unique not null,
    foreign key (user_id) references users(id)
);
