// The instrument figure: a session graph with rulers, and a readout for the selected turn.
import { decode } from "./decode.js";

const NS = "http://www.w3.org/2000/svg";
const W = 220, H = 104, COL = 250, ROW = 168, OX = 44, OY = 44;
const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;

const el = (tag, attrs = {}, parent) => {
  const e = document.createElementNS(NS, tag);
  for (const k in attrs) e.setAttribute(k, attrs[k]);
  parent?.appendChild(e);
  return e;
};
const html = (tag, cls, text) => {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text != null) e.textContent = text;
  return e;
};
const cut = (t, k) => (t.length > k ? t.slice(0, k - 1) + "…" : t);
const fmt = (p) => (p == null ? "n/a" : p.toFixed(2));
export const tag = (i) => "T" + String(i + 1).padStart(2, "0");

/** Longest-path layers; children sit under the average of their parents. */
export function layout(nodes) {
  const level = [];
  nodes.forEach((n, i) => (level[i] = n.parents.length ? Math.max(...n.parents.map((p) => level[p] + 1)) : 0));
  const col = [], used = [];
  nodes.forEach((n, i) => {
    const want = n.parents.length ? n.parents.reduce((a, p) => a + col[p], 0) / n.parents.length : (used[level[i]]?.length ?? 0);
    const taken = (used[level[i]] ??= []);
    let c = Math.max(0, want);
    while (taken.some((t) => Math.abs(t - c) < 1)) c = Math.max(...taken.filter((t) => Math.abs(t - c) < 1)) + 1;
    taken.push(c);
    col[i] = c;
  });
  return nodes.map((_, i) => ({ x: col[i] * COL, y: level[i] * ROW }));
}

/** One-line answer for a node box. */
function summary(q) {
  if (q.refused) return ["Refused", ""];
  if (q.type === "choice") {
    const o = q.options.find((x) => x.id === q.selected);
    return ["▶ " + o.label, "P " + fmt(o.p)];
  }
  if (q.type === "noul") return ["P(yes) " + fmt(q.probability), ""];
  return [`score ${fmt(q.expected)} of ${q.levels.length - 1}`, ""];
}

export class Figure {
  constructor(root, { positions } = {}) {
    this.root = root;
    this.positions = positions;
    this.cur = -1;
    root.classList.add("slab");
    this.svg = el("svg", { class: "graph" }, root);
    this.read = html("div", "read");
    root.appendChild(this.read);
    if (!reduce) requestAnimationFrame((t) => this.flow(t));
  }

  /** Draw a graph. `changed` lists node indices whose result differs from the previous graph. */
  setGraph(nodes, changed = []) {
    const first = !this.nodes;
    this.nodes = nodes;
    this.pos = this.positions ?? layout(nodes);
    const w = Math.max(...this.pos.map((p) => p.x)) + W, h = Math.max(...this.pos.map((p) => p.y)) + H;
    const svg = this.svg;
    svg.innerHTML = "";
    svg.setAttribute("viewBox", `0 0 ${w + OX + 12} ${h + OY + 12}`);
    svg.setAttribute("aria-label", `Decision graph with ${nodes.length} turns`);
    for (let x = 0; x <= w; x += 10) {
      el("line", { class: "tick", x1: OX + x, x2: OX + x, y1: 14, y2: x % 100 ? 20 : 26 }, svg);
      if (!(x % 100)) el("text", { class: "tick-l", x: OX + x + 3, y: 12 }, svg).textContent = x;
    }
    for (let y = 0; y <= h; y += 10) {
      el("line", { class: "tick", y1: OY + y, y2: OY + y, x1: 14, x2: y % 100 ? 20 : 26 }, svg);
      if (!(y % 100)) el("text", { class: "tick-l", x: 2, y: OY + y - 3 }, svg).textContent = y;
    }
    this.view = el("g", { transform: `translate(${OX},${OY})` }, svg);
    this.xh = el("g", {}, this.view);
    this.edges = [];
    nodes.forEach((n, i) => {
      const b = this.pos[i];
      for (const p of n.parents) {
        const a = this.pos[p], x1 = a.x + W / 2, y1 = a.y + H, x2 = b.x + W / 2, y2 = b.y, my = (y1 + y2) / 2;
        const e = el("path", { class: "edge", d: `M${x1},${y1} C${x1},${my} ${x2},${my} ${x2},${y2}` }, this.view);
        Object.assign(e, { from: p, to: i, len: e.getTotalLength() });
        this.edges.push(e);
      }
    });
    this.dots = this.edges.flatMap((e) => [0, 0.5].map((o) => ({ e, o, dot: el("circle", { r: 2, class: "amb" }, this.view) })));
    this.boxes = nodes.map((n, i) => {
      const g = el("g", { class: "node", transform: `translate(${this.pos[i].x},${this.pos[i].y})`, tabindex: 0, role: "button" }, this.view);
      el("rect", { width: W, height: H }, g);
      const more = n.questions.length > 1 ? ` · +${n.questions.length - 1}` : "";
      el("text", { x: 10, y: 18, class: "id" }, g).textContent = `${tag(i)} · ${cut(n.session, 22)}${more}`;
      // Show the decision itself: the first choice question, if the turn asks one.
      const q = n.questions.find((x) => x.type === "choice") ?? n.questions[0];
      el("text", { x: 10, y: 40, class: "qq" }, g).textContent = cut(q.prompt, 38);
      const [ans, p] = summary(q);
      el("text", { x: 10, y: 70, class: "ans" }, g).textContent = cut(ans, 32);
      el("text", { x: 10, y: 91, class: "p" }, g).textContent = p;
      const pick = () => this.select(i);
      g.addEventListener("pointerenter", pick);
      g.addEventListener("click", pick);
      g.addEventListener("focus", pick);
      if (changed.includes(i)) requestAnimationFrame(() => g.classList.add("changed"));
      return g;
    });
    if (!first && this.cur >= 0) this.select(this.cur, { force: true, travel: false });
  }

  /** Send a token down each edge into nodes `from` onward, in order. */
  ripple(from) {
    this.nodes.forEach((_, i) => {
      if (i < from) return;
      this.edges.filter((e) => e.to === i).forEach((e) => setTimeout(() => this.travel(e), (i - from) * 200));
    });
  }

  travel(e, ms = 420) {
    if (reduce || !e.isConnected) return;
    const dot = el("circle", { r: 4.5, class: "tok" }, this.view), t0 = performance.now();
    const step = (now) => {
      const k = Math.min(1, (now - t0) / ms), pt = e.getPointAtLength(e.len * k);
      dot.setAttribute("cx", pt.x);
      dot.setAttribute("cy", pt.y);
      k < 1 ? requestAnimationFrame(step) : dot.remove();
    };
    requestAnimationFrame(step);
  }

  flow(now) {
    for (const d of this.dots ?? []) {
      const pt = d.e.getPointAtLength(d.e.len * ((now / 2600 + d.o) % 1));
      d.dot.setAttribute("cx", pt.x);
      d.dot.setAttribute("cy", pt.y);
    }
    requestAnimationFrame((t) => this.flow(t));
  }

  ancestors(i, seen = new Set()) {
    for (const p of this.nodes[i].parents) if (!seen.has(p)) { this.ancestors(p, seen); seen.add(p); }
    return seen;
  }

  select(i, { force = false, travel = true } = {}) {
    if (i === this.cur && !force) return;
    const fresh = i !== this.cur;
    this.cur = i;
    const n = this.nodes[i], anc = this.ancestors(i);
    this.svg.classList.add("focus");
    this.boxes.forEach((b, j) => {
      b.classList.toggle("on", j === i);
      b.classList.toggle("anc", anc.has(j));
    });
    this.edges.forEach((e) => e.classList.toggle("on", e.to === i));
    if (fresh && travel) this.edges.filter((e) => e.to === i).forEach((e) => this.travel(e));
    // Crosshair out to the rulers.
    this.xh.innerHTML = "";
    const cx = this.pos[i].x + W / 2, cy = this.pos[i].y + H / 2;
    el("line", { class: "xh", x1: cx, x2: cx, y1: -OY + 26, y2: this.pos[i].y }, this.xh);
    el("line", { class: "xh", y1: cy, y2: cy, x1: -OX + 26, x2: this.pos[i].x }, this.xh);
    el("text", { class: "xh-l", x: cx + 5, y: -OY + 36 }, this.xh).textContent = `x ${Math.round(cx)}`;
    el("text", { class: "xh-l", x: -OX + 28, y: cy - 4 }, this.xh).textContent = `y ${Math.round(cy)}`;
    this.readout(i, fresh);
  }

  readout(i, fresh) {
    const n = this.nodes[i], r = this.read;
    r.innerHTML = "";
    const meta = html("div", "meta", `${tag(i)} · ${n.session} · turn ${n.turn} · ${n.kind} · ${n.model}`);
    r.appendChild(meta);
    const facts = html("div", "facts");
    facts.append(html("span", "label", "New facts this turn"), html("p", "", n.state));
    r.appendChild(facts);
    for (const q of n.questions) r.appendChild(this.answer(q, n.questions.length > 1, fresh));
    const log = html("div", "log");
    log.appendChild(html("span", "label", "What the model saw"));
    for (const c of n.context) {
      const src = this.nodes[c.turn];
      if (!src) continue;
      const row = html("div", "row");
      const answers = src.questions.map((q) => summary(q)[0].replace("▶ ", "→ ")).join("; ");
      row.append(html("span", "src", `${tag(c.turn)} ${src.session}`), html("span", "", `${cut(src.state, 140)} ${answers}`));
      if (c.representation !== "full") row.appendChild(html("em", "", c.representation));
      row.addEventListener("pointerenter", () => this.boxes[c.turn].classList.add("src"));
      row.addEventListener("pointerleave", () => this.boxes[c.turn].classList.remove("src"));
      log.appendChild(row);
    }
    const now = html("div", "row new");
    now.append(html("span", "src", "+ new"), html("span", "", cut(n.state, 140)));
    log.appendChild(now);
    r.appendChild(log);
    const cmd = html("div", "cmd");
    const copy = html("button", "", "copy");
    copy.type = "button";
    copy.addEventListener("click", () => navigator.clipboard?.writeText(n.command));
    cmd.append(html("code", "", n.command), copy);
    r.appendChild(cmd);
  }

  answer(q, several, fresh) {
    const box = html("div", "answer" + (several ? " several" : ""));
    box.appendChild(html("div", "q", q.prompt));
    if (q.refused) {
      box.appendChild(html("p", "refused", "Refused: " + q.refused));
      return box;
    }
    const rows =
      q.type === "choice" ? q.options.map((o) => ({ label: o.label, p: o.p, sel: o.id === q.selected }))
      : q.type === "noul" ? [{ label: "yes", p: q.probability, sel: true }, { label: "no", p: 1 - q.probability }]
      : q.levels.map((l, k) => ({ label: `${k} ${l.label}`, p: l.p }));
    const value = q.type === "choice" ? rows.find((x) => x.sel).p : q.type === "noul" ? q.probability : q.expected;
    const caption = q.type === "choice" ? "P · " + rows.find((x) => x.sel).label : q.type === "noul" ? "P(yes)" : `expected score of ${q.levels.length - 1}`;
    const big = html("div", "big");
    const num = html("strong", "", fmt(value));
    const cap = html("span", "", caption);
    big.append(num, cap);
    box.appendChild(big);
    if (fresh && !reduce) {
      const t0 = performance.now();
      const tick = (now) => {
        const k = Math.min(1, (now - t0) / 500);
        num.textContent = fmt(value * (1 - (1 - k) ** 3));
        if (k < 1) requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
      decode(cap);
    }
    const strip = html("div", "strip");
    const legend = html("div", "legend");
    for (const row of rows) {
      const seg = html("i", row.sel ? "sel" : "");
      strip.appendChild(seg);
      requestAnimationFrame(() => requestAnimationFrame(() => (seg.style.flexBasis = (row.p ?? 0) * 100 + "%")));
      legend.append(html("i", "sw" + (row.sel ? " sel" : "")), html(row.sel ? "b" : "span", "", row.label), html("span", "", fmt(row.p)));
    }
    box.append(strip, legend);
    return box;
  }
}
