-- Builds from before Hermes Agent read its records as an unknown harness
-- (`HarnessKind::Unknown`), and a replay never rewrites a row it already has, so empty the
-- projection (as migration 5 did for Kiro) and let the next reprojection rebuild it from the
-- record store with Hermes's rows named.
INSERT INTO messages_fts (messages_fts) VALUES ('delete-all');
DELETE FROM messages;
DELETE FROM calls;
DELETE FROM regrouped;
DELETE FROM sessions;
DELETE FROM reproject_watermark;
UPDATE projection_state SET generation = generation + 1;
