-- Add up migration script here
CREATE TABLE scripts (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    script TEXT NOT NULL,
    inserted_at INTEGER NOT NULL DEFAULT 
);

CREATE TABLE script_tags (
    script_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (script_id, tag),
    FOREIGN KEY (script_id) REFERENCES scripts(id)
);