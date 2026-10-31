---
status: accepted
date: 2026-10-30
---

# Resize the graph without changing keyboard focus

Default mouse capture is superseded by [ADR 0021](0021-default-native-text-selection.md).

Lifetime-wide reporting is superseded during explicit text selection by
[ADR 0020](0020-native-terminal-copy-mode.md). Normal divider dragging is unchanged.

## Context

The user explicitly requested divider resizing outside Ctrl-G graph focus.
Terminals report divider presses only while mouse reporting is enabled.

## Decision

Enable mouse reporting for the TUI lifetime and allow a divider drag regardless
of keyboard focus. Keep Ctrl-G for graph keyboard navigation. Preserve the draft,
focused field, pane minimums and chosen width. Cancel dragging on release, key
input, paste or terminal resize. Route wheel scrolling to the pane under the
pointer; keep result/session lists from scrolling the underlying transcript.

Use the terminal's native selection override for copying text, such as Option
in iTerm2, and document the modifier instead of claiming Esc disables reporting.
Always disable reporting when leaving the screen, including error cleanup.

This supersedes the mouse-focus restriction in
[ADR 0016](0016-reference-graph-and-divider.md); reference graph semantics stay
the same. Adding a separate in-app selection/clipboard system was considered
but would duplicate host terminal behavior beyond the requested resize change.
