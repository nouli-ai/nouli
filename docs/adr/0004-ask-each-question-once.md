---
status: accepted
date: 2026-10-29
---

# Ask each question once and never ask it again

Omitted fields on an explicit full-screen form Send are superseded by
[ADR 0008](0008-citrus-and-implicit-answer-list-reuse.md). Earlier saved answers
remain immutable.

[ADR 0002](0002-verb-cli-standing-questions-and-repl.md) gave sessions standing
questions: every later turn asked a session's questions again with the new
facts, so a turn could carry only facts. That re-decides earlier answers. In the
README's `trip` session, asking `on_time` also asked `transport` again, and its
answer moved from 1.00 to 0.11 on the same line, which reads as an earlier
answer changing. The walkthroughs never work that way. Each of their 59 recorded
turns asks one new question, and when evidence changes they build an `-updated`
session from the changed one instead of asking old questions again; they needed
`--drop` 14 times to stop sessions from asking again. A Nouli session is a
sequential chain: each answer is decided once, stays as it was, and later turns
build on it as history. This record supersedes ADR 0002's standing questions and
the REPL behavior built on them.

## Decision

- **A turn asks exactly the questions it is given.** Nothing a session asked
  before is asked again; earlier answers reach the model only as history.
- **One question per call from the CLI and the REPL.** A `?` line and a
  command's `-q` each give one question, sent in its own call, and a second
  `-q` is a usage error. Request files' `turn.questions` and workflow nodes,
  which agents and scripts write, can still list several independent questions
  for one call, as [ADR 0001](0001-decision-session-harness.md) decided.
- **No standing questions.** Sessions keep facts and history, not questions,
  and branches and blends inherit none. `--drop`, `/drop`, `nouli questions`,
  `/questions`, and `/ask` are removed. A turn with no question fails with
  `questions_required`, so facts wait for a question.
- **Asking again is explicit.** A new judgment on the same matter is a new
  question in a new turn, with the same ID or a new one. The earlier answer is
  unchanged; text output marks how the new answer compares with the last one
  under the same ID.
- **What-ifs ask their own question.** `nouli whatif SESSION [FACT...] -q Q`
  and `/whatif ?id: prompt` ask with the given facts, or the REPL's queue, and
  save nothing. The REPL keeps its queue.
- **An empty REPL line** sends again the queue a failed turn left, with its
  question, and otherwise asks nothing.
- **0.2.0 data still reads.** `StandingChanges` and the `standing` fields stay
  on requests, attempts, and turns, because their types reject unknown fields
  and 0.2.0 stores hold them. Nouli never applies them, and a new request that
  sets `standing` fails with `invalid_standing_question`.

## Considered options

| Alternative | Reason for the chosen direction |
| --- | --- |
| Keep standing questions and explain re-asking better | The behavior is the problem: asking again re-decides answers the chain depends on, and puts old and new questions in one call. |
| Keep standing questions as an opt-in library feature | It is the same behavior through another door, and keeps the ledger fold, branch inheritance, and replay checks for a mode the CLI no longer uses. |
| Remove the `standing` fields from the data model | Saved 0.2.0 turns, frozen requests, and attempts carry them, so old stores would stop reading. |
| Let `-q` repeat, so a command's questions share one call | Questions listed together would be decided together, while a person reading the session expects each answer to come from its own call, as each REPL line does. |
| One question per call for request files and workflow nodes too | They are the agent contract from [ADR 0001](0001-decision-session-harness.md), where a node's independent questions share one context and one call. Changing that is a contract change outside this record. |

## Consequences

- A saved answer is never re-decided. Later turns condition on it as history,
  and a changed fact affects only the questions asked after it.
- Every turn names its question. Facts alone no longer run a turn, and the REPL
  holds them in the queue until a `?` line sends them.
- The README, guide, home page, and walkthroughs are rewritten for this, and
  the home page's interactive session is recorded again. The home-page figure
  is recorded again too: its second travel turn asked two questions, and is now
  two turns.
- A 0.2.0 request that set standing questions can't be replayed or retried by
  its request ID, because its questions lived in `standing`. Ask them in a new
  turn.

The [glossary](../GLOSSARY.md) defines decision questions and facts.
