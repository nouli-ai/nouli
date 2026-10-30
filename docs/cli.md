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
| `nouli new NAME [--from SRC[@TURN]]... [FACT...] [-q Q]` | Creates a session: new with no `--from`, a branch with one, a blend with several. With no facts and no question it only creates the session and prints its ID, and `--request-id` and `--retry-interrupted` are rejected because no turn is saved. Otherwise it runs the first turn. |
| `nouli add SESSION [FACT...] -q Q [--replace N TEXT...]` | Runs a turn on an existing session. |
| `nouli ask SESSION -q Q` | Asks without new facts. It is `add` with no facts. |
| `nouli whatif SESSION [FACT...] -q Q [--replace N TEXT...]` | Runs one turn from the session's head and saves nothing. |

`SESSION` is an existing ID or unique case-sensitive name; an unknown selector never creates a session. `--from` names a source session, or `SESSION@TURN_ID` for a fixed finalized turn. Without `@`, acceptance captures the source's current frontier once, so later source changes do not flow into the saved branch or blend. A name belongs to one session.

## Facts

Every positional `FACT` is saved as an evidence record in the session. The model sees the current text of every fact, and a turn does not repeat the facts of earlier turns. Facts are numbered in the order they were first added, and a blend or a branch numbers the facts it inherits first. `--replace N TEXT` supersedes fact N: the new text keeps number N, the model sees only the new text from then on, and the earlier version stays in the ledger and in `nouli context`. `--replace` is repeatable, applies before new facts, and rejects a number that does not exist. When a blend joins branches that replaced the same fact differently, the fact shows every alternative, the model sees them all, and `--replace N` settles it. Empty facts are rejected. A fact that begins with `-` needs `--` before it.

## Questions

`-q` takes one question in a one-line grammar. The same grammar is used by workflow files and the REPL.

```text
transport: How should I get to the meeting? [train, taxi]   choice, 2 to 255 options
fit: How well does this plan fit my priorities? <poor, fair, good, excellent>   score, 2 to 10 ordered levels
on_time: Will the train get me there on time?               yes/no
```

An ID is lowercase letters, digits, `_` or `-`. Items are comma-separated, and an item that contains a comma, bracket, angle bracket, or quote is written in double quotes. A choice option's ID is derived from its label (`"Book the train"` becomes `book_the_train`). A grammar error is an input error (exit code 2) that names the problem, never the question's text.

A turn asks exactly its `-q` question, in its own call, and nothing else. A command takes one `-q`; a second is a usage error (exit code 2), so to ask two things, run two turns. Sessions don't keep questions: a later turn, a branch, or a blend never asks an earlier question again, so every saved answer stays as it was decided, and later turns see earlier questions and answers as history. To ask about the same thing after new facts, ask again, with the same ID or a new one; the new answer is a separate decision in a new turn. A turn with no `-q` question fails with `questions_required` (exit code 2) and sends nothing.

## Request files

`--input FILE` replaces facts and questions with a complete request, for agents and scripts: a portable `TurnRequest` (`version`, `state`, `questions`), or an `EvaluationRequest` with `turn`, `updates`, `required_records`, `model`, and `request_id`. The command supplies the session, so a request file that names one is rejected. `--input` cannot be combined with facts, `-q`, or `--replace`. A file's `turn.questions` are the questions the turn asks, and may list several independent questions for one call. A `standing` field, which Nouli 0.2.0 used for questions a session kept asking, fails with `invalid_standing_question`. Omitted turn versions default to v1. Unknown fields and duplicate keys fail. JSON and TOON share one model; files use `.json` or `.toon`, and stdin (`--input -`) requires `--input-format json|toon`. Input is bounded to 16 MiB.

```sh
nouli add Trip --input request.json
nouli new Branch --from Trip@019a0000-0000-7000-8000-000000000001 --input request.toon
nouli whatif Trip --input - --input-format json < request.json
```

## Interactive sessions

`nouli open SESSION` opens a session, full screen in a terminal, creating it if it does not exist. A bare `nouli` in a terminal starts a new session, known by its short ID; in a terminal that takes no escape codes it prints help. The loop is a thin layer over the same operations as the verbs, and shows only what the ledger holds. It uses `--provider`, `--model`, and `--context-budget` like the verbs. Without a terminal, `nouli open SESSION` reads its lines from standard input and echoes them, so a script reads as a transcript (it creates a missing session without asking), and a bare `nouli` prints help.

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
| `/replace N TEXT` | Queues a fact replacement. |
| `/clear` | Empties the queue. |
| `/facts` | Shows the session's facts, then what is queued. |
| `/new NAME`, `/use SESSION` | Start a session, or switch to one. |
| `/branch NAME` | A new session from the current head. |
| `/blend A B [...] as NAME` | A new session from several. |
| `/view` | The sessions, with turn counts and where each came from; `●` marks the current one. |
| `/context` | Which records the model saw on the last turn. |
| `/help`, `/quit`, `/exit` | The command list, and leaving. |

A turn that fails prints its error and keeps the queue.

### Full-screen view

In a terminal, the form asks for ordinary text:

- **Context** holds the facts for the next question. Enter sends when a question is ready, including a reused question. Before a question is ready, Enter queues the context.
- **Question** holds the question exactly as you write it. There is no ID, colon, bracket, quoting, or option-label syntax to remember.
- Choose **Yes / No**, **Choose one**, or **Score** with Shift-Tab from any field. Choices show Options; scores show Levels, lowest first. Type one per row or paste a list to fill several at once. Each choice needs at least two options; each score needs at least two levels.

Press **Enter** to send from Question or Context when a question is ready. In Options/Levels, Enter adds the next row; press Enter again on the empty row to send. Typing or pasting another item continues the list. Navigating away cancels the pending second Enter. **Ctrl-S / F5** still sends directly from any field. Tab moves between fields and list items without changing the answer type. A saved turn clears the editable fields and keeps the selected answer type. Failure or cancellation preserves input for correction or retry. IDs are generated from the question and answer space, so punctuation and labels such as `Rail!`, `rail`, or non-Latin names do not collide.

After a choice or score, leave Options or Levels empty to reuse the latest matching question's exact answer list and criteria. The Options/Levels heading says **using previous N**. Entering any new list replaces the old one; a partial list must still meet the minimum of two items. Leave Question empty too to reuse the preceding prompt, add new context, and press Enter to get an updated judgment. Reusing a list keeps the question ID so results can show how the answer changed; each Send saves a new turn and preserves earlier answers. Yes/No has no fallback. Reuse is scoped to the current session's latest single question, and an empty new session or branch starts without a prior list. The question grammar for CLI verbs and piped input is unchanged.

The Citrus palette gives Yes/No a lime accent, Choose one cyan, and Score rose. Results and graph cards keep their own question type's color. Labels, brackets, and emphasis still identify types with `NO_COLOR` set.

A persistent status line shows the provider and model selected for the next request, plus Ready, Draft, Asking, or Cancelling. It is visible before the first answer and refreshes when you switch sessions. While a request runs, it shows the model resolved for that request. After a response, it shows the actual served model version in place of its alias. The bottom key hints follow the focused field; errors remain visible while your input stays available for correction or retry.

The top line shows `nouli`, its version, and the session. Mouse capture is disabled so you can drag to select and copy terminal text normally. Use the keyboard to move between fields. Shift-Tab is the only answer-type shortcut and cycles left to right: Yes/No → Choose one → Score → Yes/No. The selector shows a short `Shift-Tab →` hint. It works from any field, including while the graph or a command/session list has focus; it returns focus to the form. Tab moves from Context to Question, then to Options/Levels (or back to Context for Yes/No). In Options/Levels, Tab moves to the next item, creating an item after the last filled one; Tab on an empty trailing item returns to Context. Use Tab or arrows to move from the question to its options. The decisions appear to the right at 116 columns or wider; Ctrl-G opens them on narrower terminals. The graph keeps sessions opened in the current TUI run, including after `/new`; independent sessions appear as independent roots and shared ancestors appear once.

| Key or action | What it does |
| --- | --- |
| Mouse drag | Selects text using the terminal. |
| Shift-Tab | Cycles answer types left to right from any field. |
| Tab in Question | Moves to Options/Levels, or Context for Yes/No. |
| Tab in Context | Moves to Question. |
| Tab in Options/Levels | Moves to the next item, adds one after the last filled item, or leaves an empty trailing item without sending. |
| Up, Down | Moves between rows. |
| Ctrl-S / F5 | Sends the question and queued context. |
| Enter | Sends from Question/Context when ready; queues initial context or runs a slash command. |
| Enter in Options/Levels | Adds a row; a second consecutive Enter sends. Typing, pasting, or navigation cancels the second-Enter state. |
| Drop files | Inserts their contents into the focused field without sending. |
| `/` in Context | Lists optional session commands; pasted/imported files are literal evidence. |
| `/sessions` | Lists sessions to resume. |
| PgUp, PgDn | Scrolls the transcript. |
| Ctrl-G | Focuses the decisions pane; Esc returns to the form. |
| Ctrl-C | Cancels a turn, or clears the row, then the form, then quits. |
| Ctrl-D | Deletes forward, or quits with a warning for unsent input. |

Focus the destination with arrows or Tab, then drag files from Finder or another file manager into the terminal. Most terminals paste shell-escaped paths; quoted paths, spaces, Unicode, `file://` URLs, and multiple files work. UTF-8 files insert their actual text. PDF files insert extracted text and require Poppler's `pdftotext` on PATH (`brew install poppler` on macOS). Scanned PDFs with no text need OCR or pages exported as images. Other binary formats report an error and leave the form available.

PNG, JPEG, GIF, WebP, BMP, TIFF, and ICO images appear as readable references in the focused context, question, option, or score level. Animated images use their first frame. The image bytes are saved as session evidence and sent through the provider's image format, so later turns do not depend on the original file path. Providers that only support images in shared state receive labelled images there. Choose a model with image support; provider limits still apply. For Jev, Nouli sends an upright still image scaled or padded to its accepted 280–1568 pixel dimensions, while saving the original. Image context uses a separate pixel-based estimate for Jev and direct OpenAI; repeated image references count separately. These estimates are approximate. Other routed models retain their encoded-byte capacity estimate where required by the provider. Each file is capped at 16 MiB.

Pasting never submits. Context and question fields preserve paragraphs and indentation; Options split pasted lines into individual items. The alternate-screen transcript disappears when the view closes, while the session retains its saved turns. Scripts and `TERM=dumb` keep the line syntax documented above.

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
