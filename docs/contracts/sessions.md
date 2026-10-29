# Durable sessions and attempts

Session catalogs use generated UUIDv7 IDs and optional unique, case-sensitive
names. Lookup is explicit by ID/name. Unknown selectors never create a session.
Catalog metadata contains model defaults and an ordered finalized head frontier.
`create_session`, `session`, and `list_sessions` expose these records.

`prepare_attempt` persists accepted requests, caller updates, a complete context
manifest, request identity, and the expected head/revision before provider dispatch.
Intent is immutable. Attempt progress and append-only progress events are separate;
pending/running work can fail or be interrupted without changing the finalized head.
`attempt` reads intent, state, latest event, and finalized turn link in one SQLite
snapshot. `list_attempts` keeps failed and pending submissions inspectable.

`finalize_attempt` validates the typed result and publishes the full turn, ordered
parents, context records/events, actual served provider identity, manifest,
completed-attempt link, logical head event, and new session frontier in one short
transaction. A stale expected revision fails with a conflict. Database failure
does not return saved success or leave a half-published head. Native refusal is a
valid per-question outcome and does not discard answered siblings.

`turn`, `manifest`, and `ancestry` retain complete original state/options/rubrics,
distributions, uncertainty, usage, caller commitments, and fixed source metadata.
Ancestry reads only reachable immutable IDs and applies canonical ancestor-first
ordering, with shared records deduplicated. Structured null remains a real value;
failed/unfinished attempts are separate records, never finalized source heads.

Schema v2 adds immutable accepted-attempt intents, append-only attempt/session
events, and separate mutable attempt-state projections. Existing v1 catalogs
migrate atomically. The repository stores no credential values and performs no
HTTP calls. Request replay, process guards, active revision/conflict projection,
and source branch/blend creation are subsequent task responsibilities.

## Standing questions

A session remembers the questions it keeps asking. The ledger has no separate
table for them: each finalized turn carries `standing`, the
`StandingChanges { set, drop }` it made, saved in the same transaction as the
turn (`AttemptInput.standing` becomes `FinalizedTurn.standing`). Turns written
by 0.1.0 have no such field and read as having made no changes. A failed,
interrupted, or retried attempt changes nothing, because only a finalized turn
carries changes.

A session's standing set is a fold over its own turns' changes in canonical
ancestry order, on top of a base. `set` inserts a question or replaces the one
with the same ID in place, and `drop` removes one, so the set keeps the order in
which questions were first set. A session created from exactly one source (a
branch) starts from the source's set at the captured frontier, computed the same
way and recursively, so a branch from an older point inherits the set as it was
then. A new session, or one created from several sources (a blend), starts with
no standing questions.

`Ledger::standing_questions(&SessionSelector)` returns the current set, and
`SessionExport.standing_questions` includes it. Neither the set nor the
inheritance rule is stored anywhere but in the turns, so they cannot disagree
with the history.
