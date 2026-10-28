# Official TOON conformance fixtures

These four unmodified decode fixture files and `LICENSE` are from the
[TOON specification v4.4.0 tag](https://github.com/toon-format/spec/tree/v4.4.0/tests/fixtures/decode).
The official `toon-format` 0.7.0 Rust codec targets specification 4.4.

Inline tests execute every case in these files using the declared decode options,
including upstream non-strict recovery cases. Nouli's public boundary always uses
strict parsing plus its documented numeric-domain guard. Upstream fixture
expectations apply to the official codec; Nouli additionally rejects unquoted
non-finite/out-of-domain numbers instead of accepting their fallback strings.
