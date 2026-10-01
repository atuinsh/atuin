-- Each session's branches: capture keeps one id per session across machines, so a session resumed
-- on two hosts from the same point, or rewound and continued on one, holds several lines of rows.

-- The line's position in its transcript, when the harness numbers its lines (a Codex rollout
-- line's `ordinal`, `Message::seq`): two hosts continuing one rollout number their lines alike.
-- NULL until a replay fills it in, which the watermarks cleared below cause. (`seq` is the rowid.)
ALTER TABLE messages ADD COLUMN ordinal INTEGER;

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

-- The branch tips (JSON, newest first), the last row they share, and whether hosts differ, all
-- derived from the rows and rebuilt by `AiSessionDatabase::refresh_heads`. `heads_dirty` marks a
-- session whose heads have not been computed since its rows changed: all of them, until the
-- daemon next opens the sidecar.
ALTER TABLE sessions ADD COLUMN heads TEXT;
ALTER TABLE sessions ADD COLUMN branch_point TEXT;
ALTER TABLE sessions ADD COLUMN diverged INTEGER NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN heads_dirty INTEGER NOT NULL DEFAULT 1;

-- How many messages (user prompts and assistant text) the session holds over every branch at
-- once, counted as its heads count theirs. NULL until the heads are worked out with them.
ALTER TABLE sessions ADD COLUMN messages INTEGER;

CREATE INDEX sessions_heads_dirty ON sessions (id) WHERE heads_dirty = 1;

-- Replay every record, to fill in `ordinal`: one invalidation (see the `incremental_sidecar`
-- migration), so a reprojection that read the generation before this does not move a watermark.
DELETE FROM reproject_watermark;
UPDATE projection_state SET generation = generation + 1;
