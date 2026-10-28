# Workflow compilation

`workflow::compile::compile` validates a version-1 graph without provider calls or storage. Its immutable compiled plan retains the original definition, separate shared base and node states, stable declaration-index node identities, deterministic topological order, dependency/consumer indexes, ancestor membership, typed input/condition selectors, and declared outputs. With no declared outputs, leaf nodes form the fallback output plan.

Compilation rejects duplicate nodes/questions/bindings, missing or cyclic dependencies, undeclared non-ancestor references, invalid pointer/field combinations, mismatched comparisons, incompatible defaults, and ambiguous required references to conditional nodes. No hidden dependency is inferred from a reference. Optional/default policies explicitly admit conditional absence; an explicit presence test can inspect an absent conditional result.

Choice selection is a string, probability and score are numeric, and confidence remains a separately selected numeric field when available. Typed answer paths expose only their declared shape and bounded distribution indices. `ResultField::Outcome` addresses the full question result, including a native refusal, or supported metadata fields. Nullable native metadata remains distinguishable from absence at runtime.

Conditions remain typed data with bounded structural nesting. No script/expression variant or cycle is accepted. Compiled definitions carry no execution progress, leaving scheduling and recovery state in separate modules.
