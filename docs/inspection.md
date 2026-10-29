# Read-only session inspection and export

```sh
nouli sessions list
nouli facts Research
nouli questions Research
nouli view Research
nouli view Research | less -S
nouli view Research --mermaid > research.mmd
nouli sessions show Research --format text
nouli context Research --turn 019a0000-0000-7000-8000-000000000001 --json
nouli sessions export Research --format json > research.json
nouli sessions export Research --format toon > research.toon
```

`nouli facts SESSION` lists the session's numbered facts, as `nouli add` and `--replace N` number them. A fact that came from a branch or blend source is marked `(from SESSION)`, and a replaced fact shows the text it replaced (`(replaced; was: ...)`). `nouli questions SESSION` prints the standing questions in the one-line grammar, one per line, in the order they were first set. Both support `--json` (and `--format toon`); a fact that blended branches replaced differently lists every alternative and `--replace N` settles it, the JSON lists each fact's number, records, key, text, turn, source session, and previous text (plus `alternatives` for a conflicted fact), and each question's ID, grammar line, and full definition.

`view SESSION` prints a decision graph and exits. Each turn is a
box containing its session name, unambiguous short turn ID, questions, and result
summaries. Earlier decisions appear at the top, with arrows leading to later
decisions. Independent histories appear side by side, with blends below their
parents. Long questions and answers wrap inside their boxes; several questions
stay together in the same turn.

Choice questions list every option and its probability, with `▶` marking the
selected answer. Score questions list all rubric levels and their probabilities,
then mark the expected score with `▶`. Noul questions keep `P(yes)`, also marked
with `▶`. Refusals show their reason and list options with `n/a` probabilities.
Long option descriptions wrap inside the box. Use show/export or context for
original state, native confidence, and the complete saved record.

`[latest]` marks a finalized head of the selected session. Arrows connect parent
turns to their children; `╳` marks crossing lines that do not join. Source ancestry
is fixed at the captured turns, so later source updates, unrelated branches, and
descendant sessions are excluded. Shared ancestors appear once. Failed and
unfinished attempts are available through show/export rather than displayed as
finalized decisions. A session with no finalized turns can still show its
inherited ancestry; an entirely empty history prints a message.

Highlighted rows are teal and bold in a terminal. Piped output stays plain text,
with `▶` preserving the result marker. Set `NO_COLOR=1` to disable color; color is
also disabled when `TERM=dumb`. The graph accepts `--format auto` or `--format text`
regardless of your configured output format. For JSON or TOON, use
`nouli sessions export` instead. The graph does not start a pager, load env files,
require provider credentials, or change saved history. For graphs wider than your
terminal, pipe the output to `less -S` and scroll horizontally.

`view SESSION --mermaid` prints the same graph as Mermaid flowchart
source instead. Nouli does not draw the diagram; paste the output into a
```` ```mermaid ```` block in Markdown, where GitHub, GitLab, and other Mermaid
renderers display it as boxes and arrows. Each turn becomes a node with its
session name, short turn ID, questions, and every option's probability. Selected
results are bold and marked with `▶`, and `[latest]` heads have a thicker border.
Mermaid wraps long lines itself. Caller text is escaped so it cannot end a label,
add markup, or change Mermaid settings. `--mermaid` cannot be combined with `--json`, `--jsonl`, or
`--format`.

Selectors are existing session IDs or unique case-sensitive names. The global list includes all sessions in the chosen user store. Show/export includes the selected session, its fixed creation sources, finalized heads, complete reachable turns, source-session provenance, relevant attempts, accepted request metadata, and active caller-record retention. Common ancestors appear once in canonical history order. Source provenance omits moving source heads, and later or unrelated source work is excluded. Full original state, choices, rubrics, probability distributions, refusals, usage, and record identities survive model-context compaction.

Turn outcomes are model judgments. Caller evidence and commitments are separate context records with their own kinds, revision origins, and pin state. Structural conflicts are reported explicitly rather than presented as resolved active context. Inspection does not apply updates, synchronize sources, or rewrite records.

`context SELECTOR` defaults to the session's sole finalized head. Use `--turn ID` when there are several heads or to inspect an older turn owned by that session. To inspect an inherited source turn, select its original session. The report contains the exact saved manifest, rendered TOON, included/compacted/omitted projections, protection flags, budget, counter method/exactness, requested identity, actual outcome identity/usage, and relevant attempt history. Accepted request and node-intent metadata retain explicitly required IDs and frozen selections. A protected record may be pinned, explicitly required, or current input; protection alone does not mean every historical record reached the model.

Exports include owned workflow definitions/progress, prior retry-round results/events, and accepted node intents. Source workflows contribute frozen definitions and the relevant captured turns/attempts, without importing uncaptured sibling results. Credential source names and nonsecret settings remain metadata; credential values are never stored by these contracts. Export is a serialization for inspection, not an import or federation facility.

For show/export and context, text displays the full structured history with a descriptive heading. JSON and TOON contain version-1 data without decoration. Output flags work before or after the subcommand. JSONL is reserved for invocation events and is rejected for these read-only commands. Inspection reads store/output configuration only, so removed provider profiles and unavailable credentials do not prevent viewing saved history. Invalid configuration syntax or store/output settings still fail explicitly.

All commands open an existing ledger read-only. Missing stores are reported without creating directories; older schemas require a separate writable migration, and newer schemas fail with the found/supported versions. Show/export uses one coherent SQLite read snapshot and performs no migration or recovery writes. Library callers can use `Ledger::export_session`, `inspect_turn`, and `inspect_session_turn` with the same contracts.
