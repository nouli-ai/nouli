---
status: accepted
date: 2026-10-30
---

# Keep the graph across sessions and prepare native image requests

## Context

The terminal pane cleared whenever `/new` selected an empty session. A screenshot
sent to Jev failed before inference because the byte estimate counted base64 as
text; its width also exceeded Jev's documented image limit. Tab also left the
Options section instead of moving through its items.

## Decision

- The pane shows the union of sessions visited during the current TUI run.
  Starting an empty session retains the existing graph and scroll position.
  Later decisions join the display as independent roots unless stored ancestry
  provides actual parent edges. Shared ancestors appear once. The selected
  populated session supplies the latest marker; an empty selection retains the
  previous one. Sessions and their inference contexts remain independent.
- Tab is contextual: in Context it moves to Question; in Question it cycles
  answer types forward; in Options/Levels it advances to the next item, adding
  an empty item after the last filled one. Tab on the trailing empty item
  returns to Context without sending. Shift-Tab cycles types backward; F2
  cycles them forward. Arrows and clicks move between fields.
- Save original imported image bytes. For direct TypeSafe and routed Jev,
  derive an upright still PNG, resize proportionally within 1568 pixels, and
  scale up or pad to at least 280 pixels per axis. EXIF orientation is applied
  to pixels. This follows [TypeSafe's image preparation rules](https://docs.typesafe.ai/images),
  checked 2026-10-30. Other providers retain their native image handling.
- Image evidence is removed from text counting before budgeting. Jev and direct
  OpenAI use local pixel-based heuristics, counting every expanded reference
  occurrence in shared state and each live question. These are explicitly
  `multimodal_estimate` counts with `exact: false`; TypeSafe publishes no exact
  image-token formula. Model usage remains the authoritative returned usage.
- Non-Jev routed models retain their encoded-byte estimate, because Clef/Flash
  apply such a capacity check before processing pixels. Each distinct non-Jev
  routed image is dispatched and charged once; those reference strings remain
  text. OpenAI option descriptions and score levels also remain text-only. See
  [OpenRouter's multimodal guide](https://openrouter.ai/docs/guides/community/multimodal-decisions).

This supersedes ADR 0006's Tab navigation and encoded-image-byte budgeting.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Preserve the old graph only until the next answer | New answers would still discard earlier independent session graphs. |
| Merge session histories to keep every card visible | Presentation must not create inference ancestry. |
| Increase context limits or enable long context | The failure is incorrect binary counting and image sizing. |
| Resize the stored original | Retain original evidence and derive a provider-specific copy instead. |
| Assert a vendor-exact visual token formula | No such formula is published for Jev. |

## Consequences

- Graph membership lasts for the current terminal run; saved sessions remain
  accessible through `/sessions` on later runs.
- Original evidence and provider-bound image derivatives differ in dimensions
  and sometimes pixels. The original and deterministic preparation rules remain
  available for audit; the context manifest does not contain the derivative's
  encoded bytes. Byte-for-byte native requests are not persisted.
- Heuristic image counts can differ from remote accounting. Genuine capacity
  errors remain possible and do not discard pending input.
- Native request tests cover dimensions, orientation, padding, repeated image
  references, and independent graph roots. A live Jev check is separate evidence
  from these deterministic checks.
