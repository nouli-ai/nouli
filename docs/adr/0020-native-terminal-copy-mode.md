---
status: accepted
date: 2026-10-31
---

# Return mouse selection and copying to the terminal on demand

Default mouse capture and the normal-mode header control are superseded by
[ADR 0021](0021-default-native-text-selection.md). The F6 hold-view behavior remains.

## Context

[ADR 0018](0018-unfocused-divider-dragging.md) captures the mouse so the graph
divider can be dragged without changing keyboard focus. Ordinary text dragging
then requires a terminal-specific override. Users need a discoverable way to
select and copy text before a release, preserving existing input and graph behavior.

## Decision

- Add a visible Select text control in the header and an F6 shortcut. Either
  enters native terminal selection mode, temporarily disabling mouse reporting.
  Users drag normally and use their terminal's Copy command.
- Draw the selection instructions once and hold the visible screen. Model
  requests continue and their results are recorded normally; display updates
  appear when the user resumes. This prevents a redraw from erasing selection.
- Esc or F6 resumes the previous focus, draft, lists, and graph width. Resize
  resumes normal rendering. Ordinary typing, paste, and mouse events cannot edit
  or submit while selecting. Ctrl-C leaves selection without clearing idle
  input, and still cancels an active request.
- Restore mouse reporting on resume and disable it when leaving the TUI,
  including error cleanup. Retain the host's modifier-based selection override
  in normal mode.
- Keep native terminal copying. Do not introduce an application clipboard,
  automatic clipboard writes, or platform-specific clipboard dependencies.

This supersedes ADR 0018's lifetime-wide mouse reporting only while the user
explicitly selects text. Divider dragging and pointer-based wheel routing
remain available in normal mode.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Only document Option or Shift dragging | The modifier varies by terminal and remains hard to discover. |
| Disable capture by default | Would remove the requested ability to drag the divider at any time. |
| Implement selection and clipboard writes inside Nouli | Would replace native copy behavior and add clipboard policy, encoding, and platform concerns. |
| Keep refreshing while native selection is active | Redraws can clear or move the selected text. |

## Consequences

Copying remains owned by the terminal and requires an explicit Copy action.
The user resumes the display after copying; active model requests are not
paused. A PTY regression check verifies reporting transitions, stable display,
draft preservation, resize, and terminal cleanup without provider calls.
