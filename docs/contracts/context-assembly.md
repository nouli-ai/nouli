# Context assembly

`ContextBuilder::build` accepts immutable history and fixed source points, the live turn, pending caller updates, required record/field selections, optional frozen workflow base input, resolved limits, a counter, and a reduction policy. It performs no provider or storage writes.

Canonical ancestry determines record order. Unrelated turns supplied in the history collection are excluded. Historical decisions retain their complete request and typed outcome, without embedding previous context manifests recursively. Context records retain creation provenance and their current revision status. Retired records are labeled inactive. Explicit requirements can still select historical evidence without adopting it as an active commitment.

The current state and workflow base are protected TOON evidence. Live question definitions remain a separate typed question set. All active pins and required records are protected. A required JSON Pointer preserves a present null, rejects an absent field, and reports a refused result distinctly when selection enters that refusal. Selecting a field protects its whole record in the initial policy.

`FullHistory` retains all selected records. `ContextPolicy` supplies explicit compact/omit actions for later policies. The builder rejects reductions of protected material and ineffective repeated compaction. It re-encodes and recounts every actual candidate. Protected-only overflow differs from optional-history overflow.

The returned plan includes live questions, required selections, ordered record representations, and a validated manifest containing the actual TOON payload, source points, provider/model, complete input count with attribution, budget, and output reserve. Counter estimates remain estimates. No source data is replaced by default text or numeric values.

Workflow caller bases appear once by immutable event identity. Current workflow base state stays protected across nodes, while older base state is optional historical evidence. Runtime-only historical nodes expose their node labels without invented persisted session or turn IDs.
