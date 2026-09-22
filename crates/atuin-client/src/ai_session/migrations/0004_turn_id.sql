-- The model call a row came from; groups the rows one response is split into.
ALTER TABLE messages ADD COLUMN turn_id TEXT;
