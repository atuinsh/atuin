-- Whether a tool result reported failure, and why an assistant turn ended (StopReason discriminant).
ALTER TABLE messages ADD COLUMN is_error INTEGER NOT NULL DEFAULT 0;
ALTER TABLE messages ADD COLUMN stop_reason INTEGER;
