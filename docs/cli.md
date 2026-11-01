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

New sessions created without an explicit name get a persistent random name such as `curie-7-diffraction`: a short lowercase scientist surname, a number from 1–20, and a noun from an original spinner-style verb. Names are unique within the ledger and work with `nouli open NAME` or `/use NAME`. Bare `nouli` and `/new` choose a name automatically; `/new NAME` uses your supplied name. Existing sessions keep their names or legacy ID labels. If all 19,840 generated combinations are occupied, supply an explicit name.

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

`nouli open SESSION` opens a session, full screen in a terminal, creating it if it does not exist. A bare `nouli` in a terminal starts a new session with an automatically assigned name; in a terminal that takes no escape codes it prints help. The loop is a thin layer over the same operations as the verbs, and shows only what the ledger holds. It uses `--provider`, `--model`, and `--context-budget` like the verbs. Without a terminal, `nouli open SESSION` reads its lines from standard input and echoes them, so a script reads as a transcript (it creates a missing session without asking), and a bare `nouli` prints help.

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
| `/new [NAME]`, `/use SESSION` | Start a session with an optional name, or switch to one. |
| `/rename NAME` | Rename the current session; keep its ID, history and queued draft. |
| `/branch NAME` | A new session from the current head. |
| `/blend A B [...] as NAME` | A new session from several. |
| `/view` | The sessions, with turn counts and where each came from; `●` marks the current one. |
| `/context` | Which records the model saw on the last turn. |
| `/help`, `/quit`, `/exit` | The command list, and leaving. |

A turn that fails prints its error and keeps the queue.

### Full-screen view

In a terminal, the form asks for ordinary text:

- **Context** holds the facts for the next question. Enter sends when a question is ready, including a reused question. Before a question is ready, Enter queues the context.

When saved history is available, Context shows **latest question included** and previews that question. The next TUI request requires its saved question and answer, its used image inputs, and your new/replacement evidence. This also applies to a fresh Yes/No follow-up. Including prior history does not reuse a Yes/No prompt. New independent sessions have no preceding question; branches can show inherited history. If required context cannot fit, the request fails explicitly and keeps your draft.
- **Question** holds the question exactly as you write it. There is no ID, colon, bracket, quoting, or option-label syntax to remember.
- Choose **Yes / No**, **Choose one**, or **Score** with Shift-Tab from any field. Choices show Options; scores show Levels, lowest first. Type one per row or paste a list to fill several at once. Each choice needs at least two options; each score needs at least two levels.

Press **Enter** to send from Question or Context when a question is ready. In Options/Levels, Enter adds the next row; press Enter again on the empty row to send. Typing or pasting another item continues the list. Navigating away cancels the pending second Enter. **Ctrl-S / F5** still sends directly from any field. Tab moves between fields and list items without changing the answer type. A saved turn clears the editable fields and keeps the selected answer type. Failure or cancellation preserves input for correction or retry. IDs are generated from the question and answer space, so punctuation and labels such as `Rail!`, `rail`, or non-Latin names do not collide.

After a choice or score, leave Options or Levels empty to reuse the latest matching question's exact answer list and criteria. The Options/Levels heading says **using previous N**. Entering any new list replaces the old one; a partial list must still meet the minimum of two items. Leave Question empty too to reuse the preceding prompt, add new context, and press Enter to get an updated judgment. Reusing a list keeps the question ID so results can show how the answer changed; each Send saves a new turn and preserves earlier answers. Yes/No has no fallback. Reuse is scoped to the current session's latest single question, and an empty new session or branch starts without a prior list. The question grammar for CLI verbs and piped input is unchanged.

The Citrus palette gives Yes/No a lime accent, Choose one cyan, and Score rose. Results and graph cards keep their own question type's color. Labels, brackets, and emphasis still identify types with `NO_COLOR` set.

A persistent status line shows the provider and model selected for the next request, plus Ready, Draft, Asking, or Cancelling. It is visible before the first answer and refreshes when you switch sessions. While a request runs, it shows the model resolved for that request. After a response, it shows the actual served model version in place of its alias. The bottom key hints follow the focused field; errors remain visible while your input stays available for correction or retry.

The top line shows `nouli`, its version, and the session. Drag normally to select text and use your terminal’s Copy command; no modifier or mode is required. **Ctrl-G** enables graph divider dragging and mouse scrolling; **Esc** returns to native text selection. To hold the view steady while a request runs, press **F6** (or click **select text** while the graph is focused), then drag and copy. The visible screen stays still while requests continue in the background. **Esc** or **F6** resumes the previous draft and focus; resizing also resumes. While F6 holds the view, typing and paste are ignored, and Ctrl-C leaves that mode without clearing an idle draft or cancels an active request. The terminal’s selection modifier (Option-drag in iTerm2; Shift or Alt in other terminals) can also select text while graph mouse controls are active. Use the keyboard to move between fields. Shift-Tab is the only answer-type shortcut and cycles left to right: Yes/No → Choose one → Score → Yes/No. The selector shows a short `Shift-Tab →` hint. It works from any field, including while the graph or a command/session list has focus; it returns focus to the form. Tab moves from Context to Question, then to Options/Levels (or back to Context for Yes/No). In Options/Levels, Tab moves to the next item, creating an item after the last filled one; Tab on an empty trailing item returns to Context. Use Tab or arrows to move from the question to its options. The decisions appear to the right at 116 columns or wider; Ctrl-G opens them on narrower terminals. With Ctrl-G graph focus active, drag the divider to resize both panes. Ctrl-G focuses the graph for keyboard navigation, and Esc returns to the form. The width stays through session changes and adapts to terminal size. The graph keeps sessions opened in the current TUI run, including after `/new`; independent sessions appear as independent roots and shared ancestors appear once.

References connect the exact saved source question to the question using it, including across sessions and after reopening. Referenced source history loads automatically, up to the selected saved turn; later source answers are excluded. Multi-question turns separate into question cards at reference endpoints. `nouli view SESSION` includes the same dependencies; `--mermaid` draws reference edges dashed. These graph links do not change session ancestry, active facts or structured exports.

| Key or action | What it does |
| --- | --- |
| Mouse drag | Selects text normally; resizes the divider while Ctrl-G graph focus is active. |
| F6 / hold view | Holds the view steady for copying during a request; Select text is clickable in graph focus. Esc or F6 resumes; the draft, focus, and graph width stay intact. |
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
| Ctrl-G | Focuses decisions for divider dragging and keyboard/mouse scrolling; Esc returns to native text selection. |
| Ctrl-C | Cancels a turn, or clears the row, then the form, then quits. |
| Ctrl-D | Deletes forward, or quits with a warning for unsent input. |

Focus the destination with arrows or Tab, then drag files from Finder or another file manager into the terminal. Most terminals paste shell-escaped paths; quoted paths, spaces, Unicode, `file://` URLs, and multiple files work. UTF-8 files insert their actual text. PDF files insert extracted text and require Poppler's `pdftotext` on PATH (`brew install poppler` on macOS). Scanned PDFs with no text need OCR or pages exported as images. Other binary formats report an error and leave the form available.

PNG, JPEG, GIF, WebP, BMP, TIFF, and ICO images appear by filename in the focused context, question, option, or score level. Internal image references stay in saved data and native provider input; they are hidden from the TUI. Arrow movement and deletion treat an imported image as one attachment. Animated images use their first frame. The image bytes are saved as session evidence and sent through the provider's image format, so later turns do not depend on the original file path. Providers that only support images in shared state receive labelled images there. Choose a model with image support; provider limits still apply. For Jev, Nouli sends an upright still image scaled or padded to its accepted 280–1568 pixel dimensions, while saving the original. Image context uses a separate pixel-based estimate for Jev and direct OpenAI; repeated image references count separately. These estimates are approximate. Other routed models retain their encoded-byte capacity estimate where required by the provider. Each file is capped at 16 MiB.

Pasting never submits. Context and question fields preserve paragraphs and indentation; Options split pasted lines into individual items. The alternate-screen transcript disappears when the view closes, while the session retains its saved turns. Scripts and `TERM=dumb` keep the line syntax documented above.

## Reference earlier answers in the TUI

Type `@` at a word boundary in Context, Question, an option, or a score level.
Search ordinary words from saved conversations: context, structured values,
questions, criteria, options/levels and saved answers. Question IDs, session
names and turn IDs remain searchable too. Suggestions
cover **all saved decisions in the selected local store**, including other
sessions, workspaces, branches, and workflow decisions. The default store is
shared across directories; an explicit `--store` searches that store. Independent
sessions remain separate in the graph and retain their own active facts.

Each result shows its answer, source session and saved turn, with a matching
excerpt when the words come from elsewhere in the conversation. Repeated questions
remain distinct answers, even when they share an ID. Use Up/Down and Enter to
insert one; the first Enter only inserts the reference. Choose **Browse history**
for a larger list with answer distributions and the actual saved input state,
including structured values and image filenames. PgUp/PgDn scroll the preview;
Up/Down choose results. Narrow terminals stack the preview below the list.
The preview reads the selected turn's input manifest, preserving historical
values after later replacements. Encoded image bytes are not shown or searched.
Ctrl-O in a field
with references inspects the reference nearest the cursor. Backspace/Delete
removes a whole reference. Removed draft references add no new source evidence;
previously saved history still follows the usual context policy.

Tab closes suggestions and moves to the next field. Shift-Tab closes them and
changes the answer type left to right. Esc closes them and leaves ordinary
editable text. Email addresses stay literal. Ctrl-S/F5 waits for you to select
or dismiss the open search before sending.

A reference retains its exact question, saved answer, provider, saved input
state and native image inputs. It never resolves to a newer answer
later or replaces the current draft's type, options or levels. Saved source turns
remain immutable. The latest answer is still included automatically; choosing
it does not duplicate its answer in context. Required evidence that cannot fit
produces an error and preserves your draft. Internal binding markers remain in
saved data and structured exports while human output shows the readable label.

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

`--provider` and `--model` select a named provider and a model independently, and `--context-budget TOKENS` overrides the estimated budget, clamped to an explicitly configured window. Omitted budgets use 8192. Resolution is the command line, the request file, the session's defaults, then configuration defaults. Accepted work freezes its settings and sources, records the actual served provider and model, and never falls back to another one.

`--request-id KEY` (on `new`, `add`, and `ask`) identifies the normalized intent globally. A completed request replays the saved turn, and a conflicting intent fails. The record IDs of facts given on the command line are derived from the key, so the same command replays. `--replace` numbers refer to the facts as they are when the command runs; a repeated command with a request ID replays the replacements it saved, and a different replacement under the same ID conflicts. `--retry-interrupted` requires a request ID and explicitly retries eligible interrupted work with its saved settings. Ctrl-C cancels the shared engine and awaits its cleanup. A `whatif` has no request identity.

## Library

The same operations are available to embedding programs: see [embedding](embedding.md) for `context::facts` and `Ledger::facts`.

### Local files with `@`

Type `@` and a filename or relative path, such as `@documents/budget.xlsx`.
The existing picker searches saved answers across the local ledger and files
under the directory where Nouli started, including subfolders. File rows say
“Local file”; paths are relative to that launch directory, which stays the same
through session changes. Discovery respects ignore files, skips hidden paths
and symlinks, and reads contents only after selection. Reopening the picker
refreshes the file list. Discovery is bounded to 5,000 files, 20,000 directory
entries and 250 ms per opening; drop an exact path for files outside that list.

Enter inserts one file without sending the decision. Tab moves fields and
Shift-Tab changes answer type as usual. The selected field receives extracted
text/tables or an image label. In Options/Levels, a file occupies one item;
ordinary multiline list paste still adds multiple items. Text imports show their
contents for review; `@` also includes the relative filename. Sending uses that
snapshot even if the original file is changed or deleted. Failed imports retain
the query and draft.

| Format | Input sent |
| --- | --- |
| Text, source files, JSON, Markdown and other UTF-8 text | Text |
| CSV, TSV | Parsed rows with quoted/multiline cells preserved |
| DOCX | Body text and tables |
| PPTX | Slide text and tables, in presentation order |
| XLS, XLSX, XLSM, XLSB, ODS | All sheets as named row arrays; dates as ISO text; cached formula values, or an explicit formula when no cached result exists |
| PDF | Text extracted locally with Poppler `pdftotext`; scanned pages need a text export |
| PNG, JPEG, GIF, WebP, BMP, TIFF, ICO | Native image input; other decoded raster formats normalize to PNG |
| SVG | Rendered PNG; linked assets must be embedded or exported as PNG |
| DOC, PPT | Optional local LibreOffice conversion to DOCX/PPTX, then text/table extraction |

Documents do not render pages, charts or embedded images, and no OCR or formula
recalculation runs. Macro code is not executed. Native image support and context
limits depend on the selected provider/model; existing preflight checks apply.
Nouli sends extracted text or images through its existing adapters, with no
new document-upload API or conversion service.

Files and extracted output are limited to 16 MiB, ZIP documents also have a
16 MiB expanded-content limit, and sheets are limited to one million cells.
Modern document/SVG parsing runs in an isolated worker with a ten-second deadline
and memory/CPU limits; failed workers leave the TUI usable. Optional LibreOffice
runs in a private temporary directory with a separate profile, resource limits
and process-group cleanup. Install Poppler or LibreOffice separately when needed;
Nouli does not install them automatically.

## Connect and select models

`nouli connect [SERVICE]` prompts for a connection, credentials and model, then
optionally saves startup defaults. Services are `openai`, `typesafe`, `openrouter`,
`cloudflare`, `ollama`, `llama.cpp` and `custom`. These are setup shortcuts that
write ordinary settings; hand-written configuration is equally supported.
Use `--non-interactive` with complete values for scripts, `--env NAME` or
`--stdin` for credentials, and `--replace` to replace an existing connection.
For credential programs, put `--command PROGRAM ARGS...` last. `--no-auth`
disables primary authentication. `--name`, `--protocol`, `--binding`,
`--base-url`, `--model` and `--default` make selections explicit.

Inside the full-screen session, `/connect [SERVICE]`, `/login [CONNECTION]`,
`/logout [CONNECTION]` and `/model [CONNECTION]` run while idle and retain the
unsent question/options. Setup uses the terminal outside the alternate screen,
then reloads settings. API keys never enter transcript/history fields. `/model`
accepts any exact model ID; saving a startup default is an explicit choice.
See [configuration](configuration.md) for manual TOML and all compatibility fields.

## Verify terminal selection before release

On macOS or Linux, run from the workspace root:

    cargo test --package nouli-cli --test tui_selection --locked

This Rust integration test also runs as part of `cargo test --workspace` and requires no Python interpreter. Release builds run it with `--release` on each supported platform. It uses a POSIX PTY without credentials or provider calls to verify default native dragging, graph mouse reporting, screen stability, input preservation, resizing, and cleanup. Native clipboard behavior is owned by the host terminal; also check ordinary drag-selection, graph resizing, F6, Copy, and resume in the terminals supported for the release.
