-- What the reproject watermarks were made against, in a single row.
--
-- `generation` counts invalidations: whatever clears watermarks or deletes projected rows bumps it
-- in the same transaction, and a reprojection moves a watermark only while the generation is still
-- the one it started from. So an invalidation landing mid-reprojection (a maintenance command, or
-- a replay forgetting a host) is noticed even for a series that had no watermark to lose.
--
-- `key_id` is the PASERK id (a public key identifier, not the key) of the encryption key the
-- watermarks were made with. A reprojection under a different key clears every watermark first:
-- what the old key projected, or held back, is no guide to what the new one can read.
CREATE TABLE IF NOT EXISTS projection_state (
    id INTEGER PRIMARY KEY CHECK (id = 0),
    generation INTEGER NOT NULL,
    key_id TEXT
);

INSERT INTO projection_state (id, generation, key_id) VALUES (0, 0, NULL);
