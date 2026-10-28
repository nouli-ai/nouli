CREATE TABLE workflow_round_results (
    run_id TEXT NOT NULL,
    round INTEGER NOT NULL,
    state TEXT NOT NULL,
    frontier_toon TEXT NOT NULL,
    published_revision INTEGER NOT NULL,
    payload_toon TEXT NOT NULL,
    PRIMARY KEY(run_id,round),
    FOREIGN KEY(run_id,round) REFERENCES workflow_rounds(run_id,round)
);
INSERT INTO workflow_round_results(run_id,round,state,frontier_toon,published_revision,payload_toon)
SELECT r.id,(SELECT MAX(q.round) FROM workflow_rounds q WHERE q.run_id=r.id),r.state,r.frontier_toon,r.published_revision,r.result_toon
FROM workflow_runs r WHERE r.result_toon IS NOT NULL;
CREATE TRIGGER round_results_no_update BEFORE UPDATE ON workflow_round_results BEGIN SELECT RAISE(ABORT,'immutable round result'); END;
CREATE TRIGGER round_results_no_delete BEFORE DELETE ON workflow_round_results BEGIN SELECT RAISE(ABORT,'immutable round result'); END;
