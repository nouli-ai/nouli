# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- Serve the website's fonts from nouli.ai so the headline and TUI layout remain consistent when external font services are unavailable.
- Present Nouli as a ledger-backed decision network with "Every decision has a history" across the website, guide, agent reference, and README. Link the Nouli brand to nouli.ai.
- Bring the website's TUI higher with a smaller headline, a compact product explanation beside it, and no second introduction above the replay.
- Add page descriptions, canonical URLs, and an automatically generated sitemap to help readers and search engines find Nouli's guide and recorded examples.
- Keep the TUI's bee greeter at the top of the transcript, so output scrolls it away instead of replacing it; scrolling back reveals it again.

## [0.4.2] - 2026-10-30

### Added

- Greet interactive sessions with Nouli's honey bee above the form, with `> <` eyes at rest and `> _` while drafting. The greeter gives way to conversation output and stays hidden in short terminals.

## [0.4.1] - 2026-10-30

### Changed

- Configure any compatible service through `system-one` or `openai-decisions`, explicit HTTP bindings, headers, image formats and optional limits. Remove provider/model recognition and the bundled capacity catalog; omitted budgets use 8,192 estimated tokens, clamped to a configured window. This replaces the `typesafe` and `openrouter-decisions` protocol names. Older frozen runs require a fresh store and are preserved untouched.
- Refresh the Pages home page, guide and walkthroughs with captures of the current full-screen TUI. Add a simple three-turn meeting example alongside the complex multi-session examples, preserving recorded model answers.
- Resize the graph by dragging its divider at any time, including while editing a field. Ctrl-G remains keyboard navigation; use the terminal’s selection modifier for native text selection. Keep the chosen width through session changes and clamp it when the terminal shrinks.
- Connect referenced answers to the questions that use them in the TUI and `nouli view`, including after reopening a destination session. Load the exact saved source history without blending sessions; separate question cards where necessary and mark reference edges with dashed arrows in Mermaid.
- Search saved conversation text with `@`, including context, structured values, options and answers across local sessions. Show matching excerpts, carry the selected saved state into the next decision, and preview historical input with image filenames and scrolling in Browse history, including narrow terminals and long drafts.
- Show the latest saved question included in the next TUI request, including Yes/No follow-ups, and retain its answer and used images alongside new evidence during context reduction.
- Show image filenames in the form, transcript, graph, and session previews. Keep internal image references hidden while preserving native image data; attachment movement and deletion operate on the complete image label.
- Use Shift-Tab as the only answer-type shortcut, cycling left to right from any field, with a short hint beside the selector. Tab navigates fields and options/levels.
- In Options/Levels, Enter adds the next row; a second consecutive Enter sends. Typing, pasting, or navigation continues editing without sending.
- Simplify the terminal view with a `nouli` header, plain question text, hidden generated question IDs, one-line reuse notices, shorter key hints, and one model name. Enter sends from Context/Question when ready; in Options/Levels, Enter adds a row and a second Enter sends. Tab moves between fields and list items.
- Use the terminal’s selection modifier for native text selection (Option-drag in iTerm2). Navigate form fields with the keyboard; mouse reporting lets the divider resize without changing focus.
- Use Citrus accents for answer types: lime for Yes/No, cyan for Choose one, and rose for Score, including results and decision cards. Color-free output keeps text emphasis.
- Keep the selected answer type after sending. In the terminal form, leaving a choice's options or a score's levels empty reuses the latest matching question's exact list and rubric, with a visible notice. Leave its question empty too to ask it again with new context. Enter a new list to replace it. Yes/No questions stay fresh, and earlier answers remain in history.
- Ask questions in plain text in the full-screen form, with automatic IDs and literal punctuation. Shift-Tab cycles answer types left to right from any field; Tab navigates fields and moves through options/levels, adding a row after the last filled item. Ctrl-S / F5 submits from any field, including context still being edited.
- Paste several lines into Options to add one option per line. Context and question fields preserve paragraphs and indentation.
- Keep decisions from sessions opened in the same TUI visible when starting another session. New sessions appear as independent roots; shared ancestry is drawn once.
- Prepare upright, resized images for Jev automatically while preserving saved originals. Image estimates count visual input separately from text, including repeated references, so screenshots no longer consume context as base64 text.
- Show the selected provider/model and Ready, Draft, Asking, or Cancelling status before the first answer. After a response, the actual served model replaces its alias. Key hints follow the focused field, failed input stays available for retry, and unnamed graph sessions use short IDs.

### Removed

- Remove the interactive `/whatif` command from completion, help, and input handling.

### Added

- Set up connections with `nouli connect`, service shortcuts or a fully custom endpoint, masked API keys, environment sources and credential programs. Edit the generated ordinary TOML directly; setup preserves unrelated comments and serializes concurrent edits. Use `/connect`, `/login`, `/logout` and `/model` while idle without losing the draft.
- Call Cloudflare Workers AI directly through the shared System One codec and its explicit HTTP binding, including model-specific routes and response envelopes.

- Find files from the launch folder with `@` beside saved answers. Select without sending, and snapshot contents into the focused field. Import DOCX/PPTX text and tables, CSV/TSV rows, XLS/XLSX/XLSM/XLSB/ODS sheets and native images including rendered SVGs; PDF text uses Poppler and legacy DOC/PPT conversion optionally uses LibreOffice. File drops use the same parsers, and each file occupies one option/level.
- Rename the current session with `/rename NAME`, preserving its history, IDs, model settings, references and unsent draft. Header, graph and later `@` searches use the new name; collisions leave the session unchanged.
- Reference saved answers with `@` in any TUI field. Search question text or IDs across all sessions in the selected local store, inspect answer versions in Browse history, and insert an exact source without changing the current question type or answer list. References retain their saved results and image inputs, survive follow-ups and session changes, and remain ordinary selectable terminal text.
- Give new sessions without an explicit name a durable random `scientist-number-noun` name, such as `curie-7-diffraction`, using short lowercase surnames, numbers 1–20, and original spinner-style noun forms. Names are unique within the ledger, including concurrent launches. Bare `/new` now works without a name; explicit names remain available.
- Drop files into the focused terminal field: UTF-8 files insert their contents, PDFs import text through Poppler's `pdftotext`, and PNG, JPEG, GIF, WebP, BMP, TIFF, and ICO images become saved image evidence. Quoted paths, shell-escaped paths, file URLs, multiple files, and terminals without bracketed drops are supported. Each file is limited to 16 MiB; unsupported files report an error without submitting.
- Send image evidence using native TypeSafe, OpenAI Decisions, and OpenRouter formats. Images used by the current turn are required evidence, so context reduction cannot silently remove them. Provider model support and context limits still apply.

## [0.4.0] - 2026-10-29

### Added

- Open interactive sessions full screen. In a terminal, a bare `nouli` starts a new session, known by its short ID, and `nouli open SESSION` opens one, creating it without asking when it is missing. The transcript sits above a form that holds the queued facts, the question, and its options or levels: Shift-Tab picks yes/no, choice, or score, and Tab, Up, and Down move between the sections. The session's decisions are drawn beside the transcript when the terminal is at least 116 columns wide, and Ctrl-G shows them in narrower ones. Typing `/` lists the commands, `/sessions` resumes an earlier session, and Ctrl-C cancels a running turn.

### Changed

- Brighten the teal accent in terminal output.

### Removed

- Remove the line-by-line prompt. Nouli no longer writes `repl_history`, so you can delete that file from your store. Piped input, and terminals that take no escape codes (`TERM=dumb`), still read lines from standard input. A bare `nouli` no longer lists sessions before opening one; `/sessions` lists them in the view.

## [0.3.0] - 2026-10-29

### Changed

- Ask each question once. A turn asks only the questions given with it, `-q` or a `?` line, in one call, and no later turn, branch, or blend asks them again, so a saved answer is never decided again. Sessions saved by 0.2.0 keep reading, and the questions they saved are no longer asked.
- Take one question per command: `-q` no longer repeats, so each question is its own call, and a second `-q` is a usage error. Request files and workflow nodes can still ask several independent questions in one call.
- Require a question for every turn: `nouli add`, `ask`, and `whatif`, and `new` with facts, fail with `questions_required` without `-q`. In `nouli open`, facts wait in the queue until a `?` line sends them.
- Make `/whatif` ask its own question, `/whatif ?id: prompt [a, b]`, with the queued facts; it still saves nothing. An empty line now sends again only what a failed turn left queued.
- Drop `re-asked N` from turn headers.
- Describe Nouli as a decision graph in the README and on the website. The README opens on the recall example's graph, three source sessions and two layers of blends, with a table of what each blend was built on, and a prompt to hand Nouli to a coding agent.

### Added

- Add `llms.txt` to the website: the commands and rules an agent needs, with links to the docs.

### Removed

- Remove standing questions: `--drop`, `nouli questions`, and the `/drop`, `/questions`, and `/ask` commands. A request file that sets `standing` fails with `invalid_standing_question`, and session exports no longer list `standing_questions`.

## [0.2.0] - 2026-10-29

### Added

- Add Homebrew installation through `standing-desks/tap/nouli` and notify the tap when stable releases are published.
- Add a checksum-verified download installer for Apple silicon macOS and ARM64 or x86_64 GNU/Linux.
- Build Linux x86_64 release archives alongside the existing ARM64 downloads.
- Add a `session` report for a session created without a turn.
- Add a credential store: `nouli auth login PROVIDER` saves a key once, `nouli auth logout PROVIDER` removes it, and `nouli auth list` shows where each provider's key comes from. A key can be stored directly, read from an environment variable, or printed by a command. The file is created private (`auth.toml`, mode 0600) and refused if others can read it. Shell variables and `.env` files still win.
- Add an interactive session loop: `nouli open SESSION`, or a bare `nouli` in a terminal. Type facts, then ask a question with `?id: prompt [a, b]` to send them with it; questions stay with the session, and return on an empty line asks them again. Slash commands branch, blend, replace facts, drop questions, and run what-ifs. Without a terminal, `nouli open` reads its lines from standard input.
- Add `nouli facts SESSION`, which lists numbered facts and marks the ones inherited from a source session or replaced, and `nouli questions SESSION`, which prints standing questions in the one-line syntax. Both support `--json`.
- Accept questions written as one-line strings in workflow files, mixed freely with full question objects. A mistake names the node and question.
- Add a one-line question syntax: `id: prompt [a, b]` for a choice, `id: prompt <a, b, c>` for an ordered score, and `id: prompt` for a yes/no question.
- Add standing questions to the library: a session remembers the questions it keeps asking, so a turn can send only new facts. A branch inherits its source's questions as they were at the branch point, and a blend starts with none.
- Add facts to the library: caller evidence stored as numbered records. Replacing a fact keeps its number and the model sees only the new text, while the earlier version stays in the session history.

### Changed

- Replace evaluating at the root command with verbs: `nouli new NAME`, `nouli add SESSION`, `nouli ask SESSION`, and `nouli whatif SESSION`. A session keeps its facts and the questions it keeps asking, so a turn only needs what is new. `nouli new NAME` with no facts or questions creates the session without a turn.
- Write questions on the command line as `-q "id: prompt [a, b]"` instead of JSON. Questions given with `new`, `add`, or `ask` become standing questions that later turns ask again; `--drop ID` stops asking one.
- Record facts as numbered evidence instead of turn text. `--replace N TEXT` corrects fact N; the model sees only the new text and the earlier version stays in the session history.
- Print turns as text with probability bars for every option, with a marker where an answer moved since the last turn. `--json`, `--jsonl`, and TOON output keep their shape.
- Select the session by position in `nouli view SESSION`, and rename `nouli workflows` to `nouli workflow`.
- Update the guide, README, and website walkthroughs to the new commands, with walkthrough outputs recorded from this release.
- Put the install commands, a recorded interactive session, and a getting-started section on the website's home page, add an "interactive" link to every page, and move interactive mode to the start of the guide. Walkthroughs show each step as the lines you would type in `nouli open`, with a toggle for the commands. Terminal output on the site uses the system monospace font, so probability bars line up.
- Take a request file as `--input FILE` without a `session`; the command supplies the session.

### Fixed

- Keep the user guide within the viewport on narrow screens, with long commands and tables scrolling within the content.

### Removed

- Remove `nouli view --session` and `nouli workflows`.
- Remove `-s/--state`, `--name`, `--session`, `--no-session`, and the root `--from` and `--questions` options. Use `nouli new NAME --from SOURCE`, `nouli add SESSION`, and `nouli whatif SESSION`.
- Remove JSON and TOON text after `-q`. Put complete questions in a request file with `--input`.

## [0.1.0] - 2026-10-29

### Added

- Add typed choice, noul, and score decisions through the Nouli CLI and Rust library.
- Add durable session histories, branching, blending, and workflow recovery.
- Add decision graph views in the terminal, and `nouli view --mermaid` for Mermaid flowcharts on GitHub and other Markdown renderers.
- Add provider configuration and `.env` credential loading.
- Add a website with a user guide and interactive walkthroughs for robotics, banking, venture investing, food recalls, cybersecurity, and energy planning. Each walkthrough shows its commands, recorded model outputs, and a decision graph that follows along as you read.
- License Nouli and its Rust packages under MIT, including the license in native archives.
- Add Linux and macOS ARM64 downloads, with draft releases prepared when a version tag is pushed.
