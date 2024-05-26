create virtual table history_fts using fts5(command, cwd, hostname, exit, content='history', tokenize="unicode61 tokenchars '@-_$'");

insert into history_fts(rowid, command, cwd, exit, hostname) select rowid, command, cwd, exit, hostname from history;

-- Keep the index up to date
-- We do not ever update rows
-- for big changes, Atuin can just rebuild the history db from the store
CREATE TRIGGER history_fts_begin AFTER INSERT ON history BEGIN
  INSERT INTO history_fts(rowid, command, cwd, exit, hostname) VALUES (new.rowid, new.command, new.cwd, new.exit, new.hostname);
END;

CREATE TRIGGER history_fts_delete AFTER DELETE ON history BEGIN
  INSERT INTO history_fts(history_fts, rowid, command, cwd, exit, hostname) VALUES('delete', old.rowid, old.command, old.cwd, old.exit, old.hostname);
END;
