# Command line

Nouli's commands are verbs. A session keeps its facts and the questions it keeps asking, so each turn only needs what is new. Bare invocation prints help, or opens the interactive session loop when both input and output are terminals.

```sh
nouli new Trip "The train costs 40 USD and arrives 30 minutes before the meeting." \
  -q "transport: How should I get to the meeting? [train, taxi]" \
  -q "on_time: Will the train get me there on time?"
nouli add Trip "The train has been cancelled. A taxi costs 70 USD."
nouli ask Trip --drop on_time
nouli whatif Trip "The taxi fare has doubled to 140 USD."
```

| Command | What it does |
| --- | --- |
| `nouli new NAME [--from SRC[@TURN]]... [FACT...] [-q Q...]` | Creates a session: new with no `--from`, a branch with one, a blend with several. With no facts and no questions it only creates the session and prints its ID, and `--request-id` and `--retry-interrupted` are rejected because no turn is saved. Otherwise it runs the first turn, and its `-q` questions become standing questions. |
| `nouli add SESSION [FACT...] [-q Q...] [--drop ID...] [--replace N TEXT...]` | Runs a turn on an existing session. |
| `nouli ask SESSION [-q Q...] [--drop ID...]` | Asks again without new facts. It is `add` with no facts. |
| `nouli whatif SESSION [FACT...] [-q Q...] [--drop ID...] [--replace N TEXT...]` | Runs one turn from the session's head and saves nothing. |

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

A question given with `new`, `add`, or `ask` is a standing question: later turns ask it again until `--drop ID` removes it, and `-q` with an existing ID replaces it in place. Standing questions are saved with the turn that changed them, so a turn that fails leaves them as they were. A new session starts with none, a branch inherits its source's questions as they were at the branch point, and a blend starts with none. In a `whatif`, `-q` adds or changes a question and `--drop` leaves one out, for that turn only. Dropping an unknown ID is an error. A turn with no questions at all, from the session or from `-q`, fails with `questions_required`.

## Request files

`--input FILE` replaces facts and questions with a complete request, for agents and scripts: a portable `TurnRequest` (`version`, `state`, `questions`), or an `EvaluationRequest` with `turn`, `standing` (`set` and `drop`), `updates`, `required_records`, `model`, and `request_id`. The command supplies the session, so a request file that names one is rejected. `--input` cannot be combined with facts, `-q`, `--drop`, or `--replace`. A file's `turn.questions` are one-off questions asked once on top of the standing set. Omitted turn versions default to v1. Unknown fields and duplicate keys fail. JSON and TOON share one model; files use `.json` or `.toon`, and stdin (`--input -`) requires `--input-format json|toon`. Input is bounded to 16 MiB.

```sh
nouli add Trip --input request.json
nouli new Branch --from Trip@019a0000-0000-7000-8000-000000000001 --input request.toon
nouli whatif Trip --input - --input-format json < request.json
```

## Interactive sessions

`nouli open SESSION` opens a session in a prompt, creating it if it does not exist and you agree. A bare `nouli` in a terminal first lists your sessions, with a number for each; type a number, or a new name. The loop is a thin layer over the same operations as the verbs, and shows only what the ledger holds. It uses `--provider`, `--model`, and `--context-budget` like the verbs. Without a terminal, `nouli open SESSION` reads its lines from standard input and echoes them, so a script reads as a transcript (it creates a missing session without asking), and a bare `nouli` prints help.

```text
$ nouli open travel
travel · 2 standing questions · /help for commands
travel › The train has been cancelled. The taxi is still available.
travel › ↵
  turn 2 · 1 fact · 2 questions · re-asked 2
  transport   ▶ taxi                             ███████████████████▊ 0.99   → was train 1.00
                train                            ▏                    0.01
  on_time       yes                              ██▋                  0.13   → was 0.98
```

| Input | What it does |
| --- | --- |
| plain text | Queues a fact. |
| `?id: prompt [a, b]` | Queues a standing question in the one-line grammar. A mistake is reported and nothing is queued. |
| an empty line | Sends the queue as one turn, as `add` does, or asks the standing questions again if nothing is queued. |
| `/ask` | Sends now. |
| `/whatif FACT` | An unsaved turn with the queue plus FACT. The queue stays for the next turn. |
| `/replace N TEXT`, `/drop ID` | Queue a fact replacement or the end of a standing question. |
| `/clear` | Empties the queue. |
| `/facts`, `/questions` | Show the session's facts and standing questions, then what is queued. |
| `/new NAME`, `/use SESSION` | Start a session, or switch to one. |
| `/branch NAME` | A new session from the current head. |
| `/blend A B [...] as NAME` | A new session from several. |
| `/view` | The sessions, with turn counts and where each came from; `●` marks the current one. |
| `/context` | Which records the model saw on the last turn. |
| `/help`, `/quit` | The command list, and leaving. |

A turn that fails prints its error and keeps the queue. Ctrl-C clears the queue; during a turn it cancels the turn through the same cancellation the verbs use. Ctrl-D quits, warning first when the queue is not empty. History is kept in `repl_history` inside the store, readable only by you.

## Output

In a terminal, a turn is printed as bars: every option's probability, `▶` on the selected one, a marker where an answer moved by at least 0.05 since the last answer to the same question, and the turn's header.

```text
  turn 3 · 1 fact · 2 questions · re-asked 2
  transport     train                            ▎                    0.01
              ▶ taxi                             ███████████████████▊ 0.99   → was train 1.00
  on_time       yes                              ██▋                  0.13   → was 0.98
```

A turn that was not saved starts with `(not saved)`. Color is used only in a terminal, with `NO_COLOR` unset or empty and `TERM` not `dumb`. Native confidence is in `--json` and TOON output. `--json`, `--jsonl`, and TOON output are unchanged apart from the questions a turn asked now being listed in full. A session created without a turn prints its ID (`Session: ...`, or a `session` report in JSON). Output controls and exit codes are in [output.md](output.md).

## Providers, identity, and recovery

Persisted turns use the user ledger by default. A `whatif` opens an existing store read-only. The first valid turn scaffolds a missing default config; help and inspection create no files. `HOME` supplies default config and store paths; fully explicit paths also work without `HOME`.

Credentials come from the current directory's `.env` or an explicit `--env-file FILE`; exported shell values take precedence. Missing automatic files are fine; malformed files and missing explicit files fail before execution. See [configuration](configuration.md).

`--provider` and `--model` select a named provider and a model independently, and `--context-budget TOKENS` overrides the budget within known limits. Resolution is the command line, the request file, the session's defaults, then configuration defaults. Accepted work freezes its settings and sources, records the actual served provider and model, and never falls back to another one.

`--request-id KEY` (on `new`, `add`, and `ask`) identifies the normalized intent globally. A completed request replays the saved turn, and a conflicting intent fails. The record IDs of facts given on the command line are derived from the key, so the same command replays. `--replace` numbers refer to the facts as they are when the command runs; a repeated command with a request ID replays the replacements it saved, and a different replacement under the same ID conflicts. `--retry-interrupted` requires a request ID and explicitly retries eligible interrupted work with its saved settings. Ctrl-C cancels the shared engine and awaits its cleanup. A `whatif` has no request identity.

## Library

The same operations are available to embedding programs: see [embedding](embedding.md) for `EvaluationRequest.standing`, `context::facts`, and `Ledger::standing_questions` and `Ledger::facts`.
