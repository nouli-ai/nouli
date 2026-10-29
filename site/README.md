# Nouli website

The site published to GitHub Pages: a home page, a user guide, and the example walkthroughs. It is plain HTML, CSS, and JavaScript, built with [Vite](https://vite.dev/).

## Develop

Requires Node.js 24 or later. From this directory:

```sh
npm ci
npm run dev      # local server with live reload
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

Recording makes 19 model calls on your provider account. Answers can differ between recordings, so check the new results before you commit them.

## Walkthroughs

Each walkthrough page links its steps to graph nodes with `data-node`, the node's index in that page's JSON. When you change a walkthrough's sessions, export its graph again and update the step links to match.
