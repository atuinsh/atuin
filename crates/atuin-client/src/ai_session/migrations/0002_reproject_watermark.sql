-- How far the sidecar has been reprojected from each record series (one host's records of one
-- tag): every record at or below `idx` is projected. `record_id` is the id of the record at `idx`,
-- so a series rewritten under the watermark (a reset, deleted or re-keyed store) is noticed and
-- replayed from the start. No row means replay that series from the start, so a later migration
-- needing a backfill runs `DELETE FROM reproject_watermark;`.
CREATE TABLE IF NOT EXISTS reproject_watermark (
    host TEXT NOT NULL,
    tag TEXT NOT NULL,
    idx INTEGER NOT NULL,
    record_id TEXT NOT NULL,
    PRIMARY KEY (host, tag)
);
