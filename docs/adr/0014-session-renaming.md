---
status: accepted
date: 2026-10-30
---

# Rename the current session without changing its identity

## Context

Automatically generated session names help discovery, but users need a name
that describes their conversation. The user requested `/rename` and confirmed
that its scope is the current session.

## Decision

- `/rename NAME` changes the current session's existing name. Use the same
  validated, case-sensitive namespace and unique-name constraint as `/new`.
  Command completion and help expose it in the full-screen and line loops.
- Resolve the current session by UUID. In one immediate SQLite transaction,
  update its name and append a `SessionRenamed` history event containing the
  previous and new names. An existing name fails without changing anything;
  renaming to the same name is a no-op.
- Preserve the session UUID, sources, frontier, model defaults and revision.
  Metadata renaming does not invalidate accepted work already bound by ID.
  Saved questions, answers, manifests and reference snapshots stay immutable.
- Refresh the current label and graph immediately without clearing queued
  facts, draft questions, answer lists, references, or answer-list reuse.
  Refresh cached catalog labels on the next `@` search so a renamed source is
  discoverable under its new name. Previously saved reference labels retain
  their historical text; their UUID bindings remain valid.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Create a new session with the requested name | Would change identity and split the existing conversation. |
| Rename question IDs or rewrite old prompts | Outside the confirmed scope and would alter saved provenance. |
| Increment the evaluation revision for a label change | Would reject accepted work despite unchanged decision inputs. |

## Consequences

The new name works for reopening and selectors. The old name becomes available
again, while immutable session IDs and references continue identifying the same
history. No schema migration or model request is needed.
