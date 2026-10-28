CREATE TABLE IF NOT EXISTS sessions (
    id                  TEXT PRIMARY KEY,
    head_id             TEXT,
    server_session_id   TEXT,
    directory           TEXT,
    git_root            TEXT,
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    archived_at         INTEGER
);

CREATE INDEX idx_sessions_directory  ON sessions(directory);
CREATE INDEX idx_sessions_git_root   ON sessions(git_root);
CREATE INDEX idx_sessions_updated_at ON sessions(updated_at);
CREATE INDEX idx_sessions_created_at ON sessions(created_at);

CREATE TABLE IF NOT EXISTS session_events (
    id              TEXT PRIMARY KEY,
    session_id      TEXT NOT NULL,
    parent_id       TEXT,
    invocation_id   TEXT NOT NULL,
    event_type      TEXT NOT NULL,
    event_data      TEXT NOT NULL,
    created_at      INTEGER NOT NULL,

    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE INDEX idx_session_events_session_id    ON session_events(session_id);
CREATE INDEX idx_session_events_parent_id     ON session_events(parent_id);
CREATE INDEX idx_session_events_invocation_id ON session_events(invocation_id);
CREATE INDEX idx_session_events_created_at    ON session_events(created_at);
