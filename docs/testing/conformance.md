# Provider conformance and context observations

Run the common offline corpus:

```sh
cargo test --package nouli-cli --test conformance --locked --offline
cargo test --package nouli-core --example compare_context --locked --offline
```

`nouli-cli/tests/conformance.rs` applies the same portable meal questions to native fixtures for TypeSafe/Laya, OpenAI Decisions, and OpenRouter Decisions. The normalized result must preserve option IDs and request order, P(selected)=0.7, a probabilistic noul=0.37, and score expectation=1.43 from probabilities [0, 0.57, 0.43]. Returning the argmax index 1, a Boolean noul, missing question coverage, or an unknown option fails. Native answer order is reversed where the protocol uses arrays. Missing optional confidence stays missing. OpenAI per-question refusal stays a refusal; undocumented refusal variants on the other protocols fail closed.

Confidence fields have different native meanings. OpenAI's choice confidence fixture is 0.8, TypeSafe/OpenRouter's is 0.35, and TypeSafe's separate answer confidence is 0.7. None replaces P(selected) or implies calibrated accuracy. Actual model identity, upstream/request metadata, optional cost, reported token usage, and estimated input counts remain separate.

Every adapter also receives captured TOON containing a compact selected choice with meaning and probability, plus the fractional score with its original labeled scale. Complete original state and distributions remain in exported ledger history. JSON and TOON forms of the same accepted request replay without an extra call. Strict decoding tests include exact i64/u64 boundaries, fractions, explicit null, string scalars, duplicate keys, nonfinite numbers, overflow, and malformed array lengths. Capacity errors yield a provider failure and no successful result. These are synthetic wire and semantic tests, not measurements of real models.

## Explicit live observations

This command is disabled by default. Run it only after authorizing calls to the selected service. Supply your own explicit configuration and profile from `docs/configuration.md`; hosted profiles resolve only their configured credential environment variable. Start Laya separately if selecting a local TypeSafe profile. This tool installs no model, creates no ledger, and performs one attempt for each format, with no retry or provider fallback.

```sh
cargo run --package nouli-core --example compare_context --locked -- --help
# Only after service authorization; OUTPUT.json must not exist:
cargo run --package nouli-core --example compare_context --locked -- \
  --allow-network /absolute/path/config.toml local \
  fixtures/journeys/food.json /absolute/path/OUTPUT.json
```

The flag, explicit configuration, profile, request, and new output path are all required before dispatch. Both contexts are preflighted and the output file reserved before either call. The example sends identical state facts serialized as compact JSON text and TOON; the complete native live-question envelope is unchanged. It records Nouli's version, requested model, returned actual model/provider identity, typed outcomes or safe errors, context byte sizes, byte-based token estimates with `exact=false`, and native usage with its own attribution when available. A missing actual usage field remains absent. An HTTP error still produces an observation report; it is not a successful semantic result.

The fixed JSON-first order and single sample are disclosed by the report's ordered observations. They do not control service drift, sampling variability, caching, or model revision availability. For an authorized investigation, retain raw reports with exact model versions and repeat with suitable external experimental controls. Compare outcomes against the supplied facts manually; identical fixture normalization does not establish real-model accuracy. Token savings and decision quality depend on model and data shape. No universal percentage saving or default-CI real-model assertion is made.
