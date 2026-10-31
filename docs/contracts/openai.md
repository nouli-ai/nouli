# Direct OpenAI Decisions adapter

The `openai-decisions` protocol uses `POST /v1/decisions` and the native input and
questions array. It does not call Responses, chat, tools, or a fallback model.
The shared codec follows the official
[Decisions reference](https://developers.openai.com/api/reference/resources/decisions/methods/create)
and [guide](https://developers.openai.com/api/docs/guides/decisions), verified
2026-10-28. Endpoint, model ID, headers, media preparation and optional limits
come from configuration. The OpenAI CLI shortcut only writes ordinary settings;
there is no runtime provider/model catalog.

Assembled TOON context is input text. Every question receives its unique portable
name. Noul maps to predicate; choice IDs/descriptions map to typed string values;
score criteria map to ordered label/description entries. Complete question criteria
remain in the instructions. Named responses are validated and projected back into
caller order, never assigned by an unchecked positional fallback.

Image evidence becomes ordered `input_text` / `input_image` parts inside a user
message's `content`; question instructions can also contain those parts. Native
choice descriptions remain text and refer to labelled images in shared input.
This uses the [Decisions reference](https://developers.openai.com/api/reference/resources/decisions/methods/create),
checked 2026-10-30. Binary payloads remain in saved evidence and are removed from
the TOON text sent to the model. Text-only requests preserve their string input.

Predicate probability becomes noul probability. Choice distributions retain every
original ID. Score probabilities are checked by level index and label before
preserving their fractional expectation. Unknown/duplicate/missing names, boolean
values where string IDs were requested, inconsistent scale labels, coverage, and
probabilities fail with typed errors.

A native refusal remains a per-question refusal alongside usable sibling answers.
The API supplies no refusal reason, so the portable reason identifies that fact
without inventing a provider explanation. A 200 status does not imply all answers
are present. Native model, usage, and optional confidence remain attributed to
the selected profile and served model. Capacity, authentication, and transient
errors use the shared transport taxonomy; the evaluator alone decides retries.

`fixtures/providers/openai` contains synthetic request, response, and error
examples. Required loopback tests cover all primitives, mixed refusals, native
question/level mapping, identity failures, escaping, endpoint selection, and error
categories. They do not make live OpenAI calls or need a real API key.
