// Generate the README banner and the preserved recorded recall diagram.
//
//   node scripts/readme-hero.mjs > ../.github/readme/hero.svg
//   node scripts/readme-hero.mjs --recall > ../.github/readme/recall.svg
//
// Both images embed subsets of the checked-in website fonts. The recall diagram
// reads docs/recall-example.md and refuses answers or ancestry its layout cannot draw.
import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const P = "#f4f1eb", INK = "#1d2a28", T = "#0b6e63", SOFT = "#4f5a57", TB = "#5cc8b6";
export const RECALL_SOURCE = resolve(import.meta.dirname, "../../docs/recall-example.md");
const FONT_DIR = resolve(import.meta.dirname, "../public/fonts");
const BEE = resolve(import.meta.dirname, "../public/brand/bee.svg");

/** Read the nodes and edges of the preserved example's Mermaid block. */
export function readGraph(readme) {
  const block = readme.match(/```mermaid\n([\s\S]*?)```/)?.[1];
  if (!block) throw new Error("docs/recall-example.md has no mermaid block");
  const nodes = [], seen = new Map();
  for (const [, key, label] of block.matchAll(/^\s*(t\d+)\["(.*)"\]$/gm)) {
    const [head, question, answers] = label.split("<br/><br/>");
    const session = head.match(/^<b>(\S+) · /)[1];
    const turn = (seen.get(session) ?? 0) + 1;
    seen.set(session, turn);
    const options = answers.split("<br/>").map((a) => {
      const selected = a.startsWith("<b>▶ ");
      const text = a.replace(/^<b>▶ /, "").replace(/<\/b>$/, "");
      const yes = text.match(/^P\(yes\)=(.*)$/);
      return yes ? { label: "yes", p: +yes[1], selected } : { label: text.replace(/ · [\d.]+$/, ""), p: +text.match(/ · ([\d.]+)$/)[1], selected };
    });
    nodes.push({ key, session, turn, question, options, parents: [] });
  }
  for (const [, from, to] of block.matchAll(/^\s*(t\d+) --> (t\d+)$/gm)) nodes.find((n) => n.key === to).parents.push(nodes.find((n) => n.key === from));
  return nodes;
}

// The graph this layout draws: each node's expected parents and answer, and its label on the map.
const EXPECTED = {
  "recall-scope 1": { parents: [], answer: "Match the product and both lot-code conditions", short: "Match product + both lot codes" },
  "recall-stock 1": { parents: [], answer: "yes", short: "All jars matched to a lot?" },
  "recall-stock-plan 1": { parents: ["recall-scope 1", "recall-stock 1"], answer: "Quarantine matching jars and hold unreadable codes for identification", short: "Quarantine matches, hold unreadable" },
  "recall-customers 1": { parents: [], answer: "Contact Summit", short: "Contact Summit first" },
  "recall-customers 2": { parents: ["recall-customers 1"], answer: "Escalate the 12 sold boxes and verify downstream notification", short: "Escalate the 12 sold boxes" },
  "recall-outreach-updated 1": { parents: ["recall-scope 1", "recall-customers 2"], answer: "Escalate reported sales to the recall lead and verify downstream notification", short: "Escalate reported sales to the lead" },
  "recall-response-updated 1": { parents: ["recall-stock-plan 1", "recall-outreach-updated 1"], answer: "Coordinate warehouse holds and escalation of sales to end customers", short: "Coordinate warehouse holds|and escalation of sales|to end customers" },
};

/** Check the graph against the layout and return each node's label, probability, and ancestors. */
export function placeGraph(nodes) {
  const id = (n) => `${n.session} ${n.turn}`;
  const got = Object.fromEntries(nodes.map((n) => [id(n), n]));
  if (Object.keys(got).sort().join() !== Object.keys(EXPECTED).sort().join()) throw new Error(`Recall graph has ${Object.keys(got).join(", ")}; update the layout in readme-hero.mjs`);
  const ancestors = (n, out = new Set()) => { for (const p of n.parents) { out.add(p); ancestors(p, out); } return out; };
  const place = {};
  for (const [k, want] of Object.entries(EXPECTED)) {
    const n = got[k], parents = n.parents.map(id).sort().join();
    if (parents !== [...want.parents].sort().join()) throw new Error(`${k} has parents ${parents || "none"}; update the layout in readme-hero.mjs`);
    // A yes/no question shows P(yes) whichever way it went; a choice shows its selected answer.
    const shown = want.answer === "yes" ? n.options[0] : n.options.find((o) => o.selected);
    if (shown?.label !== want.answer) throw new Error(`${k} answered "${shown?.label}"; update its label in readme-hero.mjs`);
    const kind = (a) => (a.parents.length > 1 ? "blend" : { "recall-scope": T, "recall-stock": INK, "recall-customers": TB }[a.session]);
    place[k] = { short: want.short, p: shown.p.toFixed(2), behind: [...ancestors(n)].sort((a, b) => nodes.indexOf(a) - nodes.indexOf(b)).map(kind), direct: n.parents.length };
  }
  const depth = (n) => (n.parents.length > 1 ? 1 + Math.max(...n.parents.map(depth)) : n.parents.length ? depth(n.parents[0]) : 0);
  return { place, sources: nodes.filter((n) => !n.parents.length).length, layers: Math.max(...nodes.map(depth)), decisions: nodes.length };
}

// Offset a polyline sideways by d, keeping corners mitred, so parallel tracks stay parallel.
function off(pts, d) {
  const nrm = (a, b) => { const dx = b[0] - a[0], dy = b[1] - a[1], l = Math.hypot(dx, dy); return [-dy / l, dx / l]; };
  return pts.map((p, i) => {
    const n1 = i > 0 ? nrm(pts[i - 1], p) : null, n2 = i < pts.length - 1 ? nrm(p, pts[i + 1]) : null;
    if (!n1) return [p[0] + n2[0] * d, p[1] + n2[1] * d];
    if (!n2) return [p[0] + n1[0] * d, p[1] + n1[1] * d];
    const mx = n1[0] + n2[0], my = n1[1] + n2[1], l = Math.hypot(mx, my), k = d / ((mx * n1[0] + my * n1[1]) / l);
    return [p[0] + (mx / l) * k, p[1] + (my / l) * k];
  });
}

const esc = (s) => String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
// Faces: display and names in Archivo, everything else in Martian Mono.
const FACES = {
  disp: { file: "archivo-latin-variable.woff2", family: "nd", weight: 900, axes: { wdth: 62, wght: 900 } },
  cond: { file: "archivo-latin-variable.woff2", family: "nc", weight: 800, axes: { wdth: 75, wght: 800 } },
  mono: { file: "martian-mono-latin-variable.woff2", family: "nm", weight: 400, axes: { wdth: 87, wght: 400 } },
  bold: { file: "martian-mono-latin-variable.woff2", family: "nm", weight: 700, axes: { wdth: 87, wght: 700 } },
};

/** Lay out the recorded diagram and collect the glyphs each face draws. */
export function drawRecall(graph) {
  const { place, sources, layers, decisions } = graph;
  const used = Object.fromEntries(Object.keys(FACES).map((f) => [f, new Set()]));
  const text = (x, y, t, o = {}) => {
    const face = o.face || (o.w === 700 ? "bold" : "mono");
    for (const c of t) used[face].add(c);
    const fam = FACES[face];
    return `<text x="${x}" y="${y}"${o.a ? ` text-anchor="${o.a}"` : ""} font-family="${fam.family}" font-weight="${fam.weight}" font-size="${o.s || 10.5}" fill="${o.f || SOFT}"${o.ls ? ` letter-spacing="${o.ls}"` : ""}>${esc(t)}</text>`;
  };
  const name = (x, y, t, a) => text(x, y, t, { face: "cond", s: 16, f: INK, a });
  const p = (k) => place[k].p, label = (k) => `${place[k].short} · ${p(k)}`;
  const H = 822;
  let s = "";

  // Masthead.
  s += text(34, 58, "NOULI", { face: "disp", s: 30, f: INK, ls: 0.4 }) + text(966, 55, "CLI + Rust library · MIT", { a: "end", s: 12 });
  s += text(26, 194, "EVERY DECISION", { face: "disp", s: 140, f: INK }) + text(26, 318, "HAS A HISTORY.", { face: "disp", s: 140, f: T });
  const sub1 = ["Nouli is a ledger-backed decision network", " that records AI decisions"];
  for (const c of sub1[0]) used.bold.add(c);
  for (const c of sub1[1]) used.mono.add(c);
  s += `<text x="34" y="362" font-family="nm" font-weight="400" font-size="14" fill="${INK}"><tspan font-weight="700" fill="${T}">${esc(sub1[0])}</tspan>${esc(sub1[1])}</text>`;
  s += text(34, 384, "and connects them to the facts and earlier answers they use.", { s: 14, f: INK });
  s += `<line x1="34" x2="966" y1="410" y2="410" stroke="${INK}" stroke-width="2"/>`;
  s += text(34, 434, "THE RECALL GRAPH · nouli view recall-response-updated · recorded", { s: 11, f: T });
  s += text(966, 434, `${sources} sources · ${layers} layers of blends · ${decisions} decisions`, { a: "end" });

  // Lines: scope forks into both layer-1 blends, and their two bundles meet in the layer-2 blend.
  const routes = [
    { c: INK, d: -6, p: [[90, 530], [720, 530], [810, 620], [830, 620]] },
    { c: T, d: 6, p: [[90, 608], [482, 608], [560, 530], [720, 530], [810, 620], [830, 620]] },
    { c: T, d: -6, p: [[90, 632], [482, 632], [560, 710], [720, 710], [810, 620], [830, 620]] },
    { c: TB, d: 6, edge: 1, p: [[90, 710], [720, 710], [810, 620], [830, 620]] },
  ];
  const pts = (r) => off(r.p, r.d).map((q) => q.map((v) => v.toFixed(1)).join(",")).join(" ");
  const line = (r, c, w) => `<polyline points="${pts(r)}" fill="none" stroke="${c}" stroke-width="${w}" stroke-linejoin="round"/>`;
  s += routes.map((r) => line(r, P, 13)).join("");
  for (const r of routes) s += (r.edge ? line(r, INK, 10) : "") + line(r, r.c, 8);
  const st = `fill="${P}" stroke="${INK}" stroke-width="3.5"`;
  s += `<circle cx="90" cy="524" r="9" ${st}/><rect x="80" y="604" width="20" height="32" rx="10" ${st}/><circle cx="90" cy="716" r="9" ${st}/><circle cx="360" cy="716" r="9" ${st}/>`;
  s += `<rect x="548" y="512" width="24" height="36" rx="12" ${st}/><rect x="548" y="692" width="24" height="36" rx="12" ${st}/>`;
  s += `<rect x="812" y="597" width="90" height="46" rx="23" fill="${P}" stroke="${T}" stroke-width="4"/>` + text(857, 624, "LATEST", { a: "middle", s: 10, w: 700, f: T });

  // Layers across the top, then the stations.
  for (const [x0, x1, t] of [[40, 420, "SOURCES · three sessions"], [440, 690, "LAYER 1 · blends of sources"], [720, 966, "LAYER 2 · a blend of blends"]]) {
    s += text(x0, 458, t, { s: 9.5, w: 700, f: T }) + `<line x1="${x0}" x2="${x1}" y1="467" y2="467" stroke="${INK}" stroke-opacity=".25"/>`;
  }
  s += name(70, 497, "RECALL-STOCK") + text(70, 512, `${place["recall-stock 1"].short} P(yes) ${p("recall-stock 1")}`);
  s += name(108, 588, "RECALL-SCOPE") + text(108, 603, label("recall-scope 1"));
  s += name(70, 744, "RECALL-CUSTOMERS") + text(70, 759, label("recall-customers 1"));
  s += name(360, 682, "CUSTOMERS · TURN 2", "middle") + text(360, 697, label("recall-customers 2"), { a: "middle" });
  s += `<rect x="112" y="685" width="112" height="18" rx="9" fill="${P}" stroke="${T}"/>` + text(168, 698, "+ 12 boxes sold", { a: "middle", s: 9.5, f: T });
  s += name(560, 488, "RECALL-STOCK-PLAN", "middle") + text(560, 503, label("recall-stock-plan 1"), { a: "middle" });
  s += name(560, 744, "RECALL-OUTREACH-UPDATED", "middle") + text(560, 759, label("recall-outreach-updated 1"), { a: "middle" });
  const [r1, r2, r3] = place["recall-response-updated 1"].short.split("|");
  s += name(800, 528, "RECALL-RESPONSE-") + name(800, 544, "UPDATED") + text(800, 560, r1) + text(800, 574, r2) + text(800, 588, `${r3} · ${p("recall-response-updated 1")}`, { f: INK, w: 700 });

  // A square per earlier decision behind each blend: filled for a source turn, hollow for a blend.
  const squares = (x, y, kinds) => kinds.map((k, i) => (k === "blend"
    ? `<rect x="${x + i * 11 + 0.75}" y="${y + 0.75}" width="7.5" height="7.5" fill="${P}" stroke="${INK}" stroke-width="1.5"/>`
    : `<rect x="${x + i * 11}" y="${y}" width="9" height="9" fill="${k}"${k === TB ? ` stroke="${INK}"` : ""}/>`)).join("");
  const plan = place["recall-stock-plan 1"], outreach = place["recall-outreach-updated 1"], response = place["recall-response-updated 1"];
  s += text(592, 566, `built on ${plan.behind.length}`, { s: 9 }) + squares(652, 558, plan.behind);
  s += text(592, 683, `built on ${outreach.behind.length}`, { s: 9 }) + squares(652, 675, outreach.behind);
  s += squares(824, 656, response.behind);
  s += text(966, 682, `built on ${response.behind.length} earlier decisions:`, { a: "end", s: 9.5, f: INK });
  s += text(966, 696, `${response.direct} directly, ${response.behind.length - response.direct} behind them`, { a: "end", s: 9.5, f: INK, w: 700 });

  // Legend.
  s += `<line x1="34" x2="966" y1="782" y2="782" stroke="${INK}" stroke-opacity=".2"/>`;
  let cx = 34;
  const lab = (t) => { s += text(cx, 806, t, { s: 9.5 }); cx += t.length * 6.6 + 16; };
  for (const [c, t, edge] of [[T, "scope"], [INK, "stock"], [TB, "customers", 1]]) {
    s += (edge ? `<line x1="${cx}" x2="${cx + 24}" y1="802" y2="802" stroke="${INK}" stroke-width="9"/>` : "") + `<line x1="${cx}" x2="${cx + 24}" y1="802" y2="802" stroke="${c}" stroke-width="7"/>`;
    cx += 30; lab(t);
  }
  s += `<circle cx="${cx + 6}" cy="802" r="6" ${st}/>`; cx += 18; lab("turn");
  s += `<rect x="${cx}" y="794" width="10" height="16" rx="5" ${st}/>`; cx += 16; lab("blend");
  s += `<rect x="${cx}" y="796" width="9" height="9" fill="${T}"/><rect x="${cx + 13.75}" y="796.75" width="7.5" height="7.5" fill="${P}" stroke="${INK}" stroke-width="1.5"/>`; cx += 30;
  lab("an earlier decision behind a blend: filled, a source turn; hollow, a blend");
  return { body: s, used, H };
}

/** Render the preserved recall diagram with its recorded answers and ancestry. */
export async function recallSvg(graph, fonts = FONT_DIR) {
  const { body, used, H } = drawRecall(graph);
  const style = await fontStyles(used, fonts);
  const alt = "Every decision has a history. Nouli is a ledger-backed decision network. The recorded recall example as a line map: three source sessions blend in two layers into recall-response-updated, which is built on six earlier decisions.";
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 ${H}" width="1000" height="${H}" role="img" aria-labelledby="title">
<title id="title">${esc(alt)}</title>
<!-- Generated by the recall option of site/scripts/readme-hero.mjs from docs/recall-example.md. Fonts: Archivo, Copyright 2020 The Archivo Project Authors, and Martian Mono, Copyright 2021 The Martian Mono Project Authors, both under the SIL Open Font License 1.1; subset and embedded. -->
<defs><style>${style}</style><filter id="grain" x="0" y="0" width="100%" height="100%"><feTurbulence type="fractalNoise" baseFrequency=".9" numOctaves="2" stitchTiles="stitch"/><feColorMatrix values="0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 .55 0"/></filter></defs>
<rect width="1000" height="${H}" fill="${P}"/>
${body}
<rect width="1000" height="${H}" filter="url(#grain)" opacity=".2" style="mix-blend-mode:multiply"/>
</svg>
`;
}

/** Embed only the glyphs used by each face; SVG images do not fetch website fonts. */
async function fontStyles(used, fonts = FONT_DIR) {
  const { default: subsetFont } = await import("subset-font");
  let style = "";
  for (const [face, f] of Object.entries(FACES)) {
    const chars = [...used[face]].sort().join("");
    if (!chars) continue;
    const font = await subsetFont(readFileSync(join(fonts, f.file)), chars, {
      targetFormat: "woff2",
      variationAxes: f.axes,
    });
    style += `@font-face{font-family:${f.family};font-weight:${f.weight};src:url(data:font/woff2;base64,${font.toString("base64")}) format("woff2")}`;
  }
  return style;
}

/** A compact banner in the website's paper, teal, Archivo, and bee identity. */
export async function heroSvg() {
  const lines = ["EVERY DECISION", "HAS A HISTORY."];
  const used = Object.fromEntries(Object.keys(FACES).map((f) => [f, new Set()]));
  for (const c of "NOULI" + lines.join("")) used.disp.add(c);
  for (const c of "nouli.ai") used.mono.add(c);
  const style = await fontStyles(used);
  const bee = readFileSync(BEE, "utf8")
    .replace(/<svg\s+/, '<svg x="28" y="12" width="23" height="27" ')
    .replace(/\s+(?:role|aria-labelledby)="[^"]*"/g, "")
    .replace(/<title[^>]*>[\s\S]*?<\/title>/, "");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1000 260" width="1000" height="260" role="img" aria-labelledby="title">
<title id="title">Nouli. Every decision has a history.</title>
<!-- Generated by site/scripts/readme-hero.mjs from the website's fonts and bee. Archivo: Copyright 2020 The Archivo Project Authors. Martian Mono: Copyright 2021 The Martian Mono Project Authors. Both fonts are subset and embedded under SIL OFL 1.1; notices are in site/public/fonts/. -->
<defs><style>${style}</style></defs>
<rect width="1000" height="260" fill="${P}"/>
${bee}
<text x="59" y="33" font-family="nd" font-weight="900" font-size="27" fill="${INK}">NOULI</text>
<text x="972" y="31" text-anchor="end" font-family="nm" font-weight="400" font-size="12" fill="${T}">nouli.ai</text>
<text x="24" y="133" font-family="nd" font-weight="900" font-size="124" fill="${INK}">${lines[0]}</text>
<text x="24" y="230" font-family="nd" font-weight="900" font-size="124" fill="${T}">${lines[1]}</text>
</svg>
`;
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  process.stdout.write(process.argv.includes("--recall")
    ? await recallSvg(placeGraph(readGraph(readFileSync(RECALL_SOURCE, "utf8"))))
    : await heroSvg());
}
