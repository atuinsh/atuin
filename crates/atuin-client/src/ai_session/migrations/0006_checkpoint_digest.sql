-- The digest of the item a checkpoint was taken after, so a transcript rewritten under it or an
-- event log reset since is noticed on resume. NULL on checkpoints stored before this: those read
-- their session again from the start once.
ALTER TABLE checkpoints ADD COLUMN digest INTEGER;
