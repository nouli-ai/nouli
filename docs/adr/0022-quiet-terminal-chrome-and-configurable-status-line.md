---
status: accepted
date: 2026-11-01
---

# Quiet the terminal chrome and make the status line configurable

## Context

The full-screen view had grown crowded: a permanent Enter-to-send banner, a
bottom shortcut legend, a hold-view hint in the header, and a fixed model line.
The user chose the Quiet rails design, a closer adaptation of the brand bee, and
the removal of every function-key binding. F5 duplicated Enter and Ctrl-S, and
some terminals and keyboards reserve the function keys.

## Decision

- Remove all function-key bindings. Ctrl-S sends from any field. Ctrl-P holds and
  resumes the view; Esc also resumes. Native drag-selection is unchanged.
- Show the answer types at the right end of the Question rule, keeping their
  colors and brackets. When they do not fit, show only the chosen type with `⌄`.
- Replace the send banner and legend with a status row and an activity row. The
  status row shows the chosen fields and `Shift-Tab question type`, dropping that
  hint when the fields leave no room. The activity row appears only when
  there is something to say: a running request, a message, the hints for an open
  list, or the pending second Enter in Options/Levels. Errors use it, so they stay
  visible when every status field is hidden. The held view's hint replaces the
  status row so holding moves no text.
- Open `/help` and `/statusline` as dialogs. The status line offers fixed built-in
  fields and presets and never executes anything.
- Save the status line in `tui.toml` beside `config.toml`. It is a display
  preference of this view, and `nouli-core` rejects unknown configuration keys.
  An unreadable file gives the default and does not stop the view.
- Draw the bee from a table of half-cells sampled from the brand SVG, with the
  body clipped symmetrically about the face. The brand axis falls between cell
  centers, so plain sampling leaves the lower body lopsided.
- Give the decisions pane the full height. Keep its width rules, Ctrl-G behavior,
  and divider dragging. The Select text control stays in the header only while
  graph focus captures the mouse.

This supersedes the F5 and F6 bindings in
[ADR 0006](0006-literal-form-and-file-drops.md),
[ADR 0009](0009-compact-keyboard-terminal-view.md),
[ADR 0010](0010-consistent-tab-navigation-and-list-entry.md),
[ADR 0013](0013-store-wide-saved-answer-references.md),
[ADR 0020](0020-native-terminal-copy-mode.md), and
[ADR 0021](0021-default-native-text-selection.md), and the Enter-to-send hint in
ADR 0009.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Inset composer or single dock layouts | The user selected Quiet rails. |
| Add the status line to `config.toml` | Widens `nouli-core`'s schema for a preference only this view reads, and older versions would reject the file. |
| Script-driven status fields | Running commands from a status line adds execution risk for little benefit. |
| Keep `/help` in the transcript | It mixed shortcuts into history; the `/` list already describes each command. |
| Keep hidden F5 and F6 aliases | The user asked for all function-key bindings to go, including hidden aliases. |

## Consequences

- `/help` no longer prints to the transcript; the `/` list carries command
  descriptions, so shortcuts and commands live in different places.
- Status fields and dialogs are keyboard-only: mouse reporting stays off so the
  terminal keeps ordinary dragging.
- Readiness checks can no longer wait for `Ready`, which the view stopped showing.
- The bee's table must be regenerated from the brand SVG if the brand bee changes.
