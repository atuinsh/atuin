-- The session's most recent assistant reply, so a listing can say how each session ended
-- without the reader opening every transcript. `last_reply_at` orders replies that arrive out of
-- order (a reprojection replays records in record order, not message order).
ALTER TABLE sessions ADD COLUMN last_reply TEXT;
ALTER TABLE sessions ADD COLUMN last_reply_at INTEGER;
