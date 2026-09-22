-- Contentless (content=''): a full-content fts5 table would keep an uncompressed
-- copy of every searchable body in its `_content` shadow table, duplicating (and
-- dwarfing) the zstd-compressed `messages.content_z`. With content='' only the
-- inverted index remains; previews are derived in Rust from the stored message
-- content instead of via the fts5 aux functions (which need stored content).
-- contentless_delete=1 keeps plain DELETE / INSERT OR REPLACE working.
CREATE VIRTUAL TABLE messages_fts USING fts5(
    title,
    body,
    tokenize = 'unicode61',
    content = '',
    contentless_delete = 1
);
