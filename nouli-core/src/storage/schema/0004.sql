ALTER TABLE request_reservations ADD COLUMN kind TEXT NOT NULL DEFAULT 'evaluation';
CREATE TRIGGER request_kind_no_update BEFORE UPDATE OF kind ON request_reservations BEGIN SELECT RAISE(ABORT,'immutable request kind'); END;
ALTER TABLE workflow_runs ADD COLUMN owner_id TEXT NOT NULL DEFAULT '';
ALTER TABLE workflow_runs ADD COLUMN result_toon TEXT;
ALTER TABLE workflow_runs ADD COLUMN published_revision INTEGER NOT NULL DEFAULT 0;
CREATE TRIGGER run_definition_no_update BEFORE UPDATE OF id,session_id,definition_toon,settings_toon ON workflow_runs BEGIN SELECT RAISE(ABORT,'immutable workflow definition'); END;
CREATE TRIGGER runs_no_delete BEFORE DELETE ON workflow_runs BEGIN SELECT RAISE(ABORT,'immutable workflow identity'); END;
CREATE TABLE workflow_node_intents (
    run_id TEXT NOT NULL REFERENCES workflow_runs(id),
    node_id TEXT NOT NULL,
    round INTEGER NOT NULL CHECK(round >= 0),
    payload_toon TEXT NOT NULL,
    PRIMARY KEY(run_id,node_id,round),
    FOREIGN KEY(run_id,node_id) REFERENCES workflow_nodes(run_id,node_id)
);
CREATE TRIGGER node_intents_no_update BEFORE UPDATE ON workflow_node_intents BEGIN SELECT RAISE(ABORT,'immutable node intent'); END;
CREATE TRIGGER node_intents_no_delete BEFORE DELETE ON workflow_node_intents BEGIN SELECT RAISE(ABORT,'immutable node intent'); END;
CREATE TABLE context_bases (
    id TEXT PRIMARY KEY NOT NULL REFERENCES workflow_runs(id),
    session_id TEXT NOT NULL REFERENCES sessions(id),
    payload_toon TEXT NOT NULL
);
CREATE TRIGGER bases_no_update BEFORE UPDATE ON context_bases BEGIN SELECT RAISE(ABORT,'immutable context base'); END;
CREATE TRIGGER bases_no_delete BEFORE DELETE ON context_bases BEGIN SELECT RAISE(ABORT,'immutable context base'); END;
DROP TRIGGER context_records_no_update;
DROP TRIGGER context_records_no_delete;
ALTER TABLE context_records RENAME TO old_context_records;
CREATE TABLE context_records (
    id TEXT PRIMARY KEY NOT NULL,
    turn_id TEXT REFERENCES turns(id),
    base_id TEXT REFERENCES context_bases(id),
    record_key TEXT NOT NULL,
    payload_toon TEXT NOT NULL,
    CHECK((turn_id IS NULL) != (base_id IS NULL))
);
INSERT INTO context_records(id,turn_id,record_key,payload_toon) SELECT id,turn_id,record_key,payload_toon FROM old_context_records;
DROP TABLE old_context_records;
CREATE TRIGGER context_records_no_update BEFORE UPDATE ON context_records BEGIN SELECT RAISE(ABORT,'immutable context'); END;
CREATE TRIGGER context_records_no_delete BEFORE DELETE ON context_records BEGIN SELECT RAISE(ABORT,'immutable context'); END;

ALTER TABLE attempts ADD COLUMN node_id TEXT;
CREATE INDEX attempts_by_run_node ON attempts(run_id,node_id);
CREATE TABLE workflow_rounds (run_id TEXT NOT NULL REFERENCES workflow_runs(id),round INTEGER NOT NULL CHECK(round >= 0),owner_id TEXT NOT NULL,PRIMARY KEY(run_id,round));
CREATE TRIGGER rounds_no_update BEFORE UPDATE ON workflow_rounds BEGIN SELECT RAISE(ABORT,'immutable workflow round'); END;
CREATE TRIGGER rounds_no_delete BEFORE DELETE ON workflow_rounds BEGIN SELECT RAISE(ABORT,'immutable workflow round'); END;
CREATE TRIGGER node_identity_no_update BEFORE UPDATE OF run_id,node_id,declaration_order ON workflow_nodes BEGIN SELECT RAISE(ABORT,'immutable node identity'); END;
CREATE TRIGGER completed_node_no_update BEFORE UPDATE ON workflow_nodes WHEN OLD.state='completed' BEGIN SELECT RAISE(ABORT,'immutable completed node'); END;
CREATE TRIGGER nodes_no_delete BEFORE DELETE ON workflow_nodes BEGIN SELECT RAISE(ABORT,'immutable node identity'); END;
CREATE TRIGGER completed_run_no_update BEFORE UPDATE ON workflow_runs WHEN OLD.state='completed' BEGIN SELECT RAISE(ABORT,'immutable completed run'); END;
