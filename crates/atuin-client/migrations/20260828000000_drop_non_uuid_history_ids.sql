-- `HistoryId` is now a UUID: rows are read via `try_get::<String>().parse()`, so
-- an id that is not a valid UUID fails to decode and breaks every history query,
-- not just its own row. Atuin has only ever minted ids as the 32-char simple
-- (hyphen-less) UUID form, so drop anything that isn't exactly that. On a healthy
-- database this matches every row and deletes nothing.
--
-- The character class allows upper-case hex too: `Uuid::parse_str` accepts it and
-- the next write normalises it back to lower-case, so such rows still load fine.
delete from history where length(id) <> 32 or id glob '*[^0-9a-fA-F]*';
