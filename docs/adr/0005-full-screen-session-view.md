---
status: accepted
date: 2026-10-29
---

# Replace the line prompt with a full-screen view

UUID-fragment labels for new anonymous sessions are superseded by
[ADR 0011](0011-generated-session-names.md).

The form grammar and keyboard behavior below are superseded by
[ADR 0006](0006-literal-form-and-file-drops.md).

[ADR 0002](0002-verb-cli-standing-questions-and-repl.md) made the interactive
session loop a line prompt and turned down a full-screen terminal UI as the
first step, because it is more work and harder to use over SSH or in scripts.
The prompt has a cost for people. Picking an answer type means remembering
which brackets the grammar uses: `[a, b]` for a choice, `<a, b>` for a score,
and none for yes/no. A question with long options is written as one long line.
The decision graph is out of sight in `nouli view`. This record replaces the
interactive line prompt with a full-screen view, superseding ADR 0002's
line-based loop in a terminal. Scripts keep reading lines.

## Decision

- **The only interactive view.** A bare `nouli` and `nouli open SESSION` open
  the view when standard input and output are terminals. A bare `nouli` starts
  a new session, known by its ID, and `nouli open` creates a missing session
  without asking, so nothing is asked in lines before the view opens. The line
  prompt is removed. Without a terminal, the loop still reads lines from standard input
  and echoes them, so a script reads as a transcript, and a terminal that takes
  no escape codes (`TERM=dumb`, as in an Emacs shell buffer) is read the same
  way.
- **The view is a front end to the same loop.** Its input row takes every line
  the prompt took: facts, `?` questions, and slash commands. The transcript
  shows what the loop prints. The form writes the same grammar line a `?` line
  would, and checks that it parses back unchanged before sending. A turn still
  asks one question in one call, as
  [ADR 0004](0004-ask-each-question-once.md) requires, and saves through the
  same ledger operations.
- **A form with three sections.** State lists queued facts and replacements
  above the input row, and a question that a failed `?` line left to send
  again. Question holds `id: prompt` and shows the answer type. Options, or
  Levels for a score, lists one item per row, and noul has neither. Shift-Tab
  cycles through noul, choice, and score. Tab moves to the next section, and
  Up and Down move across rows and sections. Enter queues a fact or runs a
  line, sends from Question or from an empty option, and moves to Options
  first when a choice or score has fewer than two.
- **Decisions on the right.** The pane draws the cards `nouli view` draws,
  refreshes after each saved turn, and keeps the latest card in view until it
  is scrolled. It sits beside the transcript when the terminal is at least 116
  columns wide: 76 for turn output and 40 for one card. Narrower terminals show
  it in place of the rest while it has the keys. Ctrl-G gives it the keys and
  Esc takes them back.
- **The view keeps drawing while a turn runs.** Raw mode turns Ctrl-C into a
  key, so the view cancels a turn itself, through the cancellation the verbs
  use for the signal. Ctrl-C also clears the row, then the queue and form,
  then quits.
- **ratatui on crossterm.** It draws on the alternate screen with bracketed
  paste, and restores the terminal when it exits, fails, or panics. Printed
  escapes other than colors and emphasis never reach the screen.

## Considered options

| Alternative | Reason for the chosen direction |
| --- | --- |
| Keep the line prompt beside the view, behind a flag | Two interactive front ends to keep alike, for no case the view can't serve. Scripts and terminals without escape codes keep reading lines. |
| Opt in with a flag and keep the prompt as the default | The same two front ends, with the better one hidden. |
| A separate `nouli tui` command | A second way to open the same session loop. |
| Draw with escape codes and no library | Layout, wrapping, wide characters, and redrawing only what changed would all be written again. ratatui uses crossterm, which the view needs for keys anyway. |
| Keep the graph behind `/view` | `/view` lists sessions, not decisions, and its output scrolls away. The pane shows the chain as it grows. |
| Let the form ask several questions at once | ADR 0004 asks each question in its own call, and the form keeps to that. |

## Consequences

- The loop no longer runs turns itself. A line that sends a turn returns a
  step to whichever reader took it, the view or piped input, and that reader
  runs it.
- New dependencies: `ratatui` and `crossterm` (the version ratatui uses),
  `futures-util`, and tokio's timer. The line editor, `rustyline`, and its
  history file go with the prompt.
- The transcript goes when the view closes. The ledger keeps everything, and
  `nouli facts`, `nouli view`, and `nouli sessions show` show it.
- Below 116 columns the decisions are behind Ctrl-G.
- Each bare `nouli` creates a session before anything is typed. `/view` and
  `/sessions` leave out the ones that stayed empty and unnamed.
- Key handling, the form, and drawing are unit-tested with ratatui's test
  backend. Nothing in CI drives the view in a real terminal.
