-- The users whose account had Octavo off when an upload of theirs was last refused for it. Until
-- one goes through again, what they do is dropped rather than queued to be refused.
CREATE TABLE octavo_off (
    user_id TEXT PRIMARY KEY NOT NULL
) WITHOUT ROWID;
