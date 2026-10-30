---
status: accepted
date: 2026-10-30
---

# Simplify the terminal view and restore native text selection

Single-Enter submission from Options/Levels and type-selection shortcuts are
superseded by [ADR 0010](0010-consistent-tab-navigation-and-list-entry.md).

## Context

Generated question IDs and added question-mark prefixes cluttered the transcript.
Reused-answer previews, repeated model aliases, and long hints crowded the form.
Mouse capture swallowed terminal drag selection. The user requested plain question
text, a simple Nouli header, an Enter-to-send hint, and removal of the confusing
interactive what-if command.

## Decision

- Show a simple `nouli` header with version and session. Echo questions as plain
  text and omit question IDs from TUI result rows. Keep IDs in stored requests,
  structured output, and the command-line text contract. Compact TUI results use
  the available width; same-choice probability changes omit the repeated label.
- Replace repeated reuse instructions and option previews with `Options · using
  previous N` or its Levels equivalent. The preceding question stays as a dim
  placeholder. Show one model name: the selection before a request and the served
  model after a response. Keep a short context-dependent hint and `/help`.
- Display `Press Enter to send`. Enter submits from any field when a question is
  ready, including Context after a choice/score. Initial context can still be
  queued before writing a question, and slash commands run from Context. Tab
  advances/adds options and levels. Ctrl-S/F5 remain send shortcuts.
- Disable mouse reporting on entry and exit. Native drag-selection and copy are
  owned by the terminal. Remove click-to-focus/send controls and their hitboxes;
  arrows, Tab, and Shift-Tab provide field and type navigation. File drops still
  insert into the field focused with the keyboard.
- Remove `/whatif` from the interactive parser, completion, and help, including
  piped interactive input. The separate scripted CLI verb and core ephemeral
  execution contract remain outside this terminal cleanup.

This supersedes the click controls and Enter behavior in
[ADR 0006](0006-literal-form-and-file-drops.md) and
[ADR 0007](0007-continuous-graph-and-native-image-preparation.md), and the verbose
reuse notices in [ADR 0008](0008-citrus-and-implicit-answer-list-reuse.md).

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Require Shift-drag to select text | Terminal-dependent and less discoverable than normal drag selection. |
| Build a selection and clipboard system inside the TUI | Duplicates terminal behavior and adds platform-specific clipboard dependencies. |
| Keep a send button with a long shortcut legend | The requested Enter hint is shorter and matches the primary action. |
| Remove IDs from durable contracts | Rendering can hide machine identifiers without changing stored provenance or scripting. |

## Consequences

- Mouse selection uses the user's terminal; field editing uses the keyboard.
- A follow-up can be sent directly from Context with Enter, while Tab creates
  list items without accidentally submitting a partial list.
- Existing saved decisions and source identities remain intact. CLI result
  layouts retain their question IDs; the full-screen transcript is compact.
