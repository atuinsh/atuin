-- Add up migration script here
CREATE TABLE scripts (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    shebang TEXT NOT NULL,
    script TEXT NOT NULL,
    inserted_at INTEGER NOT NULL DEFAULT (strftime('%s', 'now'))
);

CREATE TABLE script_tags (
    id INTEGER PRIMARY KEY,
    script_id TEXT NOT NULL,
    tag TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_script_tags ON script_tags (script_id, tag);