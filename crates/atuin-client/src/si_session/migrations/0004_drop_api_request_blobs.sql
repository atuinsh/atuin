-- Capture read Claude Code's `api-request-blob` lines (2.1.300+), the messages of each API
-- request as sent, as rows of the conversation they repeat. Projection now skips those rows, but
-- a replay never deletes a row, so empty the projection (as migration 2 does) and let the next
-- reprojection rebuild it from the record store without them.
INSERT INTO messages_fts (messages_fts) VALUES ('delete-all');
DELETE FROM messages;
DELETE FROM calls;
DELETE FROM regrouped;
DELETE FROM sessions;
DELETE FROM reproject_watermark;
UPDATE projection_state SET generation = generation + 1;
