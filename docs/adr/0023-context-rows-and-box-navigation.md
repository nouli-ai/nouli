---
status: accepted
date: 2026-11-01
---

# Make Context rows simple and let Tab move between boxes

Blank-row Enter navigation is superseded by [ADR 0025](0025-blank-enter-section-navigation.md).

## Context

Enter in Context either queued the text or sent the question, depending on state,
and nothing on screen said which. [ADR 0022](0022-quiet-terminal-chrome-and-configurable-status-line.md)
removed the Enter-to-send hint, which made this harder to predict. Tab walked list
items, so reaching the next box from Options took several presses.

## Decision

- Enter in Context adds a row and never sends. Slash commands still run from it.
  Enter in Question sends, and Ctrl-S sends from any box.
- Tab moves between boxes: Context, Question, then Options/Levels. Up and Down
  walk the rows of Context and Options/Levels, and move to the next box past the
  last row. Enter adds list rows.
- The input row moves through queued Context rows and holds the row being edited,
  always as literal text, never a command or a search. Backspace on an empty row
  reopens the row above.
- Show the active box through the box itself. Inactive headings dim. The empty box
  Tab goes to reads `Tab to add ...`, and other empty boxes keep their usual hint.
  All answer types show only while Question is active. Three or more Context rows
  fold into a count while Context is inactive.
- Keep `Shift-Tab question type` in the footer, because it works from every box.

## Considered alternatives

| Alternative | Reason for the chosen direction |
| --- | --- |
| Enter moves from Context to Question | Took the form pattern too far; adding several rows needed extra keys. |
| Context as one multi-line text box | Clean, but the user preferred rows. |
| Numbered Context rows matching Options | Adds numbers to quick notes. |
| A footer hint naming Tab's destination | Replaced by the hint text in the boxes, which sits where the user is looking. |
| `Enter add` and `Enter send` labels | Added text to explain behavior that can be made predictable instead. |

## Consequences

- Updating an answer with new context takes Tab then Enter, or Ctrl-S, rather than
  one Enter. In exchange, Context cannot send by accident.
- Retrying a failed request shows `Ctrl-S retry` outside Question.
