---
status: accepted
date: 2026-10-30
---

# Search conversation text and preview the saved input state

## Context

`@` searched question titles and identifiers across the store, but users often
remember words from their context or an answer. Browse history displayed a
question and result without showing the input that produced it. The user
requested ordinary text search across conversations and state data in previews.

## Decision

- Extend the existing in-memory catalog with readable authored text from each
  finalized turn: direct/shared state, new and replacement evidence, the selected
  question's wording, criteria and options/levels, and its saved result. Search
  plain words case-insensitively across all local sessions. Keep optional exact
  question/turn IDs and session-name lookup without introducing query syntax,
  an embedding model, a search service or a second database.
- Exclude encoded image bytes from searchable text. Keep filenames, structured
  keys and values, including booleans, null and numbers. Noul remains P(yes),
  without inventing a binary selection. Add a short contextual excerpt when
  a match is outside the question title; project internal attachment markers
  before truncating. Literal brackets remain searchable. Ignore `@` anchors
  inside complete attachment spans, including labels followed by more text.
- Continue selecting an exact saved question and answer, identified by session,
  turn and question. Carry its canonical saved input state beside its question,
  result and native image snapshots, so a context match remains usable evidence
  after insertion. Use the immutable manifest's image-free text representation
  with separate native image bytes. This replaces hand-picked reference-input
  dependencies with the complete frozen input. Existing snapshots stay readable;
  an explicit re-selection creates a complete snapshot when a legacy binding
  lacks state. It never blends ancestry or changes active facts in either session.
  An ancestry alias can deduplicate the question, result and native image payloads,
  but retains its frozen input state even after source facts are superseded.
- Load state only for the selected expanded preview, from that turn's immutable
  `manifest.context_toon`. Show the state and historical context actually sent,
  readable structured values and image names. Do not substitute the session's
  current active facts or display base64 image payloads.
- Keep one preview cached by source identity. Arrows choose results; PgUp/PgDn
  scroll the preview independently. Reserve room for history even with a long
  draft, use side-by-side panes when wide and stacked panes when narrow, and
  keep the draft cursor and universal Tab/Shift-Tab behavior.

This extends [ADR 0013](0013-store-wide-saved-answer-references.md).

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Search only question text | Misses the context and answer words users remember. |
| Index every turn's entire model manifest | Duplicates inherited history and native image payloads in memory. |
| Preview current session facts | Misrepresents historical inputs after replacements or later turns. |
| Add a full-text database or semantic retrieval | Unnecessary for extending the existing local literal-search path. |

## Consequences

Conversation discovery covers saved finalized turns, including workflow turns.
Unsent drafts and failed attempts are not new searchable answers. The catalog
keeps authored text; only the selected preview reads its full saved input.
State is an inspection surface, not an edit to stored or submitted evidence.
