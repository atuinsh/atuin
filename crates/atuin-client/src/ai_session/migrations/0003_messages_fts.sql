CREATE VIRTUAL TABLE messages_fts USING fts5(
    title,
    body,
    tokenize = 'unicode61'
);
