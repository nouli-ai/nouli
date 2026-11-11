# Design language: Nouli website

How the site looks and behaves. Engineering rules are in [AGENTS.md](AGENTS.md); code structure is in [ARCHITECTURE.md](ARCHITECTURE.md).

## Thesis

**A calm, luminous page built around living decision networks:** a soft violet sky around a precise, real product, with nothing else competing for attention.

## Vocabulary

Each term is a dial. "More" and "less" describe what changes when a page turns it up or down.

### Luminous

Light behaves like atmosphere: one moving sky, glow and haze instead of flat fills.

- **Apply:** one sky is fixed to the viewport. The page header, network panels and closing panel are windows onto it, each showing the slice of sky behind it. The page between them stays light.
- **More:** brighter glow, a wider gradient range. **Less:** glow only on the active step.

### Quiet type

Large sans-serif headlines in regular weight, sentence case, tight negative tracking. Confidence comes from size and space, not weight.

- **Apply:** Archivo at normal width, weight 400–500, for text; Martian Mono for labels, readouts, probabilities and code. Body text is at least 16px.
- **More:** bigger and lighter type. **Less:** smaller and medium weight.

### Real product

Show Nouli itself: decision networks with their probabilities, and the terminal. No abstract stand-ins.

- **Apply:** every question shows its whole answer distribution, each option with its bar and probability and the picked one highlighted. A bare "Yes 0.92" is never shown. Readouts use mono with tabular figures. The terminal replay is a recording of the real app.
- **More:** larger panels, more of the interface. **Less:** one focused step and its readout.

### Living network

Networks resolve step by step, light their edges as answers flow, and open a step's inputs on hover, focus or click.

- **Apply:** motion lives in the networks and the sky. Elsewhere it is limited to short reveals.
- **More:** denser networks and pointer response. **Less:** a static network that animates only on interaction.

### Soft geometry

Rounded panels (16–28px), pill buttons and a floating pill navigation.

- **Apply:** the primary action is an amber pill; the secondary is an outlined pill. Cards are white, rounded, and have hairline borders. The navigation is fixed: transparent over the page header, then after a short scroll it narrows into a frosted pill that tints what is behind it, with white text over the sky and ink text over light sections. On phones the floating pill keeps only the mark and Install.
- **More:** larger radii, floating layers. **Less:** hairline rectangles, for dense tables.

### Air

Generous whitespace, one idea per screen, and section headings beside or above short bodies.

- **Apply:** about one viewport per home section on desktop. Reference pages (guide, models) run tighter.
- **More:** taller sections, fewer elements. **Less:** tighter spacing.

## Hive theme

The bee logo and swarm decision-making give the site its motif. It stays businesslike: no cartoon bees.

- The sky is a shader field from deep indigo through violet and rose to coral.
- A honeycomb grid, fixed to the page, shows faintly around network steps and around the pointer over the sky. Network steps stay still under the pointer.
- Amber marks decisions: `--amber` on panels and buttons, `--amber-soft` on the sky, `--amber-ink` for picked answers and links on light ground.
- Pulses along network edges are abstract amber capsules.
- The home page explains the bee once: "A honeybee swarm chooses its new home from many small scout reports that build into one decision."

Colors live in `src/styles/tokens.css`; the shader and canvases read them at runtime.

## Page patterns

- **Page header:** a rounded sky panel with the navigation, a mono kicker or breadcrumb, a large quiet headline and a short subheadline (`PageHead`). The home hero adds install commands and a network.
- **Sections:** a large light heading, optionally beside a one-line lede, then the content.
- **Network panel:** a sky panel with a mono caption bar (session name on the left, mode or hint on the right) and the network inside.
- **Cards:** white, rounded, hairline border; amber border on hover.
- **Footer:** one line of small links.

## Networks

- **Home page:** every network is invented and labeled "illustrative". The real data lives on the examples pages, and the home page links there.
- **Invented networks** turn on judgment a fixed rule can't make: intent, tone, credibility, fit. Each reads more than one source, and code handles the exact parts: limits, dates, scores, routing. Shapes vary: questions that read earlier answers, side links, more than one output. Keep them to four or five columns, at most four options or three checks per card, and short facts.
- **Examples pages:** one real case per public dataset, with every option and recorded probability. Rules and computed steps are labeled as code. Each page states the routine work, measured stats (cases run, model decisions per case, median time, cost per 1,000 cases), where the cases were routed, and which steps a model decides. Small print names the source, license, model and sample. Accuracy comparisons stay out of the page body.
- **Content:** no graphic cases (deaths, amputations or vivid injuries), and no medical cases on the home page.

## Copy

- Short, concrete sentences in American English. Caveats go in the small print.
- The product is free: buttons say "Install", never "Install free".
- Never use "ledger".

## Motion and accessibility

- Text meets WCAG AA. Every effect works by keyboard and touch, pauses offscreen and in hidden tabs, and has a reduced-motion form that shows the settled state.
- Recorded numbers are available to assistive technology as static text; animated counters are hidden from it.
- Only interactive steps are buttons. Focus traces a step's inputs; Enter opens its details; Escape closes them and returns focus.
- The terminal replay has Pause and Restart controls.
- Without JavaScript, every network, answer distribution, install command and the terminal's final screen are still present.
- Narrow layouts put at most two cards side by side, and one on phones. Labels wrap rather than truncate.

## Constraints

- Astro static site with vanilla TypeScript custom elements. Third-party scripts and motion libraries need an ADR.
- Fonts are self-hosted.
- Imagery is limited to the networks, the logo and recorded product views.
- The logo is the existing bee (`public/brand/bee.svg`) with the wordmark.
