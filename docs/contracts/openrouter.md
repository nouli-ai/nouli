# OpenRouter Decisions adapter

The `openrouter-decisions` profile sends a single request to the base URL plus
`/alpha/decisions`. The standard base `https://openrouter.ai/api` therefore yields
one `/api/alpha/decisions` path; custom proxy prefixes are preserved.

The state/questions map conversion shares the documented System One shape while
router protocol selection, capability records, endpoint, model names, and native
metadata remain separate. TOON state stays text. Choice/noul/score normalization
validates every named answer and complete distribution, retains fractional scores,
and rejects undocumented omissions or refusal types.

The [native reference](https://openrouter.ai/docs/api/api-reference/alphadecisions/submit-a-decisions-request),
verified 2026-10-28, documents model, named questions, state, optional provider
routing preferences, and response metadata. The adapter sets
`provider.allow_fallbacks=false`; it does not configure a fallback chain. Selected
router-namespaced model IDs remain unchanged across attempts.

Results retain the selected profile, served model, optional upstream provider,
optional native request ID, token usage, and optional provider-reported cost.
Missing metadata stays absent. Invalid metadata types, negative/non-finite costs,
and malformed answers fail explicitly. No cost is estimated or converted by the
adapter. Other protocols do not acquire invented router metadata.

The shared evaluator retry policy handles rate limits and transient statuses,
including the documented router timeout/upstream-error statuses 524 and 529.
The adapter performs no internal retry and no model switch. Synthetic fixtures
under `fixtures/providers/openrouter` and inline loopback tests exercise prefixes,
authorization, model names, all primitives, metadata, omissions, and errors without
live router calls.
