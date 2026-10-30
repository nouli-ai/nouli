---
status: accepted
date: 2026-10-30
---

# Show the latest question in context and display images by filename

## Context

The form did not make it clear that the preceding question and answer carried
forward into a follow-up. Imported images displayed an internal UUID token after
their filename. The user requested a context cue and filename-only image labels,
while retaining color on both questions and answers in the side graph.

## Decision

- Show `Context · latest question included` and preview the latest reachable
  saved question in the selected session's ancestry, including Yes/No and
  inherited branch history. Independent new sessions have no prior context cue.
- Require that decision and all new/replacement evidence in TUI submissions.
  Retain images present in its saved input manifest or referenced by its saved
  questions. A capacity failure keeps the draft instead of dropping that context.
- Keep answer-list reuse independent of the cue. Yes/No still needs a fresh
  prompt; including its prior answer does not re-ask it. Saved turns stay
  immutable, and requirements apply to this request without permanent pins.
- Display image filenames in the form, transcript, graph, and session previews.
  Hide internal `[image:UUID]` markers in human text, preserving stored text,
  payloads, and native provider references. Structured inspection retains them.
- Map the cursor to visible text. Treat a dropped filename/reference pair as one
  attachment for arrows, deletion, and word deletion. Preserve AltGr typing.
  Removing an attachment excludes its unused payload from submission.
- Keep existing Citrus accents on graph questions and their answers.

This extends [ADR 0008](0008-citrus-and-implicit-answer-list-reuse.md)'s context
protection and [ADR 0009](0009-compact-keyboard-terminal-view.md)'s presentation.
Image storage and provider contracts in
[ADR 0006](0006-literal-form-and-file-drops.md) remain unchanged.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Show the cue without requiring the record | Reduction could remove the context the UI claims is included. |
| Automatically reuse a Yes/No prompt | Conflicts with the explicit-prompt behavior. |
| Use filenames as native identifiers | Duplicate or escaped filenames can misbind image data. |
| Hide UUIDs without adapting editing | Cursor movement would enter invisible characters and deletion could leave a misleading filename. |

## Consequences

- Follow-ups visibly retain the previous judgment and its image inputs.
- Required context can fail explicitly if a request is too large.
- Human image labels differ from stored reference text while preserving native
  image data and audit/replay provenance.
