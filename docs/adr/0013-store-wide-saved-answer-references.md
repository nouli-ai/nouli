---
status: accepted
date: 2026-10-30
---

# Reference exact saved answers across the local store

## Context

The user selected inline suggestions with a larger history browser from the
[reference designs](../design/question-references.html), and clarified that
discovery must work across all local decisions, including unrelated sessions
and workspaces. A question ID alone can identify several saved answers over
time. Importing an entire source session would also import unrelated facts and
replacement conflicts into the current decision.

## Decision

- A boundary `@` in any editable form field searches every finalized question
  in the selected local ledger. Search wording, question IDs, session names,
  or source turn IDs without a special grammar. The default store is shared
  across working directories; an explicit `--store` selects that separate
  store. There is no implicit scan of other databases or remote services.
- Show a short inline list with the question, typed answer, session name and
  turn number. Preserve separate saved versions and questions within a turn.
  A Browse history row opens a larger list and answer preview. Failed attempts
  are absent; refused questions are identifiable and cannot be used as answers.
- Enter accepts exactly one reference and never also submits the form. Tab
  dismisses without acceptance and moves fields; Shift-Tab dismisses and cycles
  answer types left to right. Esc leaves the query as editable literal text.
  Emails do not open search. Ctrl-S/F5 require selection or dismissal first.
- Bind a reference to the source session, finalized turn and question together.
  Snapshot that exact typed question, outcome and provider as ordinary evidence,
  including saved native image inputs and explicit reference dependencies from
  the source input. Do not branch/blend sessions, replace active facts, re-ask
  the source, or change the current draft's type, options or levels.
- Require selected evidence for the current request. Retain its dependencies
  in later protected follow-ups. If the source is already in ancestry, require
  it once and send only the binding beside it; preserve explicitly required
  snapshot fields. Capacity errors keep the draft. Conflicting image reference
  strings with different bytes fail explicitly rather than choosing one image.
- Persist bindings in evidence updates and actual input manifests. Keep their
  internal markers out of human output, treating each displayed label and
  marker as one editable unit. Ctrl-O inspects the nearby reference. Removing
  an unused draft label adds no new snapshot; existing saved history follows
  the ordinary context policy. Preserve unsent
  bindings through session changes and partial form sends; allocate new evidence
  IDs when carrying a binding into unrelated history. Unknown markers fail
  explicitly instead of silently losing their source.
- Discover all owned finalized turns directly, once per identity, rather than
  traversing each session's shared ancestry or using a greatest-ID watermark.
  Cache lightweight catalog entries in the TUI and decode newly published turns
  when reopening search. This includes decisions written by CLI calls, library
  callers and workflows without a schema migration or rewriting old records.

This extends [ADR 0012](0012-visible-latest-context-and-image-labels.md) and
replaces the original design study's current-session-only search proposal.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Search only current ancestry | The user explicitly requires local decisions everywhere. |
| Resolve a question ID to the latest answer at send time | A later answer could silently change the selected source. |
| Blend the source session into the destination | Imports unrelated history and active-fact conflicts. |
| Insert only a readable question label | Loses source identity, result version and image evidence. |
| Add a second search database | Duplicates immutable ledger data and requires backfill/migration. |

## Consequences

- A reference remains meaningful after source session continuation, restart,
  original image-file deletion, or carrying an unsent draft to another session.
- Initial discovery reads historical finalized payloads once; subsequent opens
  read only newly seen payloads. The search scope is the selected ledger, not
  the current directory or just sessions visited during this TUI run.
- Explicit snapshots consume context and local storage. A source turn's images
  can be shared across its independent questions, so the snapshot retains its
  saved image inputs rather than guessing which pixels affected one result.
