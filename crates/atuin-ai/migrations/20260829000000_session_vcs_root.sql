-- The root is the repo root of whatever VCS owns the tree, not git's alone.
ALTER TABLE sessions RENAME COLUMN git_root TO vcs_root;

DROP INDEX IF EXISTS idx_sessions_git_root;
CREATE INDEX idx_sessions_vcs_root ON sessions(vcs_root);
