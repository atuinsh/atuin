create table sessions (
  id int auto_increment primary key,
	user_id integer,
	token varchar(256) unique not null
);

