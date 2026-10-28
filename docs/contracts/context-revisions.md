# Context revisions

`context::projection::project` projects explicit caller updates over canonical finalized ancestry. Each append, pin, unpin, or replacement has a revision identity consisting of its turn and declared update index. Value and retention changes consume their explicitly selected predecessor revisions. A merge removes causally superseded revisions and retains independent alternatives. Shared ancestors appear once.

A replacement inherits pinned retention from its targets unless `pinned` explicitly changes it. Unpin changes retention and retains the record. Reusing record identity, targeting an unavailable or inactive version, replacing a different key, or creating an unresolved duplicate key fails validation. Records with identical contents retain independent identity.

Independent value or retention revisions produce structured conflicts containing record IDs, tagged turn/run/pending/ephemeral origins and update indices. A multi-target supersession can explicitly replace all conflicting active versions for that key. Selecting an old source frontier continues to project its old revisions. No retrieval order, recency rule, or text contradiction classifier chooses a winner.

`Ledger::context_at` is read-only. Preparation validates proposed updates and saves their audit intent. Finalization validates them again under session ownership, and publishes them together with the successful immutable turn. Failed publication rolls back the new head and context records. Model outcomes remain distinct from caller-designated evidence and commitments.

A workflow can attach one immutable shared caller base event to its node turns. The projector checks its fixed source ancestry and payload identity, then reuses the same causal revisions across roots. This prevents duplicate adoption and keeps runtime-only node provenance distinct from persisted turns.
