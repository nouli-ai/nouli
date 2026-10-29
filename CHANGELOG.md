# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Add Homebrew installation through `standing-desks/tap/nouli` and notify the tap when stable releases are published.
- Add a checksum-verified download installer for Apple silicon macOS and ARM64 or x86_64 GNU/Linux.
- Build Linux x86_64 release archives alongside the existing ARM64 downloads.
- Add a `session` report for a session created without a turn.
- Add a credential store: `nouli auth login PROVIDER` saves a key once, `nouli auth logout PROVIDER` removes it, and `nouli auth list` shows where each provider's key comes from. A key can be stored directly, read from an environment variable, or printed by a command. The file is created private (`auth.toml`, mode 0600) and refused if others can read it. Shell variables and `.env` files still win.
- Add an interactive session loop: `nouli open SESSION`, or a bare `nouli` in a terminal. Type facts, define questions with `?id: prompt [a, b]`, and press return on an empty line to send. Slash commands branch, blend, replace facts, drop questions, and run what-ifs. Without a terminal, `nouli open` reads its lines from standard input.
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
- Show the guide, README, and walkthroughs with the new commands. The walkthrough outputs were replayed from the recorded answers and still need a live re-recording.
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
