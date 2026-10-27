-- Who served a row's model (an `interned` id), for the harnesses that reopen a session on a
-- model by provider (pi, omp, opencode). No row stored so far has one, so nothing is replayed.
ALTER TABLE messages ADD COLUMN provider INTEGER;
