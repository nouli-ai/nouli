---
status: accepted
date: 2026-10-30
---

# Separate mode selection, field navigation, and finishing a list

## Context

Tab changed answer types in Question but navigated other fields. Shift-Tab
cycled backwards and F2 offered another type shortcut. This made navigation
unpredictable. A single Enter also submitted from a populated option/level,
making it too easy to send before finishing a list.

## Decision

- Shift-Tab is the only answer-type shortcut. Cycle in display order:
  Yes/No → Choose one → Score → Yes/No. Show `Shift-Tab →` beside the types.
  Handle both terminal encodings, BackTab and Tab with the Shift modifier.
- Handle Shift-Tab before command/session lists and graph navigation so it
  works universally while idle. Close those overlays and return to the form.
  Running requests keep their existing input lock and cancellation behavior.
- Tab navigates Context → Question → Options/Levels → Context. Yes/No skips
  the list. Enter a populated list at its first item, advance to the next item,
  append a blank after the last filled item, and leave a trailing blank without
  submitting. Existing command completion and graph focus behavior remain.
- In Options/Levels, Enter adds a new row and arms a second consecutive Enter
  to send. Typing, pasting, changing mode, or navigating cancels that pending
  confirmation. An initial Enter on a blank row only arms it. The second Enter
  performs ordinary question/list validation before dispatch.
- Keep single-Enter submission in Question and ready Context. Ctrl-S/F5 still
  submit directly. Show a concise list-specific send hint, changing to
  `Press Enter again to send` after the first Enter.

This supersedes the type-selection/navigation decision in
[ADR 0007](0007-continuous-graph-and-native-image-preparation.md) and single-Enter
submission from lists in [ADR 0009](0009-compact-keyboard-terminal-view.md).
Answer-list reuse, retained drafts, and immutable saved turns are unchanged.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Keep contextual Tab and backwards Shift-Tab | The user explicitly requested one universal forward type shortcut and Tab for navigation. |
| Keep F2 as another shortcut | The user requested only Shift-Tab for switching modes. |
| Require a rapidly timed double Enter | Ordinary consecutive Enter presses allow time to review the completed list. |
| Let any Enter on a trailing blank send | A blank reached with Tab should still require the deliberate second Enter. |

## Consequences

- A user can add options or levels with Enter without submitting prematurely.
- Switching to Yes/No from a list moves focus to Question, while retaining the
  list for a later choice/score.
- A pending second Enter never survives an edit or navigation action.
