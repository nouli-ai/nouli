# Defaults and fixed v1 bounds

These values describe implementation defaults, distinct from provider capabilities and explicit tutorial/test settings.

| Setting or bound | Default/value | Behavior |
| --- | --- | --- |
| Request/result/event contract | Version 1 | CLI request normalization fills omitted turn/workflow versions; other versions fail |
| Durable selection | New unnamed session | Generated UUIDv7; continuation/names/sources require explicit selection |
| Default provider/model | Neither selected | Set top-level `default_provider` and `default_model`, or supply selection through CLI/request/session/workflow settings |
| Config/store | `~/.nouli/config.toml`, `~/.nouli` | First evaluation creates a missing default config; explicit overrides, no project discovery |
| Output | `auto` | Text for terminal, JSON when redirected; explicit format overrides config |
| Execution parallelism | 4 | Workflow graph limit also defaults to 4; effective ready-node count respects both |
| Workflow mode | `durable` | `ephemeral` and `workflow run --no-session` explicitly select no recording |
| Attempts per turn/node retry round | 3 total | Includes first attempt; configured/profile overrides supported |
| Attempt timeout | 120 seconds | Covers selected-provider attempt; no fallback provider/model |
| Backoff | 1 second, doubling | Default three-attempt delays are 1 then 2 seconds, with no jitter |
| Retry-delay cap | 30 seconds | Recognized Retry-After can increase delay up to cap |
| Response and CLI file/stdin bounds | 16 MiB each | Checked before accepting oversized content; response streaming remains bounded |
| Output token reservation | 0 when omitted | Explicit model reservation reduces total/per-question input capacity |
| Context budget | 8192 | CLI > exact model > provider defaults > global defaults; clamped by an explicit window |
| Capacity replan | floor(3/4 of prior estimated input) plus reserve | Uses remaining attempt allowance only if rebuilt context actually becomes smaller; protected material remains intact |
| Byte counter | ceil(UTF-8 bytes / 4) | Estimate only; adds estimated envelope 32, plus 16 per question and serialized complete-question estimate |
| Choice/rubric bounds | 2-255 options, 2-10 ordered levels | Provider-specific count/context caps can be smaller |
| Probability/expectation rounding tolerance | 0.005 per reported value (two-decimal rounding) | Distributions must be able to sum to 1, and scores must match an expectation, within those bounds; values are never renormalized or replaced |
| Condition nesting | At most 64 recursive levels | Structured conditions only; no expression language |
| Default reference field/missing policy | `answer` / `required` | Optional/default must be explicit; present null differs from absence |
| Undeclared workflow outputs | Terminal-node answers | Declared outputs otherwise determine the compact final report |
| SQLite busy wait | 2 seconds | Cross-process ownership rejects competing same-session writers |
| SQLite schema/journal/sync | 5 / DELETE / FULL | Atomic migrations on writable opens; inspection/read-only sources never migrate |
| Unix private directory/file modes | 0700 / 0600 | For newly owned ledger paths; locks retain stable inodes |
| HTTP redirects/proxy/retry | Disabled in transport | Engine alone owns bounded retry policy; no hidden model/provider fallback |
| TOON implementation/spec | Locked `toon-format` 0.7.0 / TOON 4.4 | Strict validation around the official codec; JSON and TOON share finite exact-integer semantics |

Transient retry classes are connection failure, timeout, HTTP 429, and HTTP 500/502/503/504/524/529. Authentication, unsupported capability, malformed contracts, and cancellation fail without transient retry. Capacity has the separate smaller-context path above. Workflow resume starts an explicit new bounded round, preserving earlier attempts and unknown outcomes.

Optional configured limits and estimated counting are documented in [limits and budgets](contracts/limits.md). Nouli has no provider/model capability catalog. The [output guide](output.md) documents numerical exit statuses. [Storage](contracts/storage.md), [reduction](contracts/context-reduction.md), and [transport](contracts/transport.md) describe the corresponding invariants.
