-- Add migration script here
alter table history alter column user_id drop default;
alter table sessions alter column user_id drop default;
alter table total_history_count_user alter column user_id drop default;
