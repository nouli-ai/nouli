# Decision contract v1

`nouli_core::decision::TurnRequest` contains `version: 1`, text/structured state,
and independent named questions. Unknown fields and media tags are rejected.
State supports null, booleans, strings, arrays, objects, finite fractional
numbers, and exact signed/unsigned 64-bit integers. Integers retain their native
representation rather than passing through f64. Images and non-finite numbers
are unsupported. Strict duplicate-key and lexical integer-overflow decoding
belong to the boundary codec.

Questions have nonempty unique IDs, prompts, criteria, and an `answer_space`:

- `choice`: 2 through 255 options with unique nonempty string IDs and descriptions.
- `noul`: the probability of yes or true in [0, 1], including both endpoints.
- `score`: 2 through 10 ordered rubric levels. Level indices start at zero.

Criteria and rubric levels accept strings or `{label, description}` entries.
`TurnRequest::validate` and `TurnOutcome::validate` perform semantic validation;
successful deserialization alone does not establish a valid decision contract.
The executable fixtures in `fixtures/contracts/decision-*.json` round trip in
inline tests and demonstrate every primitive and mixed answered/refused outcomes.

Choice answers retain a selected option and a complete distribution over the
original IDs. Score answers retain their supplied fractional value and the
complete level probabilities. With probabilities [0, 0.57, 0.43], the expected
score is 1.43; the most probable level has index 1. Replacing the expectation
with that argmax index or a rounded rating is invalid.

Every probability must be finite and within [0, 1]. Providers may round each
reported probability and score to 0.01, so each true value may differ from its
report by at most 0.005, within [0, 1]. A distribution is valid when some set of
true values within those bounds sums to 1. A score is valid when it lies within
0.005 of an expectation that some such set of true level probabilities produces.
Both checks allow a small floating arithmetic epsilon. Validation never renormalizes probabilities or
replaces the supplied score. Confidence is optional, finite native provider
uncertainty with an explicit meaning. Its scale is provider-specific; it is
neither the rubric score nor implicitly the probability of a proposition.
Provider/model identity and token usage accompany normalized outcomes. Missing
usage is unknown, never invented as measured usage.

Every question has exactly one `answered` or `refused` outcome. Refusal preserves
usable sibling answers and is distinct from transport failure. Missing outcomes
are invalid. Structured JSON null is a present data value; absence of a result
and refusal have their own representations. Primitive answers cannot be null.
Model judgments do not assert caller adoption or completed actions. No implicit
acceptance threshold or generated explanation is part of this contract.
