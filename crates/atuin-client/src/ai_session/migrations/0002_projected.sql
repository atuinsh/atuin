-- How far each host's ai-session record chain is projected into this sidecar: the idx the next
-- projection starts from. Advanced contiguously only, so a record that failed to project is
-- reached again by the next build rather than skipped for good.
CREATE TABLE projected (
    host TEXT PRIMARY KEY,
    next_idx INTEGER NOT NULL
);
