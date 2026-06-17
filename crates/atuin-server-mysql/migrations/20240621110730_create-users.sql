create table users (
  id int auto_increment primary key,
	username varchar(256) not null unique,   -- being able to contact users is useful
	email varchar(256) not null unique,     -- being able to contact users is useful
	password varchar(256) not null unique,
  created_at timestamp not null default current_timestamp
);

-- the prior index is case sensitive :(
CREATE UNIQUE INDEX email_unique_idx on users ((LOWER(email)));
CREATE UNIQUE INDEX username_unique_idx on users ((LOWER(username)));
