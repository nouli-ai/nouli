---
status: accepted
date: 2026-10-31
---

# Give ordinary text dragging to the terminal by default

The F6 binding and permanent header hint are superseded by
[ADR 0022](0022-quiet-terminal-chrome-and-configurable-status-line.md); Ctrl-P
now holds the view.

## Context

Capturing the mouse by default in [ADR 0018](0018-unfocused-divider-dragging.md)
prevents ordinary native text selection. The explicit copy mode from
[ADR 0020](0020-native-terminal-copy-mode.md) works, but the user now requests
plain drag-selection without entering a mode or holding a terminal modifier.

## Decision

- Start with mouse reporting disabled. Ordinary dragging selects terminal text
  and copying uses the host terminal's Copy command.
- Capture the mouse only while Ctrl-G graph focus is active, for divider
  resizing and wheel scrolling. Esc, Tab, or returning to form editing restores
  native text selection. Ignore stale mouse events outside graph focus.
- Keep F6 to hold the visible screen while requests continue. The header shows
  a keyboard hint normally; the clickable Select text control is available in
  graph focus. Resuming restores the previous graph focus and draft.
- Update the graph hint so divider resizing is discoverable through Ctrl-G.
  Always disable reporting on startup, setup prompts, and terminal cleanup.

This supersedes the default capture decisions in ADRs 0018 and 0020.

## Alternatives and consequences

Always capturing the mouse would still require a selection modifier or explicit
mode. Implementing selection and a clipboard inside Nouli would replace native
terminal behavior and expand this fix. Default native selection instead means
divider dragging and application wheel scrolling require Ctrl-G. F6 remains
useful to prevent background display updates while copying during a request.
