# TypeSafe and local Laya adapter

The `system-one` codec posts one request to `/v1/systemone` by default,
preserving the configured base prefix. `provider::system_one::evaluate` accepts
resolved settings, assembled TOON, a counter and cancellation. It performs one
attempt, with no model/provider switching. TypeSafe, local compatible servers,
and routed deployments use explicit connection settings with this shared codec.

The native envelope has model, state, and a named questions map. State contains
the assembled TOON as an escaped JSON string. Complete live choice descriptions
become a criteria map; ordered score levels become a criteria array. Portable
question criteria remain in structured instructions alongside the prompt. Noul
keeps its probability-of-yes meaning and receives no invented yes/no criteria.
Native shape follows the
[System One reference](https://docs.typesafe.ai/api/system-one),
[choice](https://docs.typesafe.ai/primitives/choice),
[score](https://docs.typesafe.ai/primitives/score), and
[noul](https://docs.typesafe.ai/primitives/noul) documentation, checked 2026-10-28.

Image encoding and preparation are explicit connection settings. The TypeSafe
setup recipe writes `assets`, its image opt-in header, inclusive dimension bounds
of 280 to 1568 and approximate patch accounting. Those defaults live only in the
CLI recipe, never in the codec. Hand-written settings can express the same thing.
Other servers choose `images-base64`, `images-data-urls` or `state-image-urls` as
required. Original image evidence remains unchanged; see [limits](limits.md).

The adapter validates named coverage and types, complete option/level probabilities,
score expectations, and any returned rubric legend against the original scale.
Malformed, missing, extra, or foreign answers fail with typed errors. A supplied
score of 1.43 remains fractional. Model identity and available token usage are
retained; missing usage remains unknown. Undocumented refusal shapes are rejected.

Confidence retains its native value and a meaning naming the selected profile
and served model. Optional `answer_confidence` remains a separate native field,
distinct from distribution confidence. The
`value` stays absent if the native distribution-confidence field is absent;
the maximum-answer probability never fills it. The
[TypeSafe confidence documentation](https://docs.typesafe.ai/confidence) describes
its distribution-derived statistic. Laya's separate maximum-answer probability
must not be substituted for that statistic or assumed calibrated like Jev.

[Laya's HTTP server](https://github.com/NandhaKishorM/laya#self-hosting-http-server-jev-compatible)
documents the same systemone wire protocol. Configure a local unauthenticated
profile with its actual model and optional explicit limits. Nouli does not install,
start, or tune Laya. Its defaults and confidence semantics are independent of
hosted Jev; recorded profile/model provenance preserves that distinction.

`fixtures/providers/typesafe` contains synthetic native request, response, and
capacity-error examples. Inline loopback tests verify escaping, URL prefixes,
no-key operation, ordered rubric levels, typed results, malformed coverage, and
capacity classification. Required tests do not call hosted models.
