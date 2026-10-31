---
status: accepted
date: 2026-10-30
---

# Keep runtime connections independent from service setup shortcuts

## Context

Users must be able to point Nouli at any compatible decision service using
hand-written configuration. Provider-specific adapters and the bundled model
capacity catalog coupled endpoint names, image formats and limits to Nouli
releases. People also need an easy way to choose a service, authenticate and
select a model. The user approved breaking changes before public adoption.

This supersedes the provider-specific selection in
[ADR 0001](0001-decision-session-harness.md) and the implicit Jev image preparation
in [ADR 0007](0007-continuous-graph-and-native-image-preparation.md).
[ADR 0003](0003-central-credential-store.md) still governs private credential
storage. Decision-only sessions and append-only history remain unchanged.

## Decision

- Core owns two decision codecs: `system-one` and `openai-decisions`. An explicit
  HTTP binding selects standard routing or Cloudflare Workers AI's model route
  and response wrapper. Clef's choice, noul and score schema reuses System One;
  Workers AI's HTTP wrapper is a binding, not another decision protocol.
- Resolve endpoint, protocol, model, request suffix, wire-model override, auth
  header/scheme, nonsecret headers, environment-backed secret headers, optional
  body fields, image encoding/preparation/counting and optional limits from user
  settings only. Neither host nor provider/model names select runtime behavior.
- Remove the runtime provider/model registry. Omitted budgets use an operational
  8192-token estimate, clamped to an explicit context window. Optional limits
  overlay global defaults, provider defaults and exact model overrides; CLI
  budget wins. Model tables are not an allowlist.
- CLI setup recipes are convenience authors of ordinary settings, with service
  endpoint/auth/media suggestions. They contain no hidden runtime state. Users
  can edit, rename or reproduce their output manually. Store credentials and
  their sources separately. Setup is local; no discovery or inference request
  runs during configuration.
- Provide `nouli connect`, terminal first-run setup, and idle `/connect`, `/login`,
  `/logout`, `/model`. Keep draft input and reload configuration after setup.
  Preserve unrelated TOML edits, serialize writers, validate before publishing,
  save credentials first, then atomically replace a private config file.
- Freeze the effective configuration and credential source names in version 2
  snapshots. Re-resolve secrets on resume. Reject older frozen settings with
  fresh-store guidance before attempting to decode their old profiles. Do not
  reset stores, delete records or silently migrate historical configuration.
- Preserve complete distributions, fractional expectations, attributed confidence,
  native refusals where specified, and served model/request/usage metadata.
  This adds no chat planner, tools, provider fallback, OAuth or NoulXP executor.

## Considered alternatives

| Alternative | Reason for this decision |
| --- | --- |
| Provider-name adapters and bundled model limits | Require Nouli updates for deployment-specific settings and future models. |
| Entirely generic setup without shortcuts | Correct runtime boundary but unnecessarily difficult authentication and endpoint entry. |
| A new protocol for every HTTP envelope | Duplicates shared choice/noul/score schemas. Separate reusable binding from codec. |
| Universal OpenAI-shaped decisions | Decision APIs have distinct adopted schemas; preserve both native codecs rather than translate through chat. |
| Automatic migration of old snapshots | No existing users require it, and new behavior must not alter frozen inference settings. |

## Consequences

Service recipes may age; their output remains editable and custom setup remains
available. New wire schemas still require new codecs; new HTTP envelopes may
require new bindings. Image/token estimates are explicit approximations, not
capability guarantees. A crash between credential and config publication can
leave a harmless credential entry to reuse. Old stores remain available to the
matching earlier binary, while new inference uses a fresh store.
