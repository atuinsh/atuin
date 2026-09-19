-- Distinguish "no usage reported" from "zero tokens": the usage_* columns are NOT NULL DEFAULT 0,
-- so a message captured with no usage block is indistinguishable from one reporting zeros. This
-- flag records whether the message carried usage at all, so reads can reconstruct None vs Some(0).
ALTER TABLE messages ADD COLUMN usage_present INTEGER NOT NULL DEFAULT 0;
