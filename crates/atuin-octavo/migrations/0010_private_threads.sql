-- Each of a user's threads they made private: closed to every organization it was open to, its
-- shares deleted so that no link opens it, and never opened to an organization again, even one its
-- config names later. `links_revoked` once Octavo has deleted the shares. `requests` counts how
-- often the user asked: each ask sends the deletion again, and one that was reading the shares
-- when a later ask came in doesn't count as done. `queued_at` orders what is still to send.
--
-- Octavo finds a thread by listing the user's, a few pages a pass: `lookup_page_token` is the page
-- the next pass reads, unset to start at the first. A thread that gets a message moves to the
-- front of the list, behind where the search had got to, so `lookup_moved` is set whenever
-- messages of the thread are uploaded while its shares are still to delete, and a search that
-- ends without the thread begins again if it is.
CREATE TABLE private_threads (
    user_id TEXT NOT NULL,
    harness TEXT NOT NULL,
    session_id TEXT NOT NULL,
    queued_at INTEGER NOT NULL DEFAULT (unixepoch()),
    requests INTEGER NOT NULL DEFAULT 1,
    links_revoked INTEGER NOT NULL DEFAULT 0 CHECK (links_revoked IN (0, 1)),
    lookup_page_token TEXT,
    lookup_moved INTEGER NOT NULL DEFAULT 0 CHECK (lookup_moved IN (0, 1)),
    PRIMARY KEY (user_id, harness, session_id)
) WITHOUT ROWID;

CREATE INDEX private_threads_pending ON private_threads (user_id, queued_at)
    WHERE links_revoked = 0;
