ALTER TABLE session ADD COLUMN fingerprint_source_path TEXT;
ALTER TABLE session ADD COLUMN fingerprint_checked_at_epoch INTEGER;
ALTER TABLE session ADD COLUMN fingerprint_next_attempt_at_epoch INTEGER;
ALTER TABLE session ADD COLUMN fingerprint_retry_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE session ADD COLUMN fingerprint_in_flight INTEGER NOT NULL DEFAULT 0;
CREATE INDEX session_fingerprint_due ON session(fingerprint_next_attempt_at_epoch)
    WHERE fingerprint_source_path IS NOT NULL;
