# Embedding Nouli

Nouli is a Rust decision-session engine with a thin CLI. Callers supply typed questions, evidence, caller commitments, and acyclic workflow graphs. The library does not read process environment variables, exit the process, install a tracing subscriber, execute tools, or generate a plan. Its transport sends a single selected-provider attempt; the engine owns bounded retry, cancellation, context construction, and durable publication.

Run the complete credential-free example with `cargo run --package nouli-core --example embedding --locked --offline`. Its test also runs under `cargo test --workspace --all-targets --locked`. `nouli-core/examples/embedding.rs` uses an injected fake provider and an isolated temporary store. It demonstrates new/continued decisions, request replay, fixed-source branch/blend, pinned caller commitment, attached unpin and required records, read-only ephemeral evaluation, pre-dispatch cancellation, typed outcomes/refusal, workflow events, failed-run recovery, completed-node reuse, explicit null, and absent skipped/refused values. It neither invokes the CLI nor contacts a provider.

## Explicit dependencies

[`Engine`](crate::engine::Engine) takes a [`DecisionProvider`](crate::engine::execution::DecisionProvider), [`TokenCounter`](crate::limits::count::TokenCounter), and [`RetryClock`](crate::engine::execution::RetryClock). `NativeProvider`, `ByteCounter`, and `TokioClock` are supplied implementations; a fake provider needs no HTTP service. Providers must make one attempt and release owned work safely when their future is dropped. The engine can drop a provider future on timeout/cancellation. Providers must not hide retry, fallback, or detached background work.

A caller supplies configuration, overrides, a home-path fallback for store resolution, an explicit credential lookup closure, and a cancellation token. Selecting a configured credential source does not read it until the caller's lookup runs. Credential values are not serializable and never enter frozen settings. The following compiles without making a call in the documentation test:

```rust,no_run
use nouli_core::{
    config::{Config, resolve::RuntimeOverrides},
    engine::{Engine, EngineError, EvaluationInput, EvaluationResult,
             execution::{NativeProvider, TokioClock}},
    history::EvaluationRequest,
    limits::count::ByteCounter,
    storage::worker::LedgerWorker,
};
use std::path::Path;
use tokio_util::sync::CancellationToken;

async fn evaluate(
    ledger: &LedgerWorker,
    request: &EvaluationRequest,
    config: &Config,
    home: &Path,
    cancel: &CancellationToken,
) -> Result<EvaluationResult, EngineError> {
    let engine = Engine { provider: NativeProvider, counter: ByteCounter, clock: TokioClock };
    let options = RuntimeOverrides::default();
    engine.evaluate(ledger, EvaluationInput {
        request, config, overrides: &options, home, retry_interrupted: false,
    }, |_| None, cancel).await
}
```

Replace the credential closure with your own explicit secret provider when needed. Use `LedgerWorker::open(path, OpenMode::ReadWriteCreate)` for durable operations. Its serialized connection and owned blocking jobs keep SQLite work off the async executor. A submitted job retains its guards until completion even if an awaiting future is dropped. Await engine cleanup after cancellation before discarding the worker or deleting its directory. A synchronous `Ledger` is also available for synchronous inspection; callers choose the execution context for that blocking work.

`EvaluationRequest.session` selects `New`, `Continue`, or `From`. One source branches; several blend. A source without an explicit frontier captures its current finalized head once on acceptance; a source with a frontier selects that exact reachable ancestry. Later source writes do not enter the target. `required_records` names reachable decision/caller-record IDs that must survive reduction. Caller updates are attached to a turn and are append-only history events. `EvaluationRequest.standing` adds, replaces, or drops the session's standing questions, which later turns ask again; `turn.questions` then holds only one-off questions and may be empty. A model's answer is a judgment, while a caller commitment, completed action, pin, supersede, or unpin is an explicit caller declaration.

## Persistence and errors

A successful durable result means the complete turn, native outcome, actual context manifest, caller updates, finalized head, and request completion have been committed atomically. Reduced context never replaces the original ledger. Active context projects causal revisions, protects pins/current/required material, then compacts old optional decisions before evicting optional records oldest first. Capacity failure cannot silently remove protected material. Source blend conflicts require explicit caller resolution; results/probabilities/scores are never automatically averaged.

A request ID identifies normalized intent. Completed replay returns the original finalized result with no inference. The same ID with changed intent conflicts. An unfinished request reports live ownership or interruption; repeating the call does not silently retry an uncertain provider outcome. Explicit `retry_interrupted` uses frozen sources/settings and preserves uncertain prior attempts. This is durable result reuse, not exactly-once remote execution.

Match the typed `EngineError`, `WorkflowEngineError`, `StreamError`, and storage errors. `report()` produces safe structured categories/codes where supplied; process exit policy belongs to the executable. A failed event sink is an I/O/consumer failure even if useful durable work has already completed. Preserve the saved run reference from recovery or stream errors, then inspect it rather than inferring success from an incomplete stream.

For fresh ephemeral evaluation use `evaluate_ephemeral(None, ...)`. Sourced ephemeral evaluation requires an existing `ReadOnlyExisting` worker. It returns the same typed outcomes/context semantics without persisted result IDs, attempts, names, continuation, request replay, source migration, or ledger writes.

## Workflows, events, and resume

An `Arc<Engine<...>>` runs bounded ready nodes through `evaluate_workflow` or `evaluate_workflow_report`. Supply a `WorkflowRequest`, `WorkflowInput`, credential lookup `Arc`, cancellation token, and optional worker. Durable mode requires a writable worker. Each node consumes only declared completed ancestry and frozen shared caller inputs. Completion timing cannot import an unrelated sibling's result. Explicit per-node provider/model choices override run defaults.

`evaluate_workflow_report` and `resume_workflow_report` accept your `EventSink`. Sink futures are awaited in logical event order, so an embedding controls buffering, backpressure, and cancellation responsiveness. A sink must avoid blocking executor threads or waiting forever without its own cancellation policy. `CollectEvents` is a convenience for these small examples that stores all events in memory; use a streaming consumer for large runs. `validate_events` checks contiguous sequence, stable run identity, and exactly one terminal envelope. Diagnostic tracing events remain separate from result events.

Explicit resume takes a stored run ID, worker, fresh credential lookup, cancellation token, and optional event sink. It has no new graph/configuration parameter. Frozen graph/settings/source snapshots survive caller edits or missing current provider definitions. Completed/skipped nodes are reused; eligible unfinished nodes receive a new immutable retry round. An unexpectedly advanced target head is a conflict. Inspection exposes attempts, uncertain prior calls, round results, and live ownership without resuming anything.

The example asserts choice, probabilistic noul, fractional score 1.43, original scale, actual served identity, and per-question refusal. Confidence is native and attributed; it is not P(selected) or a calibration promise. Typed references preserve these distinctions, including a present JSON null versus absence:

```rust
use nouli_core::{data::Data, workflow::reference::{Absence, ReferenceValue}};
let explicit_null = ReferenceValue::Present { value: Data::new(serde_json::Value::Null)? };
let skipped = ReferenceValue::Absent { reason: Absence::Skipped };
assert_ne!(explicit_null, skipped);
# Ok::<(), nouli_core::data::DataError>(())
```

A refusal can be selected as a full typed outcome, while requesting its nonexistent answer follows the reference's explicit required/optional/default policy. Score expectations retain their ordered rubric; a different model or rubric is not a compatible number to merge implicitly.

Future REPL or natural-language orchestration can call these same contracts, engine methods, ledger worker, and event sink. The v1 seam supplies structured inputs and consumes typed results. It does not add tools, loops, approval nodes, image inputs, a generative planner, or another execution engine.
