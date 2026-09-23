-- Which revision of its source id a message is, where the harness re-emits one (opencode
-- persists a part on every upsert). NULL is a harness that writes each message once.
ALTER TABLE messages ADD COLUMN revision INTEGER;
