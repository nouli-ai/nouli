# JSON and TOON boundaries

Nouli uses the official [`toon-format`](https://github.com/toon-format/toon-rust)
0.7.0 codec targeting TOON specification 4.4. Public decoding always selects
strict parsing. Array counts, indentation, duplicate keys/columns, escaping,
and structure are validated before typed decoding. Native provider envelopes
remain a provider adapter concern.

Use `codec::decode_request` for turn/workflow input. It defaults omitted versions
to 1 and infers kind only when exactly one of `questions` or `nodes` exists. Both,
neither, unknown fields, unsupported media, invalid versions, and invalid request
semantics produce typed errors. Generic `codec::decode<T>` decodes other contract
shapes; the caller then runs the contract's semantic validator. Explicit JSON or
TOON input never falls back to another format. File extensions `.json` and `.toon`
select formats; stdin callers pass an explicit format.

Integers retain exact i64/u64 values; fractional and exponent forms use finite
f64 values. JSON integers outside i64/u64 fail before serde_json can round them
through f64. TOON additionally rejects raw numeric tokens whose out-of-range
fallback would become a string. Canonical decimals emitted by the official
encoder for large finite f64 values remain supported. To provide numeric-looking
text outside that domain or strings such as `NaN`, quote them. Ordinary unquoted
text with embedded digits or commas remains text. Errors omit supplied content,
including the source-line context contained in upstream TOON parser errors.

Integral floating-point spellings normalize to equal integer values where exact;
negative zero normalizes to zero. This follows TOON value semantics and does not
pass integer values through f64. Object keys sort lexically for deterministic
encoding/fingerprints; array order, rubric level order, and canonical history
order remain intact. Request fingerprints are SHA-256 of canonical serialized
typed requests, so omitted/default versions and JSON/TOON have equal identities.
Null remains present and differs from absent fields or typed reference absence.

`codec::encode_toon` uses the official encoder and verifies a strict decoded
round trip against the original canonical values. It never builds table rows by
concatenating caller strings. Fixture tests cover full-width integers, finite
floats, Unicode, multiline strings, punctuation, all decision primitives,
workflow definitions, and complete ordered history. The checked-in upstream
fixture corpus and license are under `fixtures/toon/upstream`.
