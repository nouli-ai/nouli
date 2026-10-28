CREATE TABLE attempt_states (
    attempt_id TEXT PRIMARY KEY NOT NULL REFERENCES attempts(id),
    state TEXT NOT NULL CHECK(state IN ('pending','running','completed','failed','interrupted')),
    finalized_turn TEXT REFERENCES turns(id)
);
CREATE TABLE attempt_events (
    attempt_id TEXT NOT NULL REFERENCES attempts(id),
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    payload_toon TEXT NOT NULL,
    PRIMARY KEY(attempt_id, sequence)
);
CREATE TABLE session_events (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    payload_toon TEXT NOT NULL,
    PRIMARY KEY(session_id, sequence)
);
CREATE TRIGGER attempts_no_update BEFORE UPDATE ON attempts BEGIN SELECT RAISE(ABORT,'immutable attempt intent'); END;
CREATE TRIGGER attempts_no_delete BEFORE DELETE ON attempts BEGIN SELECT RAISE(ABORT,'immutable attempt intent'); END;
CREATE TRIGGER attempt_events_no_update BEFORE UPDATE ON attempt_events BEGIN SELECT RAISE(ABORT,'immutable attempt event'); END;
CREATE TRIGGER attempt_events_no_delete BEFORE DELETE ON attempt_events BEGIN SELECT RAISE(ABORT,'immutable attempt event'); END;
CREATE TRIGGER session_events_no_update BEFORE UPDATE ON session_events BEGIN SELECT RAISE(ABORT,'immutable session event'); END;
CREATE TRIGGER session_events_no_delete BEFORE DELETE ON session_events BEGIN SELECT RAISE(ABORT,'immutable session event'); END;
