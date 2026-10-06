ALTER TABLE sessions ADD COLUMN atuin_id BLOB;

-- Builds before record versions skip v1 records as undecodable: replay everything once.
DELETE FROM reproject_watermark;
UPDATE projection_state SET generation = generation + 1;
