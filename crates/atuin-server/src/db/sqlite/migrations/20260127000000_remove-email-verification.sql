drop table if exists user_verification_token;
alter table users drop column verified_at;
