-- Written by hand, after `hermes_state_messages.py` (Hermes main d70cbc5205), since `/compress`
-- refused every probe session: an in-place compaction, a legacy rotated one, and sessions that
-- are no coding work. Load after schema.sql.

-- In-place: rows 101-104 summarized away (`compacted = 1`), the hidden summary 105, and the
-- carried tail as copies under the same `message_uid` (106, 107). Row 104 arrived while the
-- summary was being written: Hermes cloned it (107) and left the original neither active nor
-- compacted. Then a turn after the compaction, and rows Hermes never shows.
INSERT INTO sessions (id, source, created_source, model, model_config, started_at, cwd, title, title_source, last_activity_at)
    VALUES ('local-compacted', 'local', 'cli', 'xiaomi/mimo-v2.5-pro',
            '{"gateway_runtime": {"provider": "openrouter"}}', 1794250000.0, '/tmp/hermes-play/work',
            'Compaction probe', 'llm', 1794250090.0);
INSERT INTO messages (id, session_id, role, content, timestamp, finish_reason, active, compacted, _compressed_summary, display_kind, display_metadata, observed, message_uid) VALUES
    (101, 'local-compacted', 'user', 'first', 1794250001.0, NULL, 0, 1, 0, NULL, NULL, 0, 'u-101'),
    (102, 'local-compacted', 'assistant', 'first reply', 1794250002.0, 'stop', 0, 1, 0, NULL, NULL, 0, 'u-102'),
    (103, 'local-compacted', 'user', 'second', 1794250003.0, NULL, 0, 1, 0, NULL, NULL, 0, 'u-103'),
    (104, 'local-compacted', 'assistant', 'second reply', 1794250004.0, 'stop', 0, 0, 0, NULL, NULL, 0, 'u-104'),
    (105, 'local-compacted', 'user', '[CONTEXT COMPACTION] Earlier turns: first, second.', 1794250005.0, NULL, 1, 0, 1, 'hidden', NULL, 0, 'u-105'),
    (106, 'local-compacted', 'user', 'second', 1794250003.0, NULL, 1, 0, 0, NULL, NULL, 0, 'u-103'),
    (107, 'local-compacted', 'assistant', 'second reply', 1794250004.0, 'stop', 1, 0, 0, NULL, NULL, 0, 'u-104'),
    (108, 'local-compacted', 'user', 'third', 1794250010.0, NULL, 1, 0, 0, NULL, NULL, 0, 'u-108'),
    (109, 'local-compacted', 'user', 'internal notice', 1794250011.0, NULL, 1, 0, 0, NULL, '{"model_only": true}', 0, 'u-109'),
    (110, 'local-compacted', 'user', 'someone else in the group', 1794250012.0, NULL, 1, 0, 0, NULL, NULL, 1, 'u-110'),
    (111, 'local-compacted', 'assistant', '', 1794250013.0, NULL, 1, 0, 0, NULL, NULL, 0, 'u-111'),
    (112, 'local-compacted', 'user', 'and steer this way', 1794250014.0, NULL, 1, 0, 0, 'steer', NULL, 0, 'u-112'),
    (113, 'local-compacted', 'assistant', 'third reply', 1794250015.0, 'stop', 1, 0, 0, NULL, NULL, 0, 'u-113');

-- Legacy rotation (`compression.in_place: false`): the parent ended for compression, and its
-- child carries the conversation on under a new id and a numbered title.
INSERT INTO sessions (id, source, created_source, model, started_at, ended_at, end_reason, cwd, title, title_source)
    VALUES ('20260511_171303_982a49', 'cli', NULL, 'xiaomi/mimo-v2.5-pro', 1794240000.0, 1794240100.0,
            'compression', '/tmp/hermes-play/work', 'Git rebase', 'derived');
INSERT INTO sessions (id, source, created_source, model, parent_session_id, started_at, cwd, title, title_source)
    VALUES ('20260511_172545_31d799', 'cli', NULL, 'xiaomi/mimo-v2.5-pro', '20260511_171303_982a49',
            1794240100.0, '/tmp/hermes-play/work', 'Git rebase #2', 'derived');
INSERT INTO messages (id, session_id, role, content, timestamp, finish_reason, message_uid) VALUES
    (201, '20260511_171303_982a49', 'user', 'rebase onto main', 1794240001.0, NULL, 'u-201'),
    (202, '20260511_171303_982a49', 'assistant', 'done', 1794240002.0, 'stop', 'u-202'),
    (203, '20260511_172545_31d799', 'user', 'now push it', 1794240101.0, NULL, 'u-203'),
    (204, '20260511_172545_31d799', 'assistant', 'pushed', 1794240102.0, 'stop', 'u-204');

-- No coding work: a Telegram chat, a cron job, a tool run, and a local session Hermes's Desktop
-- created (which is).
INSERT INTO sessions (id, source, created_source, started_at, cwd) VALUES
    ('telegram-1', 'telegram', 'telegram', 1794230000.0, NULL),
    ('cron_backup_1', 'cron', 'cron', 1794230001.0, NULL),
    ('local-tool', 'local', 'tool', 1794230002.0, '/tmp/hermes-play'),
    ('local-desktop', 'local', 'desktop', 1794230003.0, '/tmp/hermes-play');
INSERT INTO messages (id, session_id, role, content, timestamp) VALUES
    (301, 'telegram-1', 'user', 'hi from telegram', 1794230000.5),
    (302, 'cron_backup_1', 'user', 'run the backup', 1794230001.5),
    (303, 'local-tool', 'user', 'tool run', 1794230002.5),
    (304, 'local-desktop', 'user', 'from the desktop app', 1794230003.5);
