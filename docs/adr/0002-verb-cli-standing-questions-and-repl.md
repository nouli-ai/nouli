---
status: accepted
date: 2026-10-29
---

# Use a verb CLI with standing questions, numbered facts, and a REPL

Nouli 0.1.0 evaluates at the root command: every turn names its session with
flags (`--name`, `--session`, `--from`, `--no-session`), supplies facts as
free-text turn state, and passes its questions as hand-written JSON. That suits
agents and scripts, but people find it awkward. In a multi-turn decision the
questions rarely change while the facts do, yet every turn repeats every
question, and a mistaken fact can't be corrected later. ADR 0001 planned a REPL
for the following release; this record decides the human-facing surface it and
the CLI share. It supersedes the CLI behavior decision in
[ADR 0001](0001-decision-session-harness.md). 0.1.0 has no users to migrate,
so the change is breaking.

## Decision

- **Verbs replace root evaluation.** The CLI's commands are:
  - `new NAME [--from SRC[@TURN]]... [FACT...] [-q Q...]` creates a session:
    new with no `--from`, a branch with one, and a blend with several. With
    facts or questions it also runs the first turn.
  - `add SESSION [FACT...] [-q Q...] [--drop ID...] [--replace N TEXT...]` runs
    a turn on an existing session.
  - `ask SESSION [-q Q...] [--drop ID...]` re-asks without new facts.
  - `whatif SESSION [FACT...] [-q Q...] [--replace N TEXT...]` runs one
    ephemeral turn from the session's head and saves nothing.
  - `facts SESSION` and `questions SESSION` show the session's numbered facts
    and standing questions.
  - `view SESSION [--mermaid]`, `context SESSION [--turn ID]`, and
    `sessions list|show|export` inspect history.
  - `workflow run|show|resume` runs workflows.

  A bare `nouli` in a terminal opens the REPL, and `nouli open SESSION` opens
  it on a session. Removed: root evaluation, `-s/--state`, `--name`,
  `--session`, root `--from`, `--no-session`, JSON after `-q`, and the
  `workflows` plural. [ADR 0004](0004-ask-each-question-once.md) removes
  `--drop` and `questions`, and makes `-q` take one question.
- **Agents keep full access.** Turn verbs accept `--input FILE|-`: an
  `EvaluationRequest` without `session`, in JSON or TOON, which the verb
  supplies. Output keeps `--json`, `--jsonl`, and TOON, and request IDs,
  interrupted retry, and exit codes are unchanged.
- **Compact question grammar.** `id: prompt` is a yes/no (noul) question;
  `id: prompt [a, b, …]` is a choice; `id: prompt <a, b, …>` is a score whose
  levels are in order. Items are comma-separated strings, bare or
  double-quoted. Option IDs are derived from labels: lowercase, with each run
  of non-alphanumeric characters becoming `_`. The CLI's `-q`, the REPL, and
  workflow files share the grammar, which lives in `nouli-core`.
- **Standing questions.** A session stores the questions it keeps asking.
  `add` and the REPL re-ask them; `-q` adds a standing question or replaces one
  with the same ID; `--drop` removes one. Changes are saved with the turn that
  makes them, so a failed turn leaves the set unchanged. A new session starts
  with none, a branch inherits its source's set as it was at the captured
  point, and a blend starts with none.
  Superseded by [ADR 0004](0004-ask-each-question-once.md), under which a turn
  asks only its own questions and nothing is asked again.
- **Facts.** Each fact is an evidence context record with key
  `fact:<record id>`, shown numbered in context order. `--replace N`
  supersedes fact N with a record that keeps the same key, so the ledger keeps
  the earlier version, as described in the
  [context revision contract](../contracts/context-revisions.md). Distinct
  facts never share a key. Blended branches that replaced the same fact leave
  alternatives under one key; they are listed as one numbered fact, and
  `--replace N` supersedes all of them.
- **REPL.** Plain lines queue facts. `?id: prompt …` adds a standing question
  and sends the queue with it as one turn, and an empty line sends the queue
  or asks the standing questions again. ADR 0004 replaces these, and removes
  `/drop`, `/questions`, and `/ask`. Slash commands map
  to the verbs: `/whatif`, `/branch`, `/blend`, `/use`, `/facts`,
  `/questions`, `/drop`, `/replace`, `/ask`, `/view`, `/context`, `/help`, and
  `/quit` or `/exit`. The REPL is a thin loop over the same operations as the CLI and
  never shows anything that isn't in the ledger. An optional drafting mode, in
  which a text model turns plain English into facts and questions for the
  person to approve, is deferred.

## Considered options

| Alternative | Reason for the chosen direction |
| --- | --- |
| Keep root evaluation and add verbs alongside it | Two ways to do every operation, and the flag combinations stay easy to misuse. With no users yet, a clean break costs nothing. |
| JSON questions only, with a template file per session | Still asks people to hand-write JSON. A one-line grammar is enough for all three answer types. |
| Re-ask questions only when the caller lists them | Every turn repeats every question, which is the friction this change removes. Standing questions keep one-off questions possible. |
| Keep facts as free-text turn state | Turn state can't be corrected after the turn. Keyed evidence records already support supersession with preserved history. |
| A full-screen terminal UI first | More work, and harder to use over SSH or in scripts. A line-based REPL covers the main loop and leaves room for richer views later. |

## Consequences

- The CLI's documentation, the website, and the walkthroughs must be rewritten
  for the verbs, and the walkthrough outputs regenerated.
- The ledger gains a log of standing-question changes and a schema migration.
  Branch inheritance has to replay that log against the captured frontier.
- Facts entered through the CLI and REPL become evidence records, not turn
  state. Model context already includes active caller records, but how they
  read to a model should be checked on real providers.
- The library's evaluation request gains standing-question changes, so library
  callers can use standing questions without the CLI.

The [glossary](../GLOSSARY.md) defines standing questions and facts.
