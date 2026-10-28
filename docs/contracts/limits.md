# Capabilities and token budgets

`fixtures/capabilities/models.json` is a primary-source snapshot verified on
2026-10-28. Entries match protocol, official host, and an explicitly recognized
model ID. Custom hosts and unknown models require an explicit context budget or
window. No network discovery or model calls run when loading the registry.

| Protocol/model | Total cap | State plus one question | Questions |
| --- | ---: | ---: | ---: |
| TypeSafe Jev 1.13/1.14 and current aliases | 64,000 | 32,000 | Token-bound |
| Direct OpenAI gpt-6-luna | 1,050,000 | No separate verified cap | 400 |
| OpenRouter openai/gpt-6-luna-decisions | 1,050,000 | No separate verified cap | 200 |
| OpenRouter typesafe/jev-1.13 | 262,144 | No separate verified cap | Token-bound |

TypeSafe publishes standard limits as 64k/32k; the registry uses conservative
decimal caps rather than assuming binary units. Long-context opt-in is not
enabled. See [TypeSafe models](https://docs.typesafe.ai/models). Aliases may move,
so recorded served model IDs and capacity errors remain significant.

Direct OpenAI's total cap is the published
[model window](https://developers.openai.com/api/docs/models/gpt-6-luna), not a
claim of exact Decisions token accounting. Its question/answer limits come from
the [Decisions guide](https://developers.openai.com/api/docs/guides/decisions) and
[reference](https://developers.openai.com/api/reference/resources/decisions/methods/create).
Router records independently follow the
[Luna Decisions page](https://openrouter.ai/openai/gpt-6-luna-decisions) and
[Jev page](https://openrouter.ai/typesafe/jev-1.13). Missing published per-question
caps remain unknown; they are not copied from another protocol. The router's
published 200-question cap remains separate from direct OpenAI's 400.

Nouli v1 supports text/structured state, 2-255 choice IDs, and 2-10 ordered score
levels. Images/audio/video are rejected even when a vendor model supports them.
Refusal support is verified for direct OpenAI; undocumented protocols retain
unknown support rather than fabricating refusal answers. Confidence and complete
distributions are capability metadata, not automatic acceptance gates.

Configured budgets can reduce verified limits; larger overrides fail explicitly.
An unknown/custom profile must supply `context_budget_tokens` or
`context_window_tokens`. Optional output reservation counts against total and
per-question budgets. Transport `Capacity` errors permit evaluator replanning;
estimates cannot guarantee that a server will accept the request.

`TokenCounter` receives protocol, model, assembled TOON evidence, and all complete
live questions. A verified implementation must count the full native request,
not just apply a generic text tokenizer. `ByteCounter` deterministically estimates
one token per four UTF-8 bytes, rounded upward, plus explicit envelope/question
overhead estimates. It counts prompts, IDs, criteria, descriptions, option IDs,
and ordered rubric levels. It never labels these estimates exact.

Every count records byte estimate, tokenizer estimate, verified protocol count,
or provider-reported provenance and exactness. Preflight assessment accepts the
first three and validates total/per-question counts separately. Provider-reported
usage remains in the result's `Usage`; it never silently replaces the preflight
estimate. The finalized history manifest retains the input count method. No live
count endpoint or generic tokenizer with exact Decisions semantics is assumed.
