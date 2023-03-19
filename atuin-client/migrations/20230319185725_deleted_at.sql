-- Add migration script here
alter table history add column deleted_at integer;
