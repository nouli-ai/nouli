# Documentation verification

Python 3.11 or later provides the standard-library TOML parser for `python3 scripts/validate_docs.py`. It checks tracked Markdown links and balanced fences, complete JSON/TOML fenced examples, tutorial JSON syntax/duplicates/nonfinite values, and checked-in configuration syntax. It does not fetch external pages, execute shell snippets, run hosted calls, or audit unrelated untracked design files. Rustdoc separately validates public API links and compiles the Rust snippets with warnings denied through the Cargo checks.

`python3 scripts/walkthrough.py --binary target/debug/nouli` validates tutorial JSON/TOON and the configured loopback profile at the actual CLI boundary, checks help for every documented command family, and executes the documented durable/ephemeral/inspection/workflow journey. It requires a preceding `cargo build --workspace --locked`; the same script accepts a staged release binary. All resources are isolated and owned. Native provider formats and additional failure paths are exercised by Cargo tests rather than by unverified shell text.

| Documentation command or payload family | Executable evidence |
| --- | --- |
| README/Cargo development commands | the Cargo checks, successful local macOS/Linux parity, CLI help smoke |
| Configuration fields, credential sources, overrides, known/explicit budgets | `nouli-core/src/config.rs` and `config/resolve.rs` inline tests; `limits.rs`/`limits/registry.rs` tests; `nouli-cli/tests/evaluation.rs` invalid-selection/budget/credential cases |
| Direct flags, complete JSON/TOON files, stdin, names, fixed source syntax | `scripts/walkthrough.py`, `nouli-cli/tests/evaluation.rs`, `nouli-cli/tests/journeys.rs` |
| Attached commitment/pin/supersede/unpin/required record payloads | `fixtures/tutorial/`, walkthrough, context projection/builder tests, journey capture assertions |
| Inspection list/show/context/export, text/JSON/TOON, readonly schemas | Walkthrough and `nouli-cli/tests/evaluation.rs` inspection/export/old-new-schema cases |
| Output configuration/auto/explicit flags and JSONL | `nouli-cli/tests/journeys.rs` output-selection test; CLI renderer unit tests; workflow event validation |
| Workflow run/show/resume, graph/stage limits, source/provider overrides | Walkthrough; `nouli-cli/tests/evaluation.rs` complete stdin/TOON, partial resume, advanced head, and cancellation cases |
| Provider conformance, native score/noul/confidence/refusal, compact/full history | `nouli-cli/tests/conformance.rs`, `journeys.rs`; codec/context/provider inline tests |
| Fault/recovery commands | `nouli-cli/tests/faults.rs` and core storage/engine inline tests |
| Rust embedding/API commands and snippets | `cargo test --workspace --all-targets --locked`, `cargo test --workspace --doc --locked`, warnings-denied `cargo doc` |
| Optional context comparison help/argument gate | Compiled `compare_context` example; credential-free local loopback command check; actual service calls remain opt-in and unrun |

Command examples in the focused guides use illustrative filenames, session selectors, and returned IDs to explain grammar. The walkthrough supplies real files/IDs and exercises those operations. The context-comparison command containing `--allow-network` is explicitly excluded from default verification and requires separate service authorization. A passing offline suite demonstrates contract behavior; it does not establish hosted-model quality, calibration, or token savings.
