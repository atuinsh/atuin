-- Builds from before undo markers read them as blocks of a kind they didn't know
-- (`Content::Unknown`), and a replay never rewrites a row it already has. So empty the projection
-- (as migration 6 did for Hermes) and let the next reprojection rebuild it from the record store,
-- with each marker read as `Content::Undone`.
INSERT INTO messages_fts (messages_fts) VALUES ('delete-all');
DELETE FROM messages;
DELETE FROM calls;
DELETE FROM regrouped;
DELETE FROM sessions;
DELETE FROM reproject_watermark;
UPDATE projection_state SET generation = generation + 1;
