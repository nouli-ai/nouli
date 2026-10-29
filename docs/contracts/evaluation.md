# Durable evaluation engine

`Engine::evaluate` accepts an explicit `LedgerWorker`, request, configuration, runtime overrides, home path, credential lookup, cancellation token, provider, counter, and retry clock. It reads no process globals and requires no CLI parser. `NativeProvider` selects the configured TypeSafe, OpenAI Decisions, or OpenRouter adapter; injected providers implement a single cancellation-safe attempt.

Explicit model overrides are normalized into submitted intent. Missing request IDs receive a generated ID returned to the caller. Existing reservations retain their settings, source points, and session identity. Completed replay returns the immutable saved turn without resolving current defaults or credentials. Interrupted work requires explicit retry, preserving unknown remote outcomes.

The engine validates selected ancestry and context revisions, plans protected evidence and staged reduction, then records an attempt and actual manifest before dispatch. Provider waits, timeout, retry delay, and cancellation occur outside SQLite transactions. Current caller updates are labeled pending in served evidence; persisted historical provenance uses actual finalized IDs. A returned saved result follows successful atomic finalization.

Only classified transient failures receive bounded retries. Each actual call has a separate append-only intent and progress record. Cancellation or timeout can leave remote execution unknown, and that evidence survives future owner recovery. A capacity rejection can lower the effective budget and replan within the same attempt limit, only when the dispatched evidence becomes smaller. Mandatory overflow or an unchanged oversized payload stops further calls. Provider/model selection never falls back automatically.

Results retain independent per-question refusals, distributions, fractional score values, native confidence, usage, and actual served metadata. A finalization failure retains the running audit attempt and cannot report saved success. `Ledger::list_attempts` exposes immutable intents and projected states in acceptance order.

`LedgerWorker::open` opens the store on a blocking worker. Its jobs own their connection lock and all captured guard references until each finite operation finishes, including when an awaiting future is dropped. Provider waits never run inside a worker job. This keeps SQLite busy waits and filesystem operations off the async executor.

## Questions

A turn asks exactly `turn.questions`, which must hold at least one question;
an empty list fails with the empty `questions` contract error, reported as
`questions_required`. The finalized turn's `request.questions` records what was
asked. `EvaluationRequest.standing` is read only so requests saved by 0.2.0
still decode: a new request that sets it fails with `HistoryError::Standing`
(`invalid_standing_question`) before the request is reserved, and a stored one
is never applied.
