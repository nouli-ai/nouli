CREATE UNIQUE INDEX global_request_ids ON request_reservations(request_id);
ALTER TABLE request_reservations ADD COLUMN frozen_toon TEXT NOT NULL DEFAULT '';
ALTER TABLE request_reservations ADD COLUMN recovery_kind TEXT;
CREATE TRIGGER request_intent_no_update BEFORE UPDATE OF request_id,fingerprint,session_id,frozen_toon ON request_reservations BEGIN SELECT RAISE(ABORT,'immutable request intent'); END;
CREATE TRIGGER completed_request_no_update BEFORE UPDATE ON request_reservations WHEN OLD.state='completed' BEGIN SELECT RAISE(ABORT,'immutable completed request'); END;
CREATE TRIGGER requests_no_delete BEFORE DELETE ON request_reservations BEGIN SELECT RAISE(ABORT,'immutable request reservation'); END;
