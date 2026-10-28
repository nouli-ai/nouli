# History and context contract v1

Session, turn, attempt, and context record IDs are opaque UUIDv7 values. Optional
session names are unique, case-sensitive, match `[A-Za-z][A-Za-z0-9._-]{0,63}`,
and cannot look like UUIDs. An unknown continuation/source selector is an error;
only explicit `new` creates a new session. A `from` selection with one captured
source branches and several sources blends. Each source captures a fixed ordered
frontier of finalized turns, never a moving session pointer. An empty session
cannot supply a source until a turn finalizes. Failed/pending/interrupted attempts
are separate records and cannot become source heads.

Finalized turns contain their request, typed model outcome, caller updates,
ordered parent IDs, and evaluation manifest. The manifest retains selected source
points, full/compacted/omitted record IDs, protection, actual serialized TOON
context, provider/model identity, budget, output reserve, and input count
exactness. Logical history events are append-only facts, separate from diagnostics.
Storage enforces immutable finalization and name uniqueness in later tasks.

Context updates accompany an evaluation. `append` creates ordinary evidence or
a caller commitment. `pin` creates a new protected record or targets an existing
reachable active record. `supersede` explicitly targets one or several active
records and adds a replacement, preserving all prior versions. Omitted `pinned`
inherits retention from any target; explicit true/false overrides it. `unpin`
releases retention without deleting the record. Record IDs, rather than reused
keys, identify revisions. Replacements may use the same key as their predecessors.

Only records reachable in the selected ancestry participate in retention or
revision projection. An unrelated session's pinned data cannot enter a turn.
Independent branches replacing the same ancestor produce a structural conflict;
source order cannot silently pick a winner. An attached supersession naming all
conflicting active replacements resolves the conflict. Mere differing values
under matching free-form keys do not prove a structural replacement conflict.

Canonical order is ancestor-first, with caller source order, frontier order, and
workflow node declaration order breaking unrelated ties. Parent lists retain
that declared order. Each turn/record identity appears once, including diamond
ancestry. Reduction uses this stable order for "oldest", never timestamps or
completion order. `history::order::canonical_history` implements the contract's
ordering over finalized records, rejecting missing/foreign heads and cycles.

`fixtures/contracts/history.json` describes continuation, one-source branching,
two-source blending, a diamond, independent conflicting replacements, an unrelated
pinned session, and a failed attempt. It is exercised by inline contract tests.
Active retention projection, conflict resolution, and persistence are later
implementations; these types do not mutate a stored ledger.
