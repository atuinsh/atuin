-- Whether a human or an agent ran the command, when the integration that
-- captured it said so. Null means it did not, and the author name is all we
-- have to go on. See `atuin_client::history::AuthorKind`.
alter table history add column author_kind integer;
