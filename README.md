# Nouli

Nouli is a Rust library and command-line harness for decision models. Callers provide text or structured evidence, typed questions, and optional acyclic workflows. Choice, noul (probability of yes), and score (expected ordered rubric index) stay typed through provider adapters, durable history, context reduction, and workflow routing.

`nouli-core` owns contracts, provider dispatch, context, SQLite persistence, and execution. `nouli-cli` owns argument parsing, terminal presentation, signals, and process exits. Supported wire protocols are TypeSafe/Jev and separately served local Laya, direct OpenAI Decisions, and OpenRouter Decisions. Custom URLs must implement one of those protocols. Native confidence is attributed and separate from answer probability; refusals remain per question.

Development uses Rust 1.99.1 from `rust-toolchain.toml`, Cargo, rustfmt, Clippy, Bash, a C toolchain for bundled SQLite, and the platform SDK/system TLS prerequisites. Documentation validation uses Python 3.11 or later. Run all required checks without provider credentials or installed models:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo build --workspace --locked
python3 scripts/walkthrough.py --binary target/debug/nouli
python3 scripts/validate_docs.py
```

The walkthrough runs the real CLI against a temporary loopback fixture and isolated home/store. Its answers are deterministic fixtures, with no claim about real-model quality. Local checks cover aarch64 macOS and Linux. Hosted CI has not been run from this local implementation branch. [Provider conformance](docs/testing/conformance.md), [CLI journeys](docs/testing/journeys.md), and [fault scenarios](docs/testing/faults.md) describe the offline gates.

For a provider you have separately configured and authorized, evaluation is the default action:

```sh
cargo run --package nouli-cli --locked -- --help
# Requires your configured provider; the offline walkthrough supplies its own:
nouli --input fixtures/tutorial/meal.json --name Dinner
nouli --input fixtures/tutorial/commitment.json --session Dinner
```

Sessions persist by default in `~/.nouli`; names are optional. One `--from` branches, several blend fixed source points into a new session, and `--session` continues an existing session. `--no-session` performs fresh or sourced read-only evaluation. Complete immutable ledger history survives automatic compaction and eviction from the model's limited context. Attached caller commitments and explicit revisions remain separate from model judgments.

- [Walkthrough and checked-in payloads](docs/walkthrough.md)
- [Configuration and credentials](docs/configuration.md), [numerical defaults](docs/defaults.md)
- [Evaluation and source selection](docs/cli-evaluation.md)
- [Workflows, events, and recovery](docs/cli-workflows.md)
- [Output formats and exit codes](docs/output.md)
- [Actual context and complete history inspection](docs/inspection.md)
- [Local release packaging and installation](docs/release.md)
- [Rust embedding API and executable fake-provider example](docs/embedding.md)

V1 does not include a REPL, model tool execution, images/audio/video, loops, an expression language, standalone context mutation, OS keychains, or project/directory session discovery. Those are future scope; no model is installed or bundled. Release publication, signing/notarization, license selection, and registry delivery require separate owner decisions and authority.
