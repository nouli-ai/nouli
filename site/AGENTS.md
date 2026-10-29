# Website

This is a static site written in plain HTML, CSS, and JavaScript. Keep that structure, using native ES modules and the existing Vite build. See [README.md](README.md) for development commands and recording tools.

## Pages and content

- Write public pages for end users: explain what they can do, show usable commands, and describe the results. Keep development instructions and implementation details in repository documentation.
- Verify CLI commands and configuration against the current implementation and [reference documentation](../docs/). Identify illustrative records and policies, link published sources, and explain incorrect recorded answers where they occur.
- Add walkthroughs under `examples/SLUG/index.html`. Update both the home page and `examples/index.html` when adding or removing an example; Vite discovers example routes automatically.
- Use page-relative navigation links so the build works under a GitHub Pages subpath. Preserve the relative base in `vite.config.js`.
- Reuse `src/style.css` and the shared `Figure` in `src/figure.js`. Extend the existing paper and teal palette, typography, and responsive layouts through the shared styles.

## Recorded decisions

- Graph data in `src/data/` comes from saved Nouli sessions. When refreshing it, follow [Graph data](README.md#graph-data) and regenerate it through the export tools. Preserve recorded answers, probabilities, ancestry, and actual context; explain model errors in the walkthrough rather than changing the recorded result.
- The home-page controls select recorded variants. Keep normal browsing, building, and previewing independent of provider credentials and live API calls. Recording new sessions is a separate operation that can incur provider charges.
- A walkthrough's `body[data-graph]` selects its JSON file. Each `.step[data-node]` points to a node index in that file. After re-exporting sessions, update these mappings and check both scroll-driven selection and node-to-step navigation.
- Preserve answer meanings in graph readouts: choice selects a supplied option, noul is the probability of yes, and score is an expected rubric index that can be fractional. See [decision contracts](../docs/contracts/decisions.md) when changing result presentation.

## Validation

- Use Node.js 24 or later. From `site/`, run `npm ci` for a fresh checkout or dependency changes, then `npm run build` after changing site code, public content, or graph data. Keep dependency changes in both `package.json` and `package-lock.json`.
- For presentation or interaction changes, check the affected pages in a browser at desktop and narrow widths. Exercise graph selection, walkthrough scroll synchronization, keyboard controls, and reduced-motion behavior as applicable. Check navigation, clipping, and browser errors.
- Check the built site with `npm run preview` when changing routes or asset paths. Edit source files; leave generated `dist/` and installed `node_modules/` out of commits.
