# OpenRouter setup using System One

OpenRouter is a setup shortcut, not a runtime protocol or model registry. It
writes protocol `system-one`, base URL `https://openrouter.ai/api`, explicit
`state-image-urls` media encoding and `extra_body.provider.allow_fallbacks=false`.
The standard suffix yields `/api/v1/systemone`. Custom prefixes and suffixes are
ordinary configuration. See the [TypeSafe SDK guide](https://openrouter.ai/docs/guides/community/typesafe-sdk),
verified 2026-10-30. There is no implicit `/alpha/decisions` route.

Use an exact-model connection override for an upstream model that accepts a
different media encoding, such as `assets`. No model ID selects that behavior.
Model suggestions are examples, never an availability guarantee or allowlist.

The shared codec validates named answer coverage, full distributions and fractional
score expectations. Optional native ID, upstream provider and usage cost are
retained on any compatible System One response. Missing metadata stays absent;
invalid or negative costs fail. Native confidence remains attributed. Retry and
context reduction belong to the shared engine; no hidden fallback chain runs.
