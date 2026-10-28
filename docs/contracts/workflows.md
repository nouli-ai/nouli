# Workflow and protocol contract v1

`WorkflowSpec` declares an ordered list of stable node IDs. A node is one turn
of independent questions. Dependent judgments belong in later nodes. Node inputs
bind references into structured state; state, context updates, explicit required
records, conditions, and provider/model overrides are optional. Source order and
node declaration order define ancestry tie ordering. The default concurrency is
four; zero is invalid. Compilation validates DAG edges, reference types,
and ancestry before execution. Shape validation alone cannot establish a DAG.

V1 forbids cycles, scripts, arbitrary tool execution, and expression strings.
Conditions are typed comparisons, all/any/not groups, existence checks, and
explicit per-question refusal checks. A reference selects a node/question,
answer/selected/probability/score/confidence field, and optional RFC 6901 pointer.
Missing policy is required, optional, or an explicit default. Present null remains
`present` and differs from absent skipped/refused/failed/blocked/missing fields.
Missing comparison operands produce an error unless a default supplies a value;
they never silently compare as false. Optional absence is suitable for output or
input omission and existence checks. Refusals never enter the transport retry
policy. Answered sibling questions remain usable.

| Node state | Allowed next states | Meaning |
| --- | --- | --- |
| pending | running, skipped, blocked, interrupted | Not evaluated |
| running | completed, failed, interrupted | An attempt is active |
| completed | none | Finalized turn, including valid per-question refusals |
| skipped | none | Intentionally excluded by conditions or inactive branches |
| failed | pending on explicit resume | Evaluation failed in this retry round |
| blocked | pending on explicit resume | Required failed/unavailable input prevents progress |
| interrupted | pending | Incomplete work eligible for frozen resume |

Run states are pending, running, completed, partial, failed, or interrupted.
Running may finish in completed/partial/failed/interrupted. Explicit resume of an
interrupted/partial/failed run starts another bounded retry round, preserving
prior attempts and completed/skipped nodes. Completed runs remain finalized.
A join waits for active parents, passes intentionally skipped branches, and
blocks when a required input failed. Independent branches continue after failure.
False conditions intentionally skip a node; unavailable required comparisons
block rather than silently skipping.

Durable runs capture `FrozenWorkflow` before work starts: definition, input state,
source frontiers, updates, required records, and selected model defaults. Resume
uses those frozen values and reuses completed nodes; it does not read a changed
workflow or moving source. Ephemeral runs retain only in-memory progress and
cannot resume after process exit. Continuing a durable run uses the completed
branch frontier with shared ancestry deduplicated.

Reports project declared outputs in their declaration order. When outputs are
absent, terminal-node answers are the fallback. Node reports retain intermediate
turn IDs, statuses, inspection references, and structured errors. Full intermediate
outcomes remain available through ledger inspection. A `ResultEnvelope` carries
version 1 and a turn/run/error report. Error categories distinguish invalid input,
configuration, authentication, capability, contract, capacity, transport, storage,
conflict, interruption, and internal faults. Messages must contain safe diagnostic
text rather than model context, response bodies, or credentials.

`EngineEvent` is a versioned logical event with consecutive zero-based sequence
numbers and a fixed optional run ID. Exactly one terminal event closes each
stream, carrying its result/error envelope. Diagnostics use tracing separately.
The terminal run report must carry the stream's run ID; standalone turn reports
have no run ID. An error may close either kind of stream.
`protocol::validate_events` checks complete serialized streams. Consumers must
not treat an unfinished stream as a finalized report.

`fixtures/contracts/workflow.json` is a five-node score-threshold branch/join
with an optional skipped output. `events.json` includes an answered/refused sibling,
partial failure, and blocked descendants. Inline tests round-trip a frozen run,
exercise state transitions, pointers and absence, and reject broken event streams.

The public compact report contains the declared outputs, or per-question whole outcomes from terminal nodes when outputs are absent. Selected scalars retain the source question and answer space, so a fractional score carries its original ordered rubric. Present null, optional absence, required-output absence, defaults, and whole native refusals remain distinct. Node summaries carry status and record references; intermediate answer bodies are retrieved separately through saved turns and run inspection. Ephemeral reports expose no persisted run, session, or turn identity.

`Engine::evaluate_workflow_report` and `resume_workflow_report` wrap the shared engine with a caller-supplied asynchronous `EventSink`. Each invocation delivers v1 consecutive events beginning with `started` and ending with one `terminal` result/error. Live progress contains node IDs, statuses, and finalized record references, without model state bodies. Completed replay emits started/terminal without provider work. A failed sink produces a typed delivery error with the saved run ID when available; terminal delivery is attempted once, and an I/O failure cannot be reported as a successful stream. Dropping an invocation or losing the process can prevent delivery; durable inspection/resume remains the recovery path.

`Ledger::workflow_events` returns immutable globally sequenced run events, including retry-round numbers and attempt/turn references. Historical payloads without an explicit version decode as v1. A recovered orphan round receives an interrupted terminal marker before the next round starts. Reporting and event delivery do not generate explanations, additional inference, or synthesized averages. Structured events and terminal results are separate from tracing diagnostics.
