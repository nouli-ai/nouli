---
status: accepted
date: 2026-10-30
---

# Draw exact reference dependencies and resize the graph divider

## Context

Saved answer references preserve exact session, turn and question identities,
but the graph previously showed only turn ancestry. A destination could appear
independent of its referenced answer, especially after restarting. Users also
need to adjust the graph width while retaining ordinary terminal text selection.

The mouse-focus portion is superseded by [ADR 0018](0018-unfocused-divider-dragging.md).

## Decision

- Derive graph edges from included saved reference bindings, targeting the
  question that mentions the binding. Shared evidence and retained references
  apply to the questions using that turn's shared context. Omitted bindings
  create no edge. Deduplicate and order dependencies deterministically.
- Load each referenced source's fixed ancestry for graph presentation, recursively.
  Exclude later source turns. Preserve ledger parents, active facts and structured
  session exports; references remain evidence rather than session blends.
- Keep existing turn cards when possible. Split multi-question turns participating
  in references into individual question cards, so edges identify the actual
  source and destination questions. Sort parents before children; Mermaid uses
  dashed reference edges beside ordinary ancestry arrows.
- Use existing Ctrl-G graph focus for divider dragging and wheel scrolling.
  Scope mouse capture to that focus and restore native selection on Esc, Tab,
  form input or exit. Close other pickers when entering graph focus.
- Retain the chosen width for the TUI run, leaving at least 40 columns per pane.
  Clamp on terminal resize and use the existing focused full-width graph when
  both panes cannot fit. New runs retain the existing automatic default width.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Add reference sources as ledger parents | Would change inference ancestry and active facts. |
| Connect whole turns only | Ambiguous when several independent questions share a turn. |
| Always capture the mouse | Interferes with native terminal text selection. |
| Add persistent layout settings | Unnecessary for dragging the existing divider. |

## Consequences

Graphs can show saved source history from sessions not visited in the current
TUI run. Reference presentation reads the same local ledger without a migration,
new dependency or inference change. Divider resizing uses the existing graph
focus shortcut, keeping ordinary form editing and copying unchanged.
