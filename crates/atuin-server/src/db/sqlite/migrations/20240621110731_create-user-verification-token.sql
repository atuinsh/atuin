create table user_verification_token(
  id integer primary key autoincrement, 
  user_id bigint unique references users(id), 
  token text, 
  valid_until timestamp with time zone
);
