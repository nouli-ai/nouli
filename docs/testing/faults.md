# Durability fault matrix

Run `cargo test --package nouli-cli --test faults --locked`. These Unix subprocess tests use isolated temporary homes, loopback HTTP fixtures, and SQLite abort triggers. No hosted endpoint, credential, model download, or real user store is involved.

| Boundary | Injection and assertions |
| --- | --- |
| Concurrent writers | Hold a provider response with a channel barrier. Reject same-session work and duplicate or conflicting request IDs while allowing another session through a separate provider. |
| Owner termination | Kill the owned process after dispatch. Verify the finalized head stays intact, OS writer ownership releases, and restart reports an unknown prior outcome. Explicit retry retains that attempt; subsequent replay makes no HTTP call. |
| Before dispatch | Abort attempt insertion. No provider call or finalized turn is published. Retry succeeds after removing the trigger. |
| After response, during finalization | Abort completion of the request reservation. Turn, manifest, caller record, and head changes roll back together. No successful result is emitted; retry records the prior uncertain call. |
| SQLite COMMIT | A deferred foreign-key trigger stages an invalid reference after all publication statements succeed. COMMIT fails, all publication rows and head changes roll back, writer ownership releases, and explicit retry retains the uncertain attempt. |
| Workflow running state | Abort the persisted running transition. Resume retains the completed initial node and dispatches only unfinished work. |
| Workflow completion | Install an abort trigger from inside the provider handler before returning its response. Node publication rolls back, including its caller record. An unrelated completed sibling survives and is reused on resume. The uncertain call remains visible in inspection and immutable retry rounds. |
| Retry with questions | Abort completion of a request that carried `-q` and `--drop`. The session keeps its original standing questions, a repeat without the flag reports the interruption and sends nothing, and `--retry-interrupted` asks the changed set once and applies the changes once. |
| Ledger from 0.1.0 | `fixtures/contracts/ledger-v0.1.0/` holds a store written by the 0.1.0 binary, with two completed `--request-id` turns and one request killed while its provider call was in flight. `nouli-cli/tests/legacy_ledger.rs` lists, views, and replays the completed requests without a provider call, reports the interrupted request, and retries it against the saved provider address (port 38817, skipped when that port is in use). |
| Migration | Give schema 4 a colliding schema 5 table. Failed writable migration and read-only inspection leave original database bytes and schema version unchanged. |
| Blend conflict | Create incomparable revisions from a pinned common commitment. Blending rejects before inference until an explicit supersede resolves both revisions. Source exports remain unchanged, with all four original turns in the resulting ancestry. |

Related executable checks live in `nouli-cli/tests/evaluation.rs`: unsupported/newer schemas, read-only old schemas, explicit resume against an advanced head, Ctrl-C cancellation, and stdout backpressure with an open reader. `nouli-cli/tests/journeys.rs` checks protected-context overflow before inference across all three protocols and source snapshot retention. Core storage tests cover immutable rows, event ordering, orphan retry rounds, and concurrent resume ownership.

The test harness owns and joins server threads, kills and reaps unfinished children, closes database connections before temporary-home removal, and avoids a second panic during destructor unwinding. Channel barriers have bounded waits and release on sender disconnection. Lock files deliberately remain stable: operating-system lock ownership releases when the owner exits, rather than unlinking an inode that another process may already have opened.

An interrupted call can have an unknown provider outcome. Recovery preserves that fact and never synthesizes an answer. An explicit retry may repeat inference, while completed request replay and completed workflow nodes reuse their original results. Tests do not assert exactly-once remote execution.
