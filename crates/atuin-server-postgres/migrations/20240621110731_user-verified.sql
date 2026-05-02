alter table users add verified_at timestamp with time zone default null;

create table user_verification_token(
  id bigserial primary key, 
  user_id bigint unique references users(id), 
  token text, 
  valid_until timestamp with time zone
);
