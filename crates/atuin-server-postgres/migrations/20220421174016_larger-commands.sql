-- Make it 4x larger. Most commands are less than this, but as it's base64
-- SOME are more than 8192. Should be enough for now.
ALTER TABLE history ALTER COLUMN data TYPE varchar(32768);
