# Command line

Nouli's commands are verbs. A session keeps its facts and every answer, so each turn only needs what is new: its facts and the questions it asks. A question is asked once, and later turns never ask it again. Bare invocation prints help, or opens the interactive session loop when both input and output are terminals.

```sh
nouli new Trip "The train costs 40 USD and arrives 30 minutes before the meeting." \
  -q "transport: How should I get to the meeting? [train, taxi]"
nouli add Trip "The train has been cancelled. A taxi costs 70 USD." \
  -q "rebook: How should I get to the meeting now? [train, taxi]"
nouli ask Trip -q "on_time: Will I get there on time?"
nouli whatif Trip "The taxi fare has doubled to 140 USD." -q "fare: Is the taxi still worth it?"
```

| Command | What it does |
| --- | --- |
| `nouli new NAME [--from SRC[@TURN]]... [FACT...] [-q Q...]` | Creates a session: new with no `--from`, a branch with one, a blend with several. With no facts and no questions it only creates the session and prints its ID, and `--request-id` and `--retry-interrupted` are rejected because no turn is saved. Otherwise it runs the first turn. |
| `nouli add SESSION [FACT...] -q Q... [--replace N TEXT...]` | Runs a turn on an existing session. |
| `nouli ask SESSION -q Q...` | Asks without new facts. It is `add` with no facts. |
| `nouli whatif SESSION [FACT...] -q Q... [--replace N TEXT...]` | Runs one turn from the session's head and saves nothing. |

`SESSION` is an existing ID or unique case-sensitive name; an unknown selector never creates a session. `--from` names a source session, or `SESSION@TURN_ID` for a fixed finalized turn. Without `@`, acceptance captures the source's current frontier once, so later source changes do not flow into the saved branch or blend. A name belongs to one session.

## Facts

Every positional `FACT` is saved as an evidence record in the session. The model sees the current text of every fact, and a turn does not repeat the facts of earlier turns. Facts are numbered in the order they were first added, and a blend or a branch numbers the facts it inherits first. `--replace N TEXT` supersedes fact N: the new text keeps number N, the model sees only the new text from then on, and the earlier version stays in the ledger and in `nouli context`. `--replace` is repeatable, applies before new facts, and rejects a number that does not exist. When a blend joins branches that replaced the same fact differently, the fact shows every alternative, the model sees them all, and `--replace N` settles it. Empty facts are rejected. A fact that begins with `-` needs `--` before it.

## Questions

`-q` takes one question per option in a one-line grammar. The same grammar is used by workflow files and the REPL.

```text
transport: How should I get to the meeting? [train, taxi]   choice, 2 to 255 options
fit: How well does this plan fit my priorities? <poor, fair, good, excellent>   score, 2 to 10 ordered levels
on_time: Will the train get me there on time?               yes/no
```

An ID is lowercase letters, digits, `_` or `-`. Items are comma-separated, and an item that contains a comma, bracket, angle bracket, or quote is written in double quotes. A choice option's ID is derived from its label (`"Book the train"` becomes `book_the_train`). A grammar error is an input error (exit code 2) that names the question's position and the problem, never its text.

A turn asks exactly its `-q` questions, in one call, and nothing else. Sessions don't keep questions: a later turn, a branch, or a blend never asks an earlier question again, so every saved answer stays as it was decided, and later turns see earlier questions and answers as history. To ask about the same thing after new facts, ask again, with the same ID or a new one; the new answer is a separate decision in a new turn. A turn with no `-q` question fails with `questions_required` (exit code 2) and sends nothing.

## Request files

`--input FILE` replaces facts and questions with a complete request, for agents and scripts: a portable `TurnRequest` (`version`, `state`, `questions`), or an `EvaluationRequest` with `turn`, `updates`, `required_records`, `model`, and `request_id`. The command supplies the session, so a request file that names one is rejected. `--input` cannot be combined with facts, `-q`, or `--replace`. A file's `turn.questions` are the questions the turn asks. A `standing` field, which Nouli 0.2.0 used for questions a session kept asking, fails with `invalid_standing_question`. Omitted turn versions default to v1. Unknown fields and duplicate keys fail. JSON and TOON share one model; files use `.json` or `.toon`, and stdin (`--input -`) requires `--input-format json|toon`. Input is bounded to 16 MiB.

```sh
nouli add Trip --input request.json
nouli new Branch --from Trip@019a0000-0000-7000-8000-000000000001 --input request.toon
nouli whatif Trip --input - --input-format json < request.json
```

## Interactive sessions

`nouli open SESSION` opens a session in a prompt, creating it if it does not exist and you agree. A bare `nouli` in a terminal first lists your sessions, with a number for each; type a number, or a new name. The loop is a thin layer over the same operations as the verbs, and shows only what the ledger holds. It uses `--provider`, `--model`, and `--context-budget` like the verbs. Without a terminal, `nouli open SESSION` reads its lines from standard input and echoes them, so a script reads as a transcript (it creates a missing session without asking), and a bare `nouli` prints help.

```text
$ nouli open trip
trip · /help for commands

trip › The train has been cancelled.
trip › ?rebook: How should I get to the meeting now? [Take the train, Take a taxi]
  turn 2 · 1 fact · 1 question
  rebook        Take the train                   ▊                    0.04
              ▶ Take a taxi                      ███████████████████▎ 0.96
```

| Input | What it does |
| --- | --- |
| plain text | Queues a fact for the next question. |
| `?id: prompt [a, b]` | Sends a turn now: the queue and this question, in one call. Only this question is asked. A grammar mistake is reported and nothing is sent. |
| an empty line | Sends the queue again when a failed turn left it, with its question. Otherwise it asks nothing, and says so when facts are waiting. |
| `/whatif ?id: prompt [a, b]` | An unsaved turn with the queue and this question. The queue stays for the next turn. |
| `/replace N TEXT` | Queues a fact replacement. |
| `/clear` | Empties the queue. |
| `/facts` | Shows the session's facts, then what is queued. |
| `/new NAME`, `/use SESSION` | Start a session, or switch to one. |
| `/branch NAME` | A new session from the current head. |
| `/blend A B [...] as NAME` | A new session from several. |
| `/view` | The sessions, with turn counts and where each came from; `●` marks the current one. |
| `/context` | Which records the model saw on the last turn. |
| `/help`, `/quit`, `/exit` | The command list, and leaving. |

A turn that fails prints its error and keeps the queue. Ctrl-C clears a typed line, then the queue, and on an empty prompt with nothing queued it quits; during a turn it cancels the turn through the same cancellation the verbs use. Ctrl-D quits, warning first when the queue is not empty. History is kept in `repl_history` inside the store, readable only by you.

## Output

In a terminal, a turn is printed as bars: every option's probability, `▶` on the selected one, and the turn's header. A question asked again under an ID used before is marked where its new answer differs by at least 0.05 from the last answer under that ID (`→ was`); the earlier answer stays as it was.

```text
  turn 3 · 1 fact (1 replaced, kept in history) · 1 question
  on_time       yes                              ██████████████████▎  0.91
```

A turn that was not saved starts with `(not saved)`. Color is used only in a terminal, with `NO_COLOR` unset or empty and `TERM` not `dumb`. Native confidence is in `--json` and TOON output. `--json`, `--jsonl`, and TOON output are unchanged apart from the questions a turn asked now being listed in full. A session created without a turn prints its ID (`Session: ...`, or a `session` report in JSON). Output controls and exit codes are in [output.md](output.md).

## Providers, identity, and recovery

Persisted turns use the user ledger by default. A `whatif` opens an existing store read-only. The first valid turn scaffolds a missing default config; help and inspection create no files. `HOME` supplies default config and store paths; fully explicit paths also work without `HOME`.

Credentials come from the current directory's `.env` or an explicit `--env-file FILE`; exported shell values take precedence. Missing automatic files are fine; malformed files and missing explicit files fail before execution. See [configuration](configuration.md).

`--provider` and `--model` select a named provider and a model independently, and `--context-budget TOKENS` overrides the budget within known limits. Resolution is the command line, the request file, the session's defaults, then configuration defaults. Accepted work freezes its settings and sources, records the actual served provider and model, and never falls back to another one.

`--request-id KEY` (on `new`, `add`, and `ask`) identifies the normalized intent globally. A completed request replays the saved turn, and a conflicting intent fails. The record IDs of facts given on the command line are derived from the key, so the same command replays. `--replace` numbers refer to the facts as they are when the command runs; a repeated command with a request ID replays the replacements it saved, and a different replacement under the same ID conflicts. `--retry-interrupted` requires a request ID and explicitly retries eligible interrupted work with its saved settings. Ctrl-C cancels the shared engine and awaits its cleanup. A `whatif` has no request identity.

## Library

The same operations are available to embedding programs: see [embedding](embedding.md) for `context::facts` and `Ledger::facts`.
