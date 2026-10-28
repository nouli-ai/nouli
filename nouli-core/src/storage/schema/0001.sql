CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL,
    name TEXT UNIQUE COLLATE BINARY,
    defaults_toon TEXT NOT NULL,
    frontier_toon TEXT NOT NULL,
    revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0)
);
CREATE TABLE session_sources (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    position INTEGER NOT NULL CHECK(position >= 0),
    source_session TEXT NOT NULL REFERENCES sessions(id),
    frontier_toon TEXT NOT NULL,
    PRIMARY KEY(session_id, position)
);
CREATE TABLE turns (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    ordinal INTEGER NOT NULL CHECK(ordinal >= 0),
    payload_toon TEXT NOT NULL,
    UNIQUE(session_id, ordinal)
);
CREATE TABLE turn_parents (
    turn_id TEXT NOT NULL REFERENCES turns(id),
    parent_id TEXT NOT NULL REFERENCES turns(id),
    position INTEGER NOT NULL CHECK(position >= 0),
    PRIMARY KEY(turn_id, position),
    UNIQUE(turn_id, parent_id),
    CHECK(turn_id != parent_id)
);
CREATE TABLE context_records (
    id TEXT PRIMARY KEY NOT NULL,
    turn_id TEXT NOT NULL REFERENCES turns(id),
    record_key TEXT NOT NULL,
    payload_toon TEXT NOT NULL
);
CREATE TABLE context_events (
    turn_id TEXT NOT NULL REFERENCES turns(id),
    position INTEGER NOT NULL CHECK(position >= 0),
    payload_toon TEXT NOT NULL,
    PRIMARY KEY(turn_id, position)
);
CREATE TABLE request_reservations (
    session_id TEXT NOT NULL REFERENCES sessions(id),
    request_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    owner_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','completed','interrupted')),
    result_toon TEXT,
    PRIMARY KEY(session_id, request_id)
);
CREATE TABLE attempts (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT NOT NULL REFERENCES sessions(id),
    run_id TEXT REFERENCES workflow_runs(id),
    turn_id TEXT REFERENCES turns(id),
    payload_toon TEXT NOT NULL
);
CREATE TABLE manifests (
    turn_id TEXT PRIMARY KEY NOT NULL REFERENCES turns(id),
    payload_toon TEXT NOT NULL
);
CREATE TABLE workflow_runs (
    id TEXT PRIMARY KEY NOT NULL,
    session_id TEXT REFERENCES sessions(id),
    definition_toon TEXT NOT NULL,
    settings_toon TEXT NOT NULL,
    state TEXT NOT NULL,
    frontier_toon TEXT NOT NULL
);
CREATE TABLE workflow_nodes (
    run_id TEXT NOT NULL REFERENCES workflow_runs(id),
    node_id TEXT NOT NULL,
    declaration_order INTEGER NOT NULL CHECK(declaration_order >= 0),
    state TEXT NOT NULL,
    turn_id TEXT REFERENCES turns(id),
    payload_toon TEXT,
    PRIMARY KEY(run_id, node_id),
    UNIQUE(run_id, declaration_order)
);
CREATE TABLE workflow_events (
    run_id TEXT NOT NULL REFERENCES workflow_runs(id),
    sequence INTEGER NOT NULL CHECK(sequence >= 0),
    payload_toon TEXT NOT NULL,
    PRIMARY KEY(run_id, sequence)
);
CREATE INDEX turn_session_order ON turns(session_id, ordinal);
CREATE INDEX parent_descendants ON turn_parents(parent_id);
CREATE INDEX attempts_by_session ON attempts(session_id);
CREATE INDEX attempts_by_run ON attempts(run_id);
CREATE TRIGGER turns_no_update BEFORE UPDATE ON turns BEGIN SELECT RAISE(ABORT,'immutable turn'); END;
CREATE TRIGGER turns_no_delete BEFORE DELETE ON turns BEGIN SELECT RAISE(ABORT,'immutable turn'); END;
CREATE TRIGGER parents_no_update BEFORE UPDATE ON turn_parents BEGIN SELECT RAISE(ABORT,'immutable ancestry'); END;
CREATE TRIGGER parents_no_delete BEFORE DELETE ON turn_parents BEGIN SELECT RAISE(ABORT,'immutable ancestry'); END;
CREATE TRIGGER context_records_no_update BEFORE UPDATE ON context_records BEGIN SELECT RAISE(ABORT,'immutable context'); END;
CREATE TRIGGER context_records_no_delete BEFORE DELETE ON context_records BEGIN SELECT RAISE(ABORT,'immutable context'); END;
CREATE TRIGGER context_events_no_update BEFORE UPDATE ON context_events BEGIN SELECT RAISE(ABORT,'immutable context event'); END;
CREATE TRIGGER context_events_no_delete BEFORE DELETE ON context_events BEGIN SELECT RAISE(ABORT,'immutable context event'); END;
CREATE TRIGGER sources_no_update BEFORE UPDATE ON session_sources BEGIN SELECT RAISE(ABORT,'immutable source'); END;
CREATE TRIGGER sources_no_delete BEFORE DELETE ON session_sources BEGIN SELECT RAISE(ABORT,'immutable source'); END;
CREATE TRIGGER manifests_no_update BEFORE UPDATE ON manifests BEGIN SELECT RAISE(ABORT,'immutable manifest'); END;
CREATE TRIGGER manifests_no_delete BEFORE DELETE ON manifests BEGIN SELECT RAISE(ABORT,'immutable manifest'); END;
CREATE TRIGGER events_no_update BEFORE UPDATE ON workflow_events BEGIN SELECT RAISE(ABORT,'immutable event'); END;
CREATE TRIGGER events_no_delete BEFORE DELETE ON workflow_events BEGIN SELECT RAISE(ABORT,'immutable event'); END;
