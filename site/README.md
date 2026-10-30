# Nouli website

The site published to GitHub Pages: a home page, a user guide, and the example walkthroughs. It is plain HTML, CSS, and JavaScript, built with [Vite](https://vite.dev/).

## Develop

Requires Node.js 24 or later. From this directory:

```sh
npm ci
npm run dev      # local server with live reload
npm test         # installer checks with local release fixtures, and the README hero
npm run build    # writes dist/
npm run preview  # serves dist/
```

Pushing changes under `site/` to `main` builds and deploys the site through [the Pages workflow](../.github/workflows/pages.yml). CI builds it on every pull request that touches `site/`.

## Layout

- `index.html`, `guide/`, `examples/`: one HTML page per route. Vite picks up every directory under `examples/` as a page.
- `src/figure.js`: the decision graph and readout used on the home page and in walkthroughs.
- `src/data/`: graph data generated from real Nouli sessions. Don't edit it by hand.

## Graph data

Every probability, context record, and answer on the site comes from a saved Nouli session. `scripts/export-graph.mjs` converts a session, or every session in a store, into the JSON the figure reads:

```sh
NOULI=../target/debug/nouli node scripts/export-graph.mjs --store STORE --all --rename '^recall-=' > src/data/examples/recall.json
```

The home page combines recorded variants: each train status and each choice of plan sources is a separate recorded session. To record them again, run this from a directory whose `.env` holds the TypeSafe key, then rebuild the data:

```sh
NOULI=/path/to/nouli sh /path/to/site/scripts/record-hero.sh /tmp/nouli-hero
NOULI=../target/debug/nouli node scripts/build-hero.mjs --store /tmp/nouli-hero/store > src/data/hero.json
```

Recording makes 22 model calls on your provider account, one question each. Answers can differ between recordings, so check the new results before you commit them.

## README hero

`.github/readme/hero.svg`, the image at the top of the repository README, is drawn from the recall graph in the README's `nouli view --mermaid` block: every answer and probability comes from there, and its squares count each blend's earlier decisions in that graph. GitHub shows README images without fetching web fonts, so the script subsets Archivo and Martian Mono into the file. Download `Archivo[wdth,wght].ttf` and `MartianMono[wdth,wght].ttf` from [google/fonts](https://github.com/google/fonts/tree/main/ofl) into a directory, then run:

```sh
node scripts/readme-hero.mjs --fonts DIR > ../.github/readme/hero.svg
```

The layout and its shortened labels are drawn for this one graph, so the script stops if the README's graph or one of its answers changes. `npm test` checks that the committed image still matches the README.

## Replaying walkthroughs

When the CLI changes, a walkthrough's commands and outputs have to be regenerated. `scripts/replay.mjs` runs a page's commands in an isolated `HOME` and writes their output back into the page, then exports the graph data again from the store it built.

```sh
node scripts/replay.mjs --example recall --nouli ../target/debug/nouli          # one walkthrough
node scripts/replay.mjs --example all --nouli ../target/debug/nouli             # all of them
node scripts/replay.mjs --example recall --nouli ../target/debug/nouli --check  # compare, write nothing
```

**Offline (the default)** needs no key and costs nothing. A local stub speaks the TypeSafe protocol and answers each model call with the result recorded for the matching step, so a run is deterministic. A call matches a step by its question prompts and option labels, since option IDs come from labels, taking the first unused step in order. The run fails if a call has no match or a recorded step is never asked. Saved steps hold every probability in `src/data/examples/*.json`. A step that saved nothing (a `whatif`) has only what its output printed, so the stub reads the answers back from the page: the bars give each option's probability to two decimals, which the stub adjusts within that rounding until they sum to one, and a score's levels come from the line beneath it. Output from CLI 0.1.0, which printed only the selected option's probability, still works: the rest is shared evenly across the other options. Such a step matches on its question IDs and, when the bars show them, its option labels.

**Live (`--live`)** runs the same commands against the provider in the current directory's `.env` (`TYPESAFE_TOKEN`, and `TYPESAFE_BASE_URL` if the endpoint is not `https://api.typesafe.ai`). Every step calls your provider account and costs money, the script says so, and the answers can differ from the recorded ones. Check them before you commit.

`--check` compares what the commands print with the page, and the exported graph with `src/data`, ignoring session and turn IDs, and exits nonzero on a difference. Without it, each run replaces the IDs in the page and the graph data, so a clean run changes only IDs. It writes a page only when every step ran.

The page's commands run through `sh`. A page may print apostrophes inside single-quoted text without escaping them; the script escapes those before running so the command still works. The commands call `nouli`, so `--nouli` names the binary they use.

## Interactive view of walkthroughs

Walkthroughs show their steps as interactive lines by default, with a toggle for the commands. `scripts/repl-view.mjs` turns each step's command into what you would type in `nouli open`: `/new`, `/blend A B as NAME`, `/use`, the facts, and then the question, which sends the turn. It runs them all in one `nouli open` session against the offline stub and writes each step's part of the transcript into a `<pre class="repl">` block. Session names drop the `$example-` prefix. A `whatif` step shows its command instead. The script fails if any turn prints differently from the step's recorded output. Run it after replaying a walkthrough:

```sh
node scripts/repl-view.mjs --example all --nouli ../target/debug/nouli
node scripts/repl-view.mjs --example all --nouli ../target/debug/nouli --check  # compare, write nothing
```

The home page's terminal plays `src/data/repl-session.txt`, a session recorded with live model calls through `nouli open`. Record a new one with real facts and questions and paste the output there.

## Walkthroughs

Each walkthrough page links its steps to graph nodes with `data-node`, the node's index in that page's JSON. When you change a walkthrough's sessions, export its graph again and update the step links to match.

## Download installer

`public/install.sh` is published as `/install.sh` under the site base path. It installs existing release archives; it does not build Nouli or call a decision provider. Run `npm test` after changing it. The tests use local archive fixtures and simulated platforms without network calls or changes to your installed executable.
