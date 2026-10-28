-- Which kinds of content block each message holds (`ContentBlocks`), so queries can tell a
-- person's prompt from tool output sent back as a user turn without decoding the content.
ALTER TABLE messages ADD COLUMN blocks INTEGER NOT NULL DEFAULT 0;

-- Rows already stored have no blocks, and a replay skips rows it already has, so empty the
-- projection (as `AiSessionDatabase::reset` does) and let the next reprojection rebuild it from
-- the record store with blocks filled in.
INSERT INTO messages_fts (messages_fts) VALUES ('delete-all');
DELETE FROM messages;
DELETE FROM calls;
DELETE FROM regrouped;
DELETE FROM sessions;
DELETE FROM reproject_watermark;
UPDATE projection_state SET generation = generation + 1;
