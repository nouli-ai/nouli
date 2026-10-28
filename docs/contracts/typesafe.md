# TypeSafe and local Laya adapter

The `typesafe` protocol posts one request to `/v1/systemone`, preserving the
configured base prefix. `provider::typesafe::evaluate` accepts explicit resolved
settings, assembled TOON context, a token counter, and cancellation. It checks
capabilities and budgets before sending; it never retries or changes models.

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
profile with its actual model and an explicit budget. Nouli does not install,
start, or tune Laya. Its defaults and confidence semantics are independent of
hosted Jev; recorded profile/model provenance preserves that distinction.

`fixtures/providers/typesafe` contains synthetic native request, response, and
capacity-error examples. Inline loopback tests verify escaping, URL prefixes,
no-key operation, ordered rubric levels, typed results, malformed coverage, and
capacity classification. Required tests do not call hosted models.
