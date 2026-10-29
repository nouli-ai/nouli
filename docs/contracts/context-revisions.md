# Context revisions

`context::projection::project` projects explicit caller updates over canonical finalized ancestry. Each append, pin, unpin, or replacement has a revision identity consisting of its turn and declared update index. Value and retention changes consume their explicitly selected predecessor revisions. A merge removes causally superseded revisions and retains independent alternatives. Shared ancestors appear once.

A replacement inherits pinned retention from its targets unless `pinned` explicitly changes it. Unpin changes retention and retains the record. Reusing record identity, targeting an unavailable or inactive version, replacing a different key, or creating an unresolved duplicate key fails validation. Records with identical contents retain independent identity.

Independent value or retention revisions produce structured conflicts containing record IDs, tagged turn/run/pending/ephemeral origins and update indices. A multi-target supersession can explicitly replace all conflicting active versions for that key. Selecting an old source frontier continues to project its old revisions. No retrieval order, recency rule, or text contradiction classifier chooses a winner.

`Ledger::context_at` is read-only. Preparation validates proposed updates and saves their audit intent. Finalization validates them again under session ownership, and publishes them together with the successful immutable turn. Failed publication rolls back the new head and context records. Model outcomes remain distinct from caller-designated evidence and commitments.

A workflow can attach one immutable shared caller base event to its node turns. The projector checks its fixed source ancestry and payload identity, then reuses the same causal revisions across roots. This prevents duplicate adoption and keeps runtime-only node provenance distinct from persisted turns.

## Facts

A fact is caller evidence that people can number and correct. `context::facts::append` makes an `Append` of an `Evidence` record whose key is `fact:<record id>` and whose value is the trimmed text. `facts::replace` makes a `Supersede` of the fact's current record with a new record that keeps the same key, so the earlier text stays in the ledger and in turn inspection. Empty or whitespace-only text is rejected with `FactError::Empty`. Each fact has its own key, so blends never create duplicate-key conflicts.

`Ledger::facts` lists the active `fact:` evidence records at a session's head. The `number` is the 1-based position in the order each fact was first added across the canonical ancestry, so a replacement keeps its predecessor's number. A blend lists the facts of every source, and a branch from an older frontier lists only the facts as they were then. `Fact.turn` is the turn that saved the current text, and `Fact.source_session` is set when that turn belongs to another session.

A replaced fact is evicted from model context: its record stays in the manifest as omitted, so the model sees only the current text. A `required_records` entry naming the earlier record still includes it.
