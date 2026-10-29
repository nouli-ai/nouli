# Ephemeral evaluation

`Engine::evaluate_ephemeral` shares the durable engine's validation, context builder, staged reduction, complete token counting, single-attempt dispatch, bounded retries, timeout, cancellation, and typed outcomes. Its recorder performs no persistence operations.

Fresh calls accept no ledger and do not create a home directory or store. Source-based calls require an existing read-only `LedgerWorker`, resolve fixed source points, and read their immutable ancestry. Proposed context updates affect only the current projection. Provider/model selection uses the caller's explicit settings and request without adopting a source's defaults.

Continuation, durable names, request replay IDs, and interrupted retry are rejected. A new selection or an ordered `From` selection remains valid. The result includes the live request, complete outcome, and actual context manifest, with no persisted session, turn, run, or replay identity. Pending context revisions are labeled pending; existing source provenance retains its finalized IDs.

Equivalent durable and ephemeral inputs dispatch equal TOON evidence and live questions. Read-only source tests compare every store path and file byte before and after success, refusal, failure, source errors, temporary replacements, and compaction. Original active context and saved outcomes remain unchanged.

A source-based ephemeral call reads the standing questions of its single source as they were at the captured frontier, applies `EvaluationRequest.standing` in memory, and asks the resulting questions. The changes are never saved. A new selection starts with no standing questions, and a blend of several sources contributes none.
