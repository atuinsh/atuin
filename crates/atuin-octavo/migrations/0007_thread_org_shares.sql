-- Each organization a user's config had one of their threads shared with, by the harness's names
-- for the thread, as only Octavo knows its id. `sent` once Octavo has heard. The row stays, so a
-- thread the user has since unshared isn't shared again by its next message.
CREATE TABLE thread_org_shares (
    user_id TEXT NOT NULL,
    harness TEXT NOT NULL,
    session_id TEXT NOT NULL,
    org_slug TEXT NOT NULL COLLATE NOCASE,
    queued_at INTEGER NOT NULL DEFAULT (unixepoch()),
    sent INTEGER NOT NULL DEFAULT 0 CHECK (sent IN (0, 1)),
    PRIMARY KEY (user_id, harness, session_id, org_slug)
) WITHOUT ROWID;

CREATE INDEX thread_org_shares_pending ON thread_org_shares (user_id, queued_at) WHERE sent = 0;

-- The organizations a user's config names that they were warned they don't belong to, so they are
-- warned once.
CREATE TABLE skipped_share_orgs (
    user_id TEXT NOT NULL,
    org_slug TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (user_id, org_slug)
) WITHOUT ROWID;
