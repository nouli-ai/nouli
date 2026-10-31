---
status: accepted
date: 2026-10-27
---

# Build Nouli as a durable decision-session harness

Provider configuration is superseded by [ADR 0019](0019-independent-protocol-connections.md).

Nouli needs to carry typed decisions across turns and combine independently
developed histories into subsequent decisions. We will build a shared Rust
runtime with a CLI, durable session ancestry, provider adapters, and explicit
context management so callers can compose decisions without a generative
planner. A future REPL or natural-language agent will use the same runtime.

## Decision

- **Structured execution boundary.** V1 callers supply text or structured
  evidence, typed questions, answer spaces, and workflow definitions. The
  library owns evaluation, sessions, context assembly, persistence, and
  scheduling; the CLI owns configuration, input, and presentation. Ship the
  library and CLI first, with the REPL in the following release.
- **Typed results.** Support choice, noul, and score throughout the runtime.
  Noul remains a probability of yes. Score remains the expected ordered
  rubric-level index, including fractional values. Preserve uncertainty and
  provider/model provenance. A result is a model judgment; adoption and
  completed actions are separate caller commitments. Acceptance thresholds
  come from explicit caller rules.
- **Durable sessions.** Keep append-only logical history in a transactional
  SQLite ledger under the user's Nouli store. Persist by default, including
  standalone invocations, with an explicit ephemeral mode that may read saved
  sources without recording a new evaluation. V1 session discovery is global
  to the user; project-based grouping is deferred.
- **Branching and blending.** Create new sessions from fixed source points.
  One source produces a branch; several produce a blend. Preserve branch
  identities and include shared ancestors once. A new evaluation conditions
  on those histories rather than automatically averaging their outputs.
  Conflicting explicit replacements require caller resolution. Continuing a
  completed workflow combines its completed branch histories.
- **Bounded model context.** Preserve complete records in the ledger and
  assemble a separate context for each evaluation. Protect current input,
  pinned information, and explicitly required records. Under budget pressure,
  first shrink historical decisions oldest first to question, meaningful
  output, and uncertainty; then evict oldest optional records. Score retains
  scale meaning. Return an error if protected material alone cannot fit.
  Record what actually reached the model. Context updates accompany an
  evaluation in v1; standalone editing operations are deferred.
- **TOON context representation.** Use typed Rust values in memory and TOON
  for serialized internal context/history. Accept TOON and JSON at the CLI
  boundary and emit each provider's required native HTTP representation.
  Budget the complete evaluation against known model limits or explicit
  configured limits, distinguishing estimated counts from verified usage.
- **Explicit workflow execution.** V1 workflows are acyclic graphs of turns,
  with independent questions within a turn and dependent questions in later
  turns. Nodes inherit ancestor history. Use bounded parallelism and
  structured conditions over typed results. Joins follow active branches;
  intentionally skipped branches are distinct from failures. Preserve
  per-question refusals, continue independent work after failure, and resume
  frozen runs using completed results. Return declared outputs and status,
  with intermediate results available through inspection.
- **Provider and process boundaries.** Configure supported protocols through
  named profiles for TypeSafe/Jev, OpenAI Decisions, OpenRouter Decisions, and
  compatible custom servers such as local Laya. Credentials come from named
  environment variables. Allow explicit model overrides and bounded retries
  on the selected model. Reject conflicting session writers and support
  request IDs that replay stored completed submissions.
- **CLI behavior.** Evaluate directly when input is supplied, without a
  required `decide` verb. Support continuation and inline branch/blend
  sources, optional session names, files and stdin, adaptive text/JSON output,
  explicit output overrides, and optional JSONL workflow events.
  Superseded in part by [ADR 0002](0002-verb-cli-standing-questions-and-repl.md),
  which replaces root evaluation with verbs.

## Considered options

| Alternative | Reason for the chosen direction |
| --- | --- |
| Require a conversational LLM or planner to drive every decision | Structured callers can already supply questions and graphs. Keeping orchestration above the runtime permits a later natural-language layer without making it a v1 dependency. |
| Use stateless calls or mutable, automatically synchronized session histories | Durable fixed ancestry preserves the evidence behind each evaluation and allows independent chains to be combined deliberately. |
| Store sessions as append-only JSONL files | SQLite provides transactions and coordination for shared ancestry, concurrent runs, and resumable node state while inspection can expose readable records. |
| Serialize all context as JSON | TOON was chosen for compact structured model context, while JSON remains available where external clients and provider protocols need it. |
| Keep every historical detail until whole-record FIFO eviction | Shrinking to meaningful question/result records retains more decision history under a budget; explicit retention protects information the caller designates as required. |
| Include cyclic workflows, tools, and interactive orchestration immediately | Start with acyclic structured execution and separate run state, leaving extension points for bounded loops and later interfaces. |

## Consequences

- The ledger, session ancestry, and actual evaluation context are separate
  concepts. Storage and execution must preserve source identity, full typed
  results, context updates, and frozen run inputs even when a later prompt
  omits their detail.
- Compaction is lossy. Preserving a full ledger enables inspection, but does
  not prove that a bounded context contains every fact relevant to the next
  judgment. Callers control required retention; token savings and decision
  quality need validation on the selected models.
- Provider adapters must preserve score expectations, refusal outcomes, and
  uncertainty provenance. Similar field names do not establish identical
  confidence semantics across providers.
- Completed-request replay prevents duplicate recorded turns. An interrupted
  remote call whose result was never recorded may still need another inference
  attempt; exactly-once provider execution is not guaranteed.
- Accepted architecture does not imply an implemented release. Exact command
  spellings, schemas, numerical execution defaults, and library signatures in
  the walkthrough remain interface proposals. Tools, images, bounded loops,
  keychain integration, and project-aware discovery are deferred.

The [glossary](../GLOSSARY.md) defines the domain terms used here.
