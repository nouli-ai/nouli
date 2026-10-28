# Credential-free CLI journeys

Run from the workspace root with the pinned toolchain:

```sh
cargo test --package nouli-cli --test journeys --locked --offline
cargo test --package nouli-cli --tests --locked --offline
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
cargo test --workspace --locked --offline
```

Every journey invokes the real `nouli` binary with an isolated home, configuration, and SQLite store. Owned loopback servers implement the native TypeSafe/Laya, OpenAI Decisions, and OpenRouter Decisions wire formats. No hosted endpoint, credential, model download, or real-model answer is involved. The response/usage fields are synthetic fixture observations, not hosted accuracy, calibration, billing, or token-saving measurements.

| Journey | Assertions |
| --- | --- |
| Meal and budget chain | Choice meaning/P(selected), noul probability, 1.43 and 0.6 score expectations, native confidence provenance, caller commitment, completed replay with zero extra inference |
| Two-stage reduction | Actual captured TOON first retains compact selected choice/score/confidence while dropping unselected alternatives; tighter context later omits optional evidence and old decisions while retaining a compact score with its original scale |
| Branch and blend | Inline one-source branch/unpin leaves the source pinned; multiple sources deduplicate shared ancestors and preserve mixed models/rubrics without averaging; later source supersession does not enter the captured blend |
| Protected overflow | Each of the three protocols returns capacity failure before an extra provider call or attempt; finalized heads remain unchanged |
| Mixed workflow | Typed score/noul conditions, a skipped branch, concurrent independent work, a native per-question refusal, two transient failures, partial output, explicit resume reusing recorded successes, declared outputs, and continuation through the completed frontier |
| Ephemeral operation | Sourced decisions and workflows leave database bytes unchanged; returned persisted identities are absent |
| Serialization/output | Checked-in JSON/TOON payloads decode equivalently; auto/config/explicit text/JSON/JSONL behavior preserves valid protocol stdout and stderr separation |

The tests assert captured wire context, record/source identities, native numerical values, and provider call counts. The meal/blend chain dispatches 3 TypeSafe, 2 OpenAI, and 2 OpenRouter calls, including its deliberate source update and ephemeral evaluation. The mixed workflow dispatches 5, 4, and 4 respectively across exhausted attempts, resume, continuation, and a separate ephemeral run. A valid completed request replay adds no inference.

`fixtures/journeys/` contains the portable food/budget requests and native responses. The budget score is 0.6 even though its most likely rubric index is 1; the food score is 1.43 even though its most likely index is also 1. Their different scales remain labeled. These values are deterministic contract fixtures, not predictions from an installed model.

The adjacent evaluation/smoke suites cover fixed source-head syntax, complete requests and context updates, schema/readonly behavior, frozen model/source recovery, advanced heads, Ctrl-C, output backpressure, and missing stores. Unsupported or billable live-provider observations are outside the required matrix.
