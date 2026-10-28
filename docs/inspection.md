# Read-only session inspection and export

```sh
nouli sessions list
nouli sessions show Research --format text
nouli context Research --turn 019a0000-0000-7000-8000-000000000001 --json
nouli sessions export Research --format json > research.json
nouli sessions export Research --format toon > research.toon
```

Selectors are existing session IDs or unique case-sensitive names. The global list includes all sessions in the chosen user store. Show/export includes the selected session, its fixed creation sources, finalized heads, complete reachable turns, source-session provenance, relevant attempts, accepted request metadata, and active caller-record retention. Common ancestors appear once in canonical history order. Source provenance omits moving source heads, and later or unrelated source work is excluded. Full original state, choices, rubrics, probability distributions, refusals, usage, and record identities survive model-context compaction.

Turn outcomes are model judgments. Caller evidence and commitments are separate context records with their own kinds, revision origins, and pin state. Structural conflicts are reported explicitly rather than presented as resolved active context. Inspection does not apply updates, synchronize sources, or rewrite records.

`context SELECTOR` defaults to the session's sole finalized head. Use `--turn ID` when there are several heads or to inspect an older turn owned by that session. To inspect an inherited source turn, select its original session. The report contains the exact saved manifest, rendered TOON, included/compacted/omitted projections, protection flags, budget, counter method/exactness, requested identity, actual outcome identity/usage, and relevant attempt history. Accepted request and node-intent metadata retain explicitly required IDs and frozen selections. A protected record may be pinned, explicitly required, or current input; protection alone does not mean every historical record reached the model.

Exports include owned workflow definitions/progress, prior retry-round results/events, and accepted node intents. Source workflows contribute frozen definitions and the relevant captured turns/attempts, without importing uncaptured sibling results. Credential source names and nonsecret settings remain metadata; credential values are never stored by these contracts. Export is a serialization for inspection, not an import or federation facility.

Text displays the full structured history with a descriptive heading. JSON and TOON contain version-1 data without decoration. Output flags work before or after the subcommand. JSONL is reserved for invocation events and is rejected for these read-only commands. Inspection reads store/output configuration only, so removed provider profiles and unavailable credentials do not prevent viewing saved history. Invalid configuration syntax or store/output settings still fail explicitly.

All commands open an existing ledger read-only. Missing stores are reported without creating directories; older schemas require a separate writable migration, and newer schemas fail with the found/supported versions. Show/export uses one coherent SQLite read snapshot and performs no migration or recovery writes. Library callers can use `Ledger::export_session`, `inspect_turn`, and `inspect_session_turn` with the same contracts.
