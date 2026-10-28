-- Cache of the server-reported credit usage snapshot, keyed by a hash of
-- the auth token (the client never learns its hub user id). One row per
-- key; snapshot is the JSON `credits` object from the hub.
CREATE TABLE IF NOT EXISTS usage (
    user_key    TEXT NOT NULL,
    snapshot    TEXT NOT NULL,
    updated_at  INTEGER NOT NULL,

    PRIMARY KEY (user_key)
);
