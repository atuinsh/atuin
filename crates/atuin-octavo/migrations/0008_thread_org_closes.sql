-- How far each of a thread's organizations got, in place of `sent`: 0 while queued to be opened, 1
-- once Octavo has heard, 2 while queued to be closed, as the thread went where the organization's
-- members mustn't follow, and 3 once Octavo has heard that. The row stays at 3 too, so a thread
-- closed to an organization isn't shared with it again by its next message.
ALTER TABLE thread_org_shares
    ADD COLUMN state INTEGER NOT NULL DEFAULT 0 CHECK (state IN (0, 1, 2, 3));
UPDATE thread_org_shares SET state = sent;
DROP INDEX thread_org_shares_pending;
ALTER TABLE thread_org_shares DROP COLUMN sent;
CREATE INDEX thread_org_shares_pending ON thread_org_shares (user_id, state, queued_at)
    WHERE state IN (0, 2);

-- Each directory one of a user's threads had a row uploaded from, with the organizations its config
-- shared threads with when atuin first looked, as a JSON list of slugs. A thread opens, and stays
-- open, to an organization only while each of its directories shares with it.
CREATE TABLE thread_dirs (
    user_id TEXT NOT NULL,
    harness TEXT NOT NULL,
    session_id TEXT NOT NULL,
    dir TEXT NOT NULL,
    org_slugs TEXT NOT NULL,
    PRIMARY KEY (user_id, harness, session_id, dir)
) WITHOUT ROWID;
