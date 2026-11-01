# Nouli website

The site published at [nouli.ai](https://nouli.ai/) through GitHub Pages: a home page, a user guide, and the example walkthroughs. It is plain HTML, CSS, and JavaScript, built with [Vite](https://vite.dev/).

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

The version beneath the home-page install commands is fetched from GitHub's
`releases/latest` API when the page is viewed, using the same approach as
Handshake. It does not read the workspace version. Offline, without JavaScript,
or if the request fails or exceeds ten seconds, the link reads “latest release”.
Building the site requires no release lookup.

## Layout

- `index.html`, `guide/`, `examples/`: one HTML page per route. Vite picks up every directory under `examples/` as a page.
- `src/figure.js`: the decision graph and readout used on the home page and in walkthroughs.
- `src/data/`: graph data generated from real Nouli sessions. Don't edit it by hand.

Fonts are served from [public/fonts/](public/fonts/) with their SIL Open Font
License notices. The homepage preloads Archivo and Martian Mono; neither the
site nor its approved layout depends on Google Fonts requests.

## Search and answer engines

Keep the product definition, installation commands, and example explanations in
the HTML so they remain readable without JavaScript. The home page describes
Nouli beside its headline; its SoftwareApplication JSON-LD uses the same
description and links to the repository, license, and guide. Preserve that
visible definition when changing the opening layout. Structured data describes
the software; it does not promise a rich result or an AI citation.

Every published route has a description. The Vite build adds a canonical URL
under https://nouli.ai/ and emits sitemap.xml from the same page list used to
build the site, including newly added examples. The build
fails if a published page has no description. The sitemap can be submitted in
Search Console after deployment; generating it does not submit it.

The existing [llms.txt](public/llms.txt) remains an agent usage reference, not an
indexing requirement. Follow
[Google's guidance for generative AI search](https://developers.google.com/search/docs/fundamentals/ai-optimization-guide):
use crawlable, helpful text and keep metadata consistent with visible content.

The custom domain is recorded in [public/CNAME](public/CNAME). The build copies
[public/robots.txt](public/robots.txt) to https://nouli.ai/robots.txt, where it
allows crawling and advertises the sitemap. Crawl rules are scoped to the host
root; a /nouli/robots.txt on the GitHub alias would not control crawling. See
[robots.txt scope](https://developers.google.com/crawling/docs/robots-txt/robots-txt-spec).
Local build and browser checks do not verify indexing or search visibility.

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

## README images

The root README uses a recorded TUI replay in `.github/readme/tui-replay.gif`,
with a static frame in `.github/readme/tui-poster.png` and capture details in
`.github/readme/tui-replay-provenance.json`. The reusable brand banner uses the
website's paper and teal palette, shared bee, and Archivo headline. The detailed
recall diagram lives with its recorded source in
[docs/recall-example.md](../docs/recall-example.md).

Regenerate both images from this directory:

```sh
node scripts/readme-hero.mjs > ../.github/readme/hero.svg
node scripts/readme-hero.mjs --recall > ../.github/readme/recall.svg
```

The generator uses the checked-in Archivo and Martian Mono files from
[public/fonts/](public/fonts/) and the bee in [public/brand/](public/brand/).
It subsets the fonts into each SVG, so the images work without external font
requests or downloads. Their SIL Open Font License notices are alongside the fonts.

The recall layout reads the Mermaid block in the preserved example: answers,
probabilities, and ancestor counts come from that graph. The script stops if the
graph or a selected answer no longer matches its layout. `npm test` checks both
generated assets and the recorded diagram's ancestry and answer validation.

## Replaying walkthroughs

When the CLI changes, a walkthrough's commands and outputs have to be regenerated. `scripts/replay.mjs` runs a page's commands in an isolated `HOME` and writes their output back into the page, then exports the graph data again from the store it built.

```sh
node scripts/replay.mjs --example recall --nouli ../target/debug/nouli          # one walkthrough
node scripts/replay.mjs --example all --nouli ../target/debug/nouli             # all of them
node scripts/replay.mjs --example recall --nouli ../target/debug/nouli --check  # compare, write nothing
```

**Offline (the default)** needs no key and costs nothing. A local stub speaks the System One protocol and answers each model call with the result recorded for the matching step, so a run is deterministic. A call matches a step by its question prompts and option labels, since option IDs come from labels, taking the first unused step in order. The run fails if a call has no match or a recorded step is never asked. Saved steps hold every probability in `src/data/examples/*.json`. A step that saved nothing (a `whatif`) has only what its output printed, so the stub reads the answers back from the page: the bars give each option's probability to two decimals, which the stub adjusts within that rounding until they sum to one, and a score's levels come from the line beneath it. Output from CLI 0.1.0, which printed only the selected option's probability, still works: the rest is shared evenly across the other options. Such a step matches on its question IDs and, when the bars show them, its option labels.

**Live (`--live`)** runs the same commands against the provider in the current directory's `.env` (`TYPESAFE_TOKEN`, and `TYPESAFE_BASE_URL` if the endpoint is not `https://api.typesafe.ai`). Every step calls your provider account and costs money, the script says so, and the answers can differ from the recorded ones. Check them before you commit.

`--check` compares what the commands print with the page, and the exported graph with `src/data`, ignoring session and turn IDs, and exits nonzero on a difference. Without it, each run replaces the IDs in the page and the graph data, so a clean run changes only IDs. It writes a page only when every step ran.

The page's commands run through `sh`. A page may print apostrophes inside single-quoted text without escaping them; the script escapes those before running so the command still works. The commands call `nouli`, so `--nouli` names the binary they use.

## Home-page CLI replay

The home page shows the real full-screen app on a teal bench. `src/cli-replay.js`
renders `src/data/cli/tui.json` as selectable text at 3× speed, in the
app's own recorded colors (`src/term-colors.js`). It starts
once visible; reduced-motion visitors get the finished session. Clicking the
terminal never pauses it; press Space to pause or resume, and Enter to restart.

The recording runs a five-turn workshop session: a yes/no question, two
branches, a blend and a score. Its right-hand pane is the app's own decision
graph. Beneath the terminal, `src/home.js` draws the same session from
`src/data/cli/graph.json` with the instrument figure (`src/figure.js`): x/y
rulers, a crosshair on the selected turn, and its readout. The figure follows
the replay as each answer arrives. Clicking a box in the TUI's graph pane
lightens that box and selects the same turn in the figure without pausing.
Screens 700 px wide or less load `tui-narrow.json`, a 64-column recording of
the transcript only.

Re-record both after TUI changes. The recorder drives the app in a PTY and
answers from the saved answers in `graph.json` through a local stub, so it uses
no credentials or provider calls. It needs Python with `pyte`:

```sh
node scripts/record-cli-replay.mjs --nouli ../target/release/nouli --python /path/to/python
node scripts/record-cli-replay.mjs --nouli ../target/release/nouli --python /path/to/python --columns 64
npm test
```

The wide recording also refreshes `graph.json` from its own store, so the turn
IDs shown in the graph pane match the figure. `provenance.json` records each
recording's binary, size and timing. The illustrative answers come from saved
live TypeSafe runs; the score turn was answered live once.

Replay updates use a fixed 60 Hz clock, with cursor positions interpolated on
each display frame. Hidden tabs pause playback, and stalled frames catch up by
at most 100 ms. Text scrambling also keeps a 60 Hz cadence on faster displays.

## Current TUI captures

The guide and walkthroughs display real 140-column and 80-column PTY
captures of the current full-screen app. Form, Answer and Decisions show the
actual before-send form, saved result, and narrow graph focus. Static text
fallbacks keep walkthrough captures readable without JavaScript.

`capture-tui.py` needs Python with `pyte` (a recording-only dependency). With a
current release build and that interpreter, run from `site/`:

```sh
node scripts/tui-capture.mjs --example all --nouli ../target/release/nouli --python /path/to/python
npm test
npm run build
```

Each walkthrough runs in a temporary HOME/store against the existing offline
recordings in `src/data/examples/`. The stub maps generated form IDs to the
same recorded prompts, types and answer labels, preserving recorded answers and
probabilities; unmatched calls fail the capture. No provider credentials or
live calls are used. Existing graph recordings remain unchanged. The simple
meeting example reuses three travel answers from `src/data/hero.json`, replayed
through the CLI export tools before its TUI capture.

Capture data lives in `src/data/tui/`; `src/tui.js` renders its terminal cells
with selectable text, native horizontal scrolling and responsive wide/narrow
screens. `npm test` checks coverage, form fields, type colors and the absence of
obsolete prompt-based captures. Browser validation should exercise all three
views, graph navigation, command toggles, mobile layouts and reduced motion.

## Walkthroughs

Each walkthrough page links its steps to graph nodes with `data-node`, the node's index in that page's JSON. When you change a walkthrough's sessions, export its graph again and update the step links to match.

## Download installer

`public/install.sh` is published as `/install.sh` under the site base path. It installs existing release archives; it does not build Nouli or call a decision provider. Run `npm test` after changing it. The tests use local archive fixtures and simulated platforms without network calls or changes to your installed executable.
