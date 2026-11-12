-- A store Hermes v2026.9.24 (schema version 30) wrote: no `created_source`, no `message_uid`,
-- and the CLI's sessions under `source = 'cli'`. Columns trimmed as in schema.sql. Load alone.
CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version (version) VALUES (30);
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    source TEXT NOT NULL,
    model TEXT,
    model_config TEXT,
    parent_session_id TEXT,
    started_at REAL NOT NULL,
    ended_at REAL,
    end_reason TEXT,
    cwd TEXT,
    title TEXT,
    title_source TEXT,
    last_activity_at REAL,
    rewind_count INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    role TEXT NOT NULL,
    content TEXT,
    tool_call_id TEXT,
    tool_calls TEXT,
    tool_name TEXT,
    timestamp REAL NOT NULL,
    finish_reason TEXT,
    reasoning TEXT,
    observed INTEGER DEFAULT 0,
    _compressed_summary INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1,
    compacted INTEGER NOT NULL DEFAULT 0,
    display_kind TEXT,
    display_metadata TEXT
);
INSERT INTO sessions (id, source, model, started_at, ended_at, end_reason, cwd, title, title_source) VALUES
    ('20260514_220652_40bb26', 'cli', 'gpt-5', 1778796412.0, 1778796500.0, 'cli_close', '/tmp/hermes-play', 'Old CLI session', NULL),
    ('20260514_230000_aaaaaa', 'tui', 'gpt-5', 1778800000.0, NULL, NULL, '/tmp/hermes-play', NULL, NULL),
    ('20260514_235959_bbbbbb', 'telegram', 'gpt-5', 1778803199.0, NULL, NULL, NULL, NULL, NULL);
INSERT INTO messages (id, session_id, role, content, tool_call_id, tool_calls, tool_name, timestamp, finish_reason) VALUES
    (1, '20260514_220652_40bb26', 'user', 'list the files', NULL, NULL, NULL, 1778796413.0, NULL),
    (2, '20260514_220652_40bb26', 'assistant', '', NULL, '[{"id": "call_1", "type": "function", "function": {"name": "terminal", "arguments": "{\"command\": \"ls\"}"}}]', NULL, 1778796414.0, 'tool_calls'),
    (3, '20260514_220652_40bb26', 'tool', '{"output": "a\nb", "exit_code": 0, "error": null}', 'call_1', NULL, NULL, 1778796415.0, NULL),
    (4, '20260514_220652_40bb26', 'assistant', 'a and b', NULL, NULL, NULL, 1778796416.0, 'stop'),
    (5, '20260514_230000_aaaaaa', 'user', 'from the tui', NULL, NULL, NULL, 1778800001.0, NULL),
    (6, '20260514_235959_bbbbbb', 'user', 'from telegram', NULL, NULL, NULL, 1778803200.0, NULL);
