---
status: accepted
date: 2026-11-01
---

# Move to the next form section with Enter on a blank row

## Context

The user expects Enter on an empty row to advance to the next section. Previously,
empty Context did nothing and an empty Options/Levels row could submit after a
second Enter. Both interrupted navigation through the form.

## Decision

- Treat Enter on a whitespace-only form row like Tab: Context to Question,
  Question to Options/Levels (or Context for Yes/No), and Options/Levels to Context.
  Remove the empty option when leaving it and preserve all nonempty draft rows.
- Keep Enter on nonempty Context for adding facts or running commands, nonempty
  Question for sending, and nonempty Options/Levels for adding another item.
- Keep Ctrl-S for explicit submission from any section, including reuse of a
  saved question with an empty Question row. Remove double-Enter submission and
  its pending-confirmation hint.
- Keep native terminal selection and Ctrl-G graph focus for divider dragging,
  as chosen by the user after considering direct dragging and in-app selection.

This supersedes the blank-row behavior in [ADR 0023](0023-context-rows-and-box-navigation.md).
Mouse reporting continues to follow [ADR 0021](0021-default-native-text-selection.md).

## Considered alternatives

Keeping blank Context inert or submitting from a blank list conflicts with the
requested navigation. Adding a dedicated Send section would change the form
layout. Capturing the mouse for a divider hitbox cannot preserve native dragging
elsewhere because the terminal's mouse-reporting mode applies to the whole view.
Implementing selection and clipboard handling inside Nouli was declined in favor
of native selection and Ctrl-G resizing.

## Consequences

Blank Enter cannot submit accidentally. Reusing an empty question requires Ctrl-S;
users can also return to a nonempty Question to send. Tab, editing, native copying,
and the graph's focus and width behavior remain available.
