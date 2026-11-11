# Nouli website

The site published at [nouli.ai](https://nouli.ai/), built with Astro. [ARCHITECTURE.md](ARCHITECTURE.md) describes its structure; [AGENTS.md](AGENTS.md) holds the rules for changing it.

## Develop

Requires Node.js 24 or later. From this directory:

```sh
npm ci
npx playwright install chromium   # browser for the end-to-end tests; ShellCheck must be on PATH
npm run dev                       # local server with live reload
npm run check                     # format, lint, types, unit tests, build, built-output checks, browser tests
```

`npm run format` applies formatting.

## Refresh recordings and media

Recording and export tools live in `tools/`; their existing Python, JavaScript, and shell are preserved. Run them from `site/`. Committed captures and replays keep normal builds independent of these tools and provider credentials.

Python recordings require Python 3 with `pyte` and a POSIX PTY. Media rendering also requires Pillow, FFmpeg, and local monospace fonts. `readme-hero.mjs` uses the `subset-font` development dependency. These are manual tool prerequisites, not website runtime requirements.

```sh
# Refresh the wide and narrow home replays.
node tools/record-cli-replay.mjs --nouli ../target/release/nouli --python /path/to/python
node tools/record-cli-replay.mjs --nouli ../target/release/nouli --python /path/to/python --columns 64
# Render committed replay cells as README media and website video.
python tools/render-tui-media.py --replay src/data/replays/wide.json \
  --output ../.github/readme --video public/media/tui-workshop.webm --ffmpeg /path/to/ffmpeg
# Regenerate the README's SVG diagrams.
node tools/readme-hero.mjs > ../.github/readme/hero.svg
node tools/readme-hero.mjs --recall > ../.github/readme/recall.svg
```

Home recordings write `src/data/replays/{wide,narrow,provenance}.json`, refresh `src/data/graphs/home.json` on wide runs, and keep `--tour` output in `tools/data/tui-tour.json`. `recorded-provider.mjs` returns saved answers without live provider calls.

The guide's meeting capture is committed data. Example data comes from recorded evaluation runs, with source and method notes in the typed content entries.

`record-hero.sh` uses a configured provider and can incur charges. To export an existing store without inference:

```sh
NOULI=../target/release/nouli node tools/export-graph.mjs --store STORE --all > src/data/graphs/NAME.json
NOULI=../target/release/nouli node tools/build-hero.mjs --store STORE > tools/data/hero.json
```
