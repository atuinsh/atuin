-- Whether a human or an agent ran the command. If null, we have to go off
-- the author name.
alter table history add column author_kind integer;
