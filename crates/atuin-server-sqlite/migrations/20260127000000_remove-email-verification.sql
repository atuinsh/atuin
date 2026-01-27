drop table if exists user_verification_token;
alter table users drop column if exists verified_at;
