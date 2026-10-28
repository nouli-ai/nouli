# Workflow scheduling

`schedule` runs an immutable compiled graph through an injected cancellation-safe `NodeExecutor`. It bounds owned tasks by the smaller of caller and graph parallelism. Declaration order breaks ready-node ties. Completion order is recorded separately and never chooses ancestry.

A node waits for all direct dependencies to settle. False conditions skip it. Active joins accept intentionally skipped branches and use the canonical union of completed parent paths. Failed dependencies block consumers unless explicit optional/default input or condition handling covers the failed source. Required missing/refused fields block their consumer; usable sibling question results remain available. Optional absence stays typed; a default is marked as used. A consumer with explicit missing-value handling can execute even when all parents are inactive.

Each executor call carries separately labeled base state, its node definition, resolved bindings, active direct parents, and only their completed ancestor records. Unrelated completed siblings cannot enter this history. An entirely inactive path with no explicit condition/input handling remains skipped.

Presence tests distinguish present null from absence. Refusal tests inspect the native question outcome. Comparisons preserve full-width integer ordering and fractional scores. All/any/not propagate unavailable operands unless explicit logic determines the result, such as false AND unknown or true OR unknown. There is no confidence threshold or numeric fallback.

Cancellation aborts and joins every owned task before returning, retaining results that had already completed. Pending and canceled work become interrupted. Invalid executor outcomes and executor panics become node failures; unrelated branches continue. Declared outputs retain their absence/default metadata. Without declared bindings, terminal-node question outcomes form the fallback projection.
