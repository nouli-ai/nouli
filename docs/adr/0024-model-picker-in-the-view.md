---
status: accepted
date: 2026-11-01
---

# Choose models in the view from what providers list

## Context

`/model` left the full-screen view because it reused the line prompts of
`nouli connect`, and it offered no list beyond configured limits. Nouli accepts
any model ID and has no catalog of its own.

## Decision

- `/model` and Ctrl-L open a search list over the transcript, like the `@` and `/`
  lists. Each row shows the model ID and, in a separate dim column, its provider.
  Model IDs can contain `/`, so `provider/model` is ambiguous. The status row
  shows `model · provider`.
- The list opens at once from the configuration. Each connection with a usable
  credential is then asked for `GET v1/models` in the background, and its models
  join the list. A failed list leaves only the configured models.
- Leave out models that plainly cannot answer in text: `architecture.output_modalities`
  without text, and names for speech, image, moderation, and embedding models.
  No provider reports decision support, so anything else stays and the provider's
  error covers a model that cannot answer.
- Choosing a model saves it as the startup default. This replaces the explicit
  default step in the picker.
- A missing key is asked for in place, and an empty key goes on without one.
  **Add connection** sets up services that need at most a key. **Other…** uses
  `/connect`, which still leaves the view for its longer prompts.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Configured and recent models only | Leaves the list nearly empty for most setups. |
| A built-in model catalog | Goes stale, and Nouli would have to maintain it. |
| A dialog or shortlist strip | The inline list matches `@` and `/` and keeps the draft visible. |
| Separate session and default choices | Adds a decision; people expect a pick to stay. |

## Consequences

- Long provider lists rely on typing to filter, and can include models that fail.
- The picker makes read-only requests to each connection when it opens, using that
  connection's own credential.
- Cloudflare Workers AI has no list here and shows configured models only.
