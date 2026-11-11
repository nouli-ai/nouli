---
status: accepted
date: 2026-11-09
---

# Build the website with Astro and a small, verified toolchain

## Context

The website under `site/` is 21 hand-written HTML pages, one global stylesheet,
and untyped browser modules built with Vite. Each page repeats its head,
navigation, and footer. Model facts such as versions, licenses, and evaluation
links are typed into several pages. Walkthroughs embed terminal captures that
also exist as JSON. One class lays out, renders, animates, and reports the
decision graph. Nothing checks formatting, lint, types, markup, links, or
accessibility, and the tests run browser code against hand-built DOM fakes.
Three recording scripts are Python.

The site's copy will be rewritten after the rebuild, so this decision covers
the stack and the rules, not page content.

## Decision

- Build the site with **Astro 7** as a static site served from the root of
  `https://nouli.ai/`. Drop the relative asset base and the
  `github.io/nouli/` mirror it supported. Keep every published URL.
- Model content as typed Astro content collections. Compute layout, captures,
  catalog lists, structured data, and the sitemap at build time. Ship browser
  JavaScript only for interaction, as vanilla TypeScript custom elements; use no
  UI framework.
- Format every file type with **Oxfmt**, with Astro formatting enabled.
- Lint TypeScript with **Oxlint**, including type-aware rules.
- Type-check with **`astro check`** on Astro's strictest TypeScript preset.
- Test logic and components with **Vitest** (including Astro's Container API),
  and browser behavior with **Playwright** and **axe-core** on every page.
- Validate the built output with **html-validate** and TypeScript checks over
  `dist/` for links, the published URL inventory, and size budgets.
- Expose all of these through one `npm run check`, which CI runs unchanged.
- Keep the recording and export tools in their existing Python, JavaScript,
  and shell under `site/tools/`. They run by hand to refresh recordings and
  never during a build, so rewriting them would not change the site.
- Review site changes with a dedicated frontend reviewer agent, alongside the
  Rust reviewer.

### Trial evidence

A scratch Astro 7.4.8 project with planted defects showed:

- Oxfmt 0.78 with `"astro": true` and `@astrojs/compiler-rs` formats `.astro`
  files byte-for-byte as Prettier 3.9 with `prettier-plugin-astro` does, and a
  second pass makes no changes. Without the option it skips `.astro` files
  silently.
- Oxlint 1.93 applies syntactic rules (`eqeqeq`, `no-debugger`,
  `no-non-null-assertion`) to Astro frontmatter and `<script>` blocks. Its
  type-aware rules (`no-floating-promises`) run on `.ts` files only. Template
  markup, such as an image without `alt` or a clickable `div`, is not linted.
- `astro check` reports unused frontmatter variables.

Because type-aware lint stops at `.ts` files, logic lives in `.ts` modules and
Astro frontmatter only imports, reads props, and passes data. Because templates
are not linted, markup and accessibility are verified on the built and running
pages, which also covers the graph SVG that is generated in the browser.

## Considered alternatives

- **Keep plain HTML with Vite.** Every shared element and fact would stay
  copied across pages, with no schema to catch drift.
- **React or Next.js.** The site is static content with a few instruments; a
  component runtime adds weight without a matching need.
- **ESLint with `eslint-plugin-astro`.** It is the only template linter. On this
  architecture its rules add little: client-directive rules do not apply without
  framework islands, `set:html` can be rejected by a source search, and its
  accessibility rules see only static markup, which axe covers on the rendered
  page. A second linter beside Oxlint would split the rules. If the
  thin-frontmatter rule fails in practice, replace Oxlint with ESLint rather than
  running both.
- **Biome.** Its documentation marks Astro support as partial.
- **Prettier.** Its output matched Oxfmt's in the trial; Oxfmt keeps formatting
  and linting on one toolchain.
- **Vite+.** Astro runs its own development server and build, so Vite+ would
  only wrap tools the site can call directly.
- **Git hooks.** `npm run check` in CI and before each commit catches the same
  problems without another dependency.

## Consequences

Each fact and shared element has one source, and invalid content fails the
build. Pages work without JavaScript, and interactive parts clean up after
themselves. The site gains a Node toolchain whose versions are pinned in its
lockfile, and the Oxfmt Astro option must stay enabled for `.astro` files to be
formatted. [site/DESIGN.md](../../site/DESIGN.md) records the visual design.
