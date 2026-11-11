# Website

The site at https://nouli.ai/ is built with Astro under [ADR 0026](../docs/adr/0026-astro-site-stack.md). Follow [DESIGN.md](DESIGN.md) for visual and interaction decisions.

## Pages work without JavaScript

- Put every explanation, navigation link, command, model fact, and network step in the built HTML. Scripts enhance pages that already read correctly.
- Use native elements first: `<a>` for navigation, `<button>` for actions, `<details>`, `<dialog>`, and form controls before ARIA roles.

## Compute at build time

- Compute everything that does not depend on the visitor at build time: graph layout, terminal captures, catalog cards, related-model lists, counts, structured data, and the sitemap.
- Ship browser JavaScript only for interaction. Each page has a JavaScript budget enforced by a test over `dist/`.
- Load large recordings only on the pages that render them.

## Content is typed data with one source

- Keep models, examples, and guide pages in Astro content collections with Zod schemas. Pages are templates over that data.
- State each fact once: a version, parameter count, license, command, URL, or model count. Derive every other appearance from that source.
- Let schema errors fail the build.
- Write every published page through the shared layout, which requires a title and a description and produces the canonical URL, Open Graph tags, and structured data.

## Logic lives in TypeScript modules

- Keep Astro frontmatter thin: it imports, reads props, calls module functions, and passes data to markup. Type-aware lint covers `.ts` files only, so logic in frontmatter goes unchecked.
- Write logic as pure functions in `.ts` modules, unit-tested with Vitest.

## Interactive parts are self-contained

- Build each interactive part as a vanilla TypeScript custom element. It queries only inside its own element and reports changes with `CustomEvent`s that others listen for.
- Release everything it starts when it disconnects: listeners through one `AbortController`, animation frames, timers, and observers. Pause animation while the element is offscreen or the tab is hidden.
- Resolve required elements through a helper that throws with the selector it missed.

## State and URLs

- Reflect shareable state in the URL, such as catalog filters and the selected industry. Back and Forward restore it.
- Treat published URLs as a public contract, including `/install.sh` and `/llms.txt`. The URL inventory test lists them; a removed URL needs a redirect page and an updated inventory.
- Use root-relative URLs with trailing slashes.

## Styling

- Use colors, type, and spacing only through the design tokens. Scope styles to their component; global styles sit in cascade layers for reset, tokens, base, and prose, and component styles stay unlayered so they take precedence.
- Animate `transform` and `opacity`, and name each transitioned property.
- Express reduced motion in CSS with `prefers-reduced-motion`, and have scripts follow the live media query.
- Show focus with `:focus-visible`, and set numeric columns in `tabular-nums`.

## Accessibility

- Meet WCAG 2.2 AA, checked by axe on every page and by the reviewer against [Vercel's Web Interface Guidelines](https://github.com/vercel-labs/web-interface-guidelines).
- Give every pointer interaction a keyboard equivalent, and hit targets of at least 24 px.
- Give each page a skip link and one ordered heading outline. Carry status in text or shape as well as color, and announce confirmations such as "copied" through a polite live region.
- Give motion that runs longer than five seconds, such as the terminal replay, a pause control.

## Performance

- Meet Core Web Vitals budgets: LCP under 2.5 s, CLS under 0.1, INP under 200 ms.
- Serve images through `astro:assets` with explicit dimensions, and lazy-load everything below the first screen.
- Batch layout reads before writes; cache measurements and refresh them on resize.

## Security

- Set a content security policy through Astro's CSP support, and render content through Astro's escaping. `set:html` appears only in `JsonLd.astro`, which emits structured data with `<` escaped; a unit test enforces this.
- Add third-party scripts, fonts, or trackers only through an ADR.
- Treat `public/install.sh` as product code: keep it covered by its tests and ShellCheck.

## Recorded decisions

- Graph and capture data in `src/data/` comes from saved Nouli sessions through the export tools; see [Tools](ARCHITECTURE.md#tools). Preserve recorded answers, probabilities, ancestry, and context, and explain model errors in the page text.
- Keep browsing, building, and testing independent of provider credentials and live model calls. Recording new sessions is a separate step that can incur provider charges.
- Preserve answer meanings in readouts: choice selects a supplied option, noul is the probability of yes, and score is an expected rubric index that can be fractional. See [decision contracts](../docs/contracts/decisions.md).
- Verify every CLI command and configuration shown on a page against the current implementation and [reference documentation](../docs/).

## Tooling

- Format with Oxfmt; keep its `astro` option enabled so `.astro` files are formatted.
- Astro drops a line break that sits next to a tag, so text that starts a line right after an inline element such as a link loses its space. Write `{" "}` at the end of the previous line, or keep the words on one line.
- Lint with Oxlint, type-aware rules on, warnings denied. Type-check with `astro check` on the strictest preset.
- Test with Vitest for logic and components and Playwright with axe for browser behavior. Write tests and new scripts in TypeScript. The recording tools in `tools/` keep their existing languages; see [ARCHITECTURE.md](ARCHITECTURE.md#tools).
- Add dependencies with `npm install`, commit `package-lock.json`, and state each new dependency's reason in the pull request.

## Validation

Use Node.js 24 or later. Run `npm run check` from `site/`; CI runs the same command. Before committing a visual or interaction change, screenshot the affected pages with Playwright at desktop and 390 px widths and inspect them. Exercise keyboard use and reduced motion where the change touches them.

## Review

After changing site files, have the frontend reviewer agent (`frontend-reviewer` in Claude Code, `frontend_reviewer` in Codex) review the change, and address its findings before committing.
