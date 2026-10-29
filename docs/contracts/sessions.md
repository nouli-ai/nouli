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

## Questions are asked once

A session keeps facts and history, not questions. Each turn asks exactly the
questions its request lists, and no later turn, branch, or blend asks them
again; later turns see earlier questions and answers only as history.

Turns written by 0.2.0 may carry `standing`, the `StandingChanges { set, drop }`
of questions that release asked again on every turn, and their attempts carry
`AttemptInput.standing`. Both are still read so 0.2.0 stores open, but nothing
applies them: a 0.2.0 session's saved questions are never asked again. See
[ADR 0004](../adr/0004-ask-each-question-once.md).
