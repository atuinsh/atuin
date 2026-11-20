-- Builds from before Kilo Code read its records as its own harness
-- (`HarnessKind::Kilo`), and a replay never rewrites a row it already has, so empty the
-- projection (as migration 5 did for Kiro and 6 for Hermes) and let the next reprojection
-- rebuild it from the record store with Kilo's rows named instead of unknown.
INSERT INTO messages_fts (messages_fts) VALUES ('delete-all');
DELETE FROM messages;
DELETE FROM calls;
DELETE FROM regrouped;
DELETE FROM sessions;
DELETE FROM reproject_watermark;
UPDATE projection_state SET generation = generation + 1;
