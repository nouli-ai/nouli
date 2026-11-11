# Site architecture

The static Astro site follows [ADR 0026](../docs/adr/0026-astro-site-stack.md), [AGENTS.md](AGENTS.md), and [DESIGN.md](DESIGN.md).

## Routes and data

- `/` composes the hive components in `src/components/hive/`. Its networks are illustrative and live in `src/lib/home/` (`illustrative.ts` and `teams/`).
- `/examples/` and `/examples/<slug>/` render the `examples` content collection from `src/content/examples/*.json`. Its schema validates step IDs, ancestry, probability bounds, and rounded distribution totals. Each page includes the source, license, sample, model, measured stats, routing counts and notes.
- The eight retired walkthrough URLs remain Astro redirects. The URL inventory verifies that each target exists. HTML validation permits either doctype case because Astro generates lowercase doctypes in redirects; the remaining recommended rules still apply.
- `/guide/` renders its recorded meeting capture. `/models/` and the model pages render the `models` collection.
- Files under `public/` retain their published URLs, including `/install.sh` and `/llms.txt`. `integrations/sitemap.ts` writes `/sitemap.xml`.

`lib/site.ts` holds shared URLs, install commands and navigation. Model facts live in `content/models/`. Example facts and probabilities live in `content/examples/`. Home operations, the network assignments and the measured count of models ahead of Jev live in `lib/home/teams.ts`; the total model count derives from the collection.

## Rendering and interaction

`BaseLayout` produces the shared head, CSP, main landmark, skip link and footer slot. `HivePage` wraps it for every page: each page draws the navigation in its sky header (`PageHead`, or the home hero) and points the skip link at the content after it. Published text, every answer distribution, SVG fallback edges, install commands and the terminal's final screen are present without JavaScript. All industry networks remain readable without scripts.

`lib/home/layout.ts` computes graph ancestry, positions and conservative wrapped-card heights at build time. Wide layouts use columns by depth. Narrow layouts run top to bottom with up to two cards per row, and one on phones. Deep networks switch to the vertical form earlier to keep tablet layouts readable.

Vanilla custom elements enhance this HTML:

- `float-nav` floats the navigation after scrolling and marks whether sky or page is behind it.
- `sky-field` draws a viewport-aligned shader sky and pointer honeycomb. Colors come from tokens. Measurements refresh on scroll and resize instead of in every frame.
- `decision-net` reads the placed HTML cards, draws edges and pulses, and animates the visible probabilities. Static numbers remain in the accessibility tree. Only inspector cards are buttons. `net-inspector` supports keyboard tracing, Escape and closing when focus leaves.
- `net-tabs` selects an industry and reflects it in `?industry=`, including Back and Forward. `net-modes` demonstrates the four operations using recorded answers.
- The terminal replay has its own Pause and Restart controls. Scripts follow the live reduced-motion preference, stop frames offscreen or in hidden tabs, and release listeners and observers on disconnect.
- `reveal-sections` batches section measurements before writes; everything starts visible, and reduced motion reveals remain readable.
- Existing copy, release lookup, model filtering and capture-view elements remain self-contained.

Animations and transitions use only transform and opacity. Colors, type and spacing come from design tokens.

## Tools

Tools run by hand, never during a build or page view. Provider credentials are unnecessary for browsing and testing.

- `record-cli-replay.mjs` drives `record-cli-replay.py` to refresh `src/data/replays/{wide,narrow,provenance}.json`. Wide runs also refresh `src/data/graphs/home.json`; `--tour` writes `tools/data/tui-tour.json`. The shared `recorded-provider.mjs` serves saved answers without live calls; its unit test verifies probabilities under remapped form IDs and rejects unrecorded calls.
- `render-tui-media.py` renders README and website media. Python recordings require `pyte` and a POSIX PTY; rendering also requires Pillow, FFmpeg and local fonts. See [README.md](README.md).
- `export-graph.mjs` and `build-hero.mjs` export existing stores. `readme-hero.mjs` renders the README SVGs, and `record-hero.sh` records new sessions using a configured provider.

The guide's meeting capture is committed data; no current tool regenerates it.

## Checks

`npm run check` runs formatting, lint, ShellCheck, types, unit tests, the static build, HTML and URL checks, JavaScript budgets, and browser tests with axe on every built page. Changed visual pages also require screenshot inspection at desktop and 390 px widths and an Astro dev-toolbar audit.
