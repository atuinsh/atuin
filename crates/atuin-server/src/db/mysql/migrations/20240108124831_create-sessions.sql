create table sessions (
  id bigint auto_increment primary key,
	user_id bigint,
	token varchar(256) unique not null
);

