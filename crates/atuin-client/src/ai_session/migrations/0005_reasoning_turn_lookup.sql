-- Bound reasoning-count lookups to the rows of one model call.
CREATE INDEX messages_harness_session_turn ON messages (harness, session_id, turn_id);
