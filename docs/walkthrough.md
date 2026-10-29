# Executable CLI walkthrough

These commands use the checked-in tutorial requests and your configured provider. Run them from the workspace root with a fresh store, because the requests contain fixed record IDs. `RUN_ID` and `TURN_ID` below are values returned by preceding commands.

The Rust integration tests exercise these behaviors using local fixture providers. To run them without API credentials:

```sh
cargo test --package nouli-cli --locked
```

```sh
nouli --input fixtures/tutorial/meal.json --name Dinner --request-id tutorial-meal
nouli --input fixtures/tutorial/meal.toon --name Dinner --request-id tutorial-meal
nouli --input fixtures/tutorial/commitment.json --session Dinner
nouli --input fixtures/tutorial/meal.json --from Dinner --name Branch
nouli --input fixtures/tutorial/meal.json --from Dinner --from Branch --name Blend
nouli --input fixtures/tutorial/supersede.json --session Dinner
nouli --input fixtures/tutorial/unpin.json --session Branch
nouli --input - --input-format json --session Blend < fixtures/tutorial/meal.json
nouli context Blend --turn "$TURN_ID"
nouli sessions export Blend --format json
nouli sessions export Blend --format toon
nouli sessions list
nouli sessions show Branch --format text
nouli --input fixtures/tutorial/meal.json --from Blend --no-session
nouli workflow run fixtures/tutorial/workflow.json --name Travel
nouli workflow show "$RUN_ID"
nouli workflow resume "$RUN_ID" --jsonl
```

The simulated provider responses used in tests select lentils with probability 0.7. Its score is 1.43 on Poor/Adequate/Excellent, while the most probable level is index 1. The budget request's score is 0.6 on Over budget/Affordable/Comfortable. These deterministic fixtures demonstrate different scale meanings, not model predictions. Noul stays a numeric probability of yes. Native confidence is separate from P(selected); see [conformance](testing/conformance.md) for differing native fields and per-question refusal behavior.

The equivalent TOON request with the same ID replays the completed meal without another provider call. `commitment.json` pins a caller-declared budget of 8. A commitment or completed action records what the caller declares, not a model's judgment or an executed tool. `supersede.json` appends a replacement budget of 12 and retains earlier evidence. The existing blend still sees its captured budget of 8. `unpin.json` changes retention on the branch and explicitly requires that original record for its evaluation. No standalone update command is implied. Static record IDs in the tutorial require a fresh store; use a separate `--store` directory when running these requests manually.

The blend deduplicates shared ancestry, retains separate branch identities, and never averages scores/probabilities from different rubrics. Its complete export includes five captured turns at the tested point; later source/branch updates do not enter. Sourced ephemeral evaluation reads saved history without modifying database bytes or returning persisted session/turn IDs. A fresh ephemeral call needs no store.

The five-node travel workflow is initial, high/low conditional branches, join, final. A score comparison against 1.4 activates high and skips low; join waits for active parents. The fixture deliberately returns one HTTP 503 on high with one configured attempt, producing exit 4 and a partial report. Show inspects that saved run. Resume reuses the completed initial node, makes exactly three new calls (high/join/final), and emits consecutive version-1 JSONL events with one final terminal envelope. The test scenario makes 14 provider calls. Live services can choose different answers, routing, refusals, and failures; those outcomes are illustrative unless controlled by these fixtures.

[Context reduction](contracts/context-reduction.md) first shrinks old optional decisions, oldest first, to self-describing question/output/uncertainty. Choice retains selected meaning/P(selected); noul retains P(yes); score retains expected value and labeled indices; refusals remain typed. Historical unpinned state and unselected alternatives/distributions leave compact context, then optional records are evicted oldest first only if necessary. Pins/current/required material is protected. Overflow fails before inference rather than silently discarding it. Complete originals remain in the ledger. [Journeys](testing/journeys.md) capture actual dispatched compact/evicted context across providers.

Use [context inspection/export](inspection.md) to see the exact sent TOON, fixed source points, included/compacted/omitted records, budget and count attribution, requested settings, served identity, attempts, and full original history. [Request replay](contracts/request-replay.md) and [workflow recovery](cli-workflows.md) distinguish completed replay, active ownership, explicit interrupted retry, unknown provider outcomes, and advanced-head conflicts. A killed process or unsaved response cannot be reported as a known answer; explicit retry may repeat remote inference.

[Output](output.md) covers terminal auto selection, explicit JSON/TOON, configured defaults, JSONL, and exit codes. [Configuration](configuration.md) covers credential source names and known/explicit budgets. All numerical defaults are listed in [defaults](defaults.md). REPL, tools, images, bounded loops, expression language, standalone context management, keychain integration, and directory-aware discovery remain future work.
