-- A session's branches: capture keeps one id per session across machines, so a session resumed on
-- two hosts from the same point, or rewound and continued on one, holds several lines of rows.
-- Everything here is derived from the rows and rebuilt by `AiSessionDatabase::refresh_heads`.

-- The line's position in its transcript, when the harness numbers its lines (a Codex rollout
-- line's `ordinal`, `Message::seq`): two hosts continuing one rollout number their lines alike.
ALTER TABLE messages ADD COLUMN seq INTEGER;

-- 1 for a row that makes a branch worth telling apart: a user prompt, or assistant text. Parallel
-- tool calls, attachments and compaction branch the tree too, but hold neither. NULL until
-- computed: rows whose content is compressed are decoded when the daemon opens the sidecar.
ALTER TABLE messages ADD COLUMN substantive INTEGER;

-- The stored row the parent pointer resolves to, within the session (`Message::parent_row`). The
-- same id as parent_source_id for most harnesses; opencode's rows are parts while their pointer
-- names a message (`msg_`), whose first part it resolves to.
ALTER TABLE messages ADD COLUMN parent_row TEXT;

UPDATE messages SET substantive = (role = '"User"') WHERE role <> '"Assistant"';
UPDATE messages SET substantive = EXISTS (
    SELECT 1 FROM json_each(messages.content) c
    WHERE json_type(c.value, '$.Text') = 'text' AND trim(json_extract(c.value, '$.Text')) <> ''
)
WHERE role = '"Assistant"' AND content_z IS NULL AND json_valid(content);

-- The branch tips (JSON, newest first), the last row they share, and whether hosts differ.
-- `heads_dirty` marks a session whose heads have not been computed since its rows changed: all of
-- them, until the daemon next opens the sidecar.
ALTER TABLE sessions ADD COLUMN heads TEXT;
ALTER TABLE sessions ADD COLUMN branch_point TEXT;
ALTER TABLE sessions ADD COLUMN diverged INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN heads_dirty INTEGER NOT NULL DEFAULT 1;
CREATE INDEX sessions_heads_dirty ON sessions (harness, session_id) WHERE heads_dirty = 1;

-- Records from hosts on a newer build may already carry `seq`, which only a replay stores: forget
-- the watermarks, so the next daemon start replays every record (a replayed row fills in a
-- missing `seq`, as it does a missing host).
DELETE FROM reproject_watermark;
UPDATE projection_state SET generation = generation + 1 WHERE id = 0;
