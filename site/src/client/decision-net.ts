import { prefersReducedMotion } from "./motion";
import { curveBetween, drawComb, drawEdge, drawLight, drawPulse } from "./net-canvas";
import type { Box, Weighted } from "./net-canvas";
import { readNode } from "./net-node";
import type { Node } from "./net-node";

/** Seconds between network layers, an edge pulse's travel, and the rest before a replay. */
const STEP = 1.25;
const PULSE = 0.75;
const HOLD = 3.2;
const ease = (x: number): number => 1 - (1 - Math.min(Math.max(x, 0), 1)) ** 3;
const reducedQuery = matchMedia("(prefers-reduced-motion: reduce)");

/**
 * A decision network that resolves step by step: edges light as answers arrive, finished steps
 * glow, and a honeycomb shows around the steps. Hovering or focusing a step traces what it saw;
 * clicking one reports `net:select`. The `mode` attribute demonstrates chain, branch, blend, or run.
 */
class DecisionNet extends HTMLElement {
  static observedAttributes = ["mode"];
  #controller: AbortController | undefined;
  #observers: (IntersectionObserver | ResizeObserver)[] = [];
  #canvas = document.createElement("canvas");
  #nodes: Node[] = [];
  #trace: Set<Node> | undefined;
  #start = 0;
  #frame: number | undefined;
  #visible = false;
  #box: Box & { dpr: number; narrow: boolean } = {
    w: 0,
    h: 0,
    left: 0,
    top: 0,
    dpr: 1,
    narrow: false,
  };
  #colors = { edge: "", lit: "", glow: "", line: "" };

  connectedCallback(): void {
    this.#controller = new AbortController();
    const { signal } = this.#controller;
    this.#canvas.setAttribute("aria-hidden", "true");
    Object.assign(this.#canvas.style, { position: "absolute", inset: "0", width: "100%" });
    this.#canvas.style.height = "100%";
    this.prepend(this.#canvas);
    const style = getComputedStyle(this);
    const read = (name: string): string => style.getPropertyValue(name).trim();
    this.#colors = {
      edge: read("--net-edge"),
      lit: read("--amber-soft"),
      glow: read("--net-glow"),
      line: read("--net-comb"),
    };
    this.#nodes = [...this.querySelectorAll<HTMLElement>(".node")].map((el) => readNode(el));
    this.dataset["live"] = "";
    for (const node of this.#nodes)
      if (node.el.dataset["inspectable"] !== undefined) this.#bind(node, signal);
    this.#observe(signal);
    this.#restart();
  }

  disconnectedCallback(): void {
    this.#controller?.abort();
    for (const observer of this.#observers) observer.disconnect();
    if (this.#frame !== undefined) cancelAnimationFrame(this.#frame);
    this.#frame = undefined;
    this.#canvas.remove();
    delete this.dataset["live"];
  }

  attributeChangedCallback(): void {
    if (this.#controller) this.#restart();
  }

  #bind(node: Node, signal: AbortSignal): void {
    const trace = (on: boolean): void => {
      this.#trace = on ? new Set([node, ...this.#ancestors(node)]) : undefined;
      this.#paint();
    };
    for (const [type, on] of [
      ["pointerenter", true],
      ["pointerleave", false],
      ["focus", true],
      ["blur", false],
    ] as const) {
      node.el.addEventListener(
        type,
        () => {
          trace(on);
        },
        { signal },
      );
    }
    node.el.addEventListener(
      "click",
      () => {
        this.dispatchEvent(new CustomEvent("net:select", { bubbles: true, detail: node.id }));
      },
      { signal },
    );
  }

  #observe(signal: AbortSignal): void {
    const resize = new ResizeObserver(() => {
      this.#measure();
    });
    resize.observe(this);
    const view = new IntersectionObserver(([entry]) => {
      this.#visible = entry?.isIntersecting ?? false;
      this.#schedule();
    });
    view.observe(this);
    this.#observers = [resize, view];
    document.addEventListener(
      "animation:change",
      () => {
        this.#schedule();
      },
      { signal },
    );
    document.addEventListener(
      "visibilitychange",
      () => {
        this.#schedule();
      },
      { signal },
    );
    reducedQuery.addEventListener(
      "change",
      () => {
        this.#restart();
      },
      { signal },
    );
  }

  get #mode(): string {
    return this.getAttribute("mode") ?? "chain";
  }

  #active(): Node[] {
    return this.#nodes.filter((node) => !node.branch || this.#mode === "branch");
  }

  #ancestors(node: Node, seen = new Set<Node>()): Set<Node> {
    for (const id of node.from) {
      const source = this.#active().find((candidate) => candidate.id === id);
      if (source && !seen.has(source)) {
        seen.add(source);
        this.#ancestors(source, seen);
      }
    }
    return seen;
  }

  #timing(node: Node): { on: number; done: number } {
    const same = this.#active().filter((other) => other.depth === node.depth);
    const stagger = this.#mode === "run" ? 0 : this.#mode === "chain" ? 0.45 : 0.18;
    const on = node.depth * STEP + same.indexOf(node) * stagger + (node.branch ? 0.4 : 0);
    return { on, done: on + 0.55 };
  }

  #restart(): void {
    this.#start = performance.now();
    for (const node of this.#nodes) {
      node.state = "";
      node.k = -1;
    }
    this.#measure();
    this.#schedule();
  }

  /** Read every position first, then size the canvas, so layout is computed once. */
  #measure(): void {
    const rect = this.getBoundingClientRect();
    for (const node of this.#nodes) {
      node.x = node.el.offsetLeft;
      node.y = node.el.offsetTop;
    }
    const dpr = Math.min(devicePixelRatio, 2);
    this.#box = {
      w: rect.width,
      h: rect.height,
      left: rect.left + scrollX,
      top: rect.top + scrollY,
      dpr,
      narrow: getComputedStyle(this).getPropertyValue("--vertical").trim() === "1",
    };
    this.#canvas.width = rect.width * dpr;
    this.#canvas.height = rect.height * dpr;
    this.#paint();
  }

  #schedule(): void {
    const run = this.#visible && !document.hidden && !prefersReducedMotion();
    if (run && this.#frame === undefined) {
      const loop = (): void => {
        this.#paint();
        this.#frame = requestAnimationFrame(loop);
      };
      this.#frame = requestAnimationFrame(loop);
    } else if (!run && this.#frame !== undefined) {
      cancelAnimationFrame(this.#frame);
      this.#frame = undefined;
      this.#paint();
    }
  }

  /** The time into the run; reduced motion shows the settled network. */
  #now(): number {
    if (prefersReducedMotion()) return Number.POSITIVE_INFINITY;
    const total = Math.max(...this.#active().map((node) => this.#timing(node).done)) + HOLD;
    const t = (performance.now() - this.#start) / 1000;
    if (t <= total) return t;
    this.#restart();
    return 0;
  }

  #focus(t: number): Set<Node> | undefined {
    if (this.#trace) return this.#trace;
    if (this.#mode !== "blend") return undefined;
    const target = this.#active().find((node) => node.from.length > 1);
    if (!target || t < this.#timing(target).done) return undefined;
    return new Set([target, ...this.#active().filter((node) => target.from.includes(node.id))]);
  }

  #paint(): void {
    const t = this.#now();
    const nodes = this.#active();
    const focus = this.#focus(t);
    for (const node of nodes) this.#show(node, t, focus);
    this.#draw(t, nodes, focus);
  }

  #show(node: Node, t: number, focus: Set<Node> | undefined): void {
    const { on, done } = this.#timing(node);
    const state = t < on ? (node.fact ? "idle" : "wait") : t < done ? "active" : "done";
    if (state !== node.state) {
      node.el.classList.remove("idle", "wait", "active", "done");
      node.el.classList.add(state);
      node.state = state;
    }
    node.el.classList.toggle("trace", focus?.has(node) === true);
    node.el.classList.toggle("dim", focus !== undefined && !focus.has(node));
    const k = node.fact ? 1 : ease((t - on) / 0.9);
    if (k === node.k) return;
    node.k = k;
    node.bars.forEach((bar, i) => {
      bar.style.setProperty("--p", String((node.ps[i] ?? 0) * k));
    });
    node.nums.forEach((num, i) => {
      num.textContent = ((node.ps[i] ?? 0) * k).toFixed(2);
    });
  }

  #draw(t: number, nodes: Node[], focus: Set<Node> | undefined): void {
    const ctx = this.#canvas.getContext("2d");
    if (!ctx) return;
    const box = this.#box;
    const { edge, lit, glow, line } = this.#colors;
    ctx.setTransform(box.dpr, 0, 0, box.dpr, 0, 0);
    ctx.clearRect(0, 0, box.w, box.h);
    const lights: Weighted[] = nodes
      .filter((node) => t >= this.#timing(node).on)
      .map((node) => [node.x, node.y, t < this.#timing(node).done ? 1 : 0.45]);
    const byId = new Map(nodes.map((node) => [node.id, node]));
    const pulses: Weighted[] = [];
    for (const to of nodes) {
      for (const from of to.from.map((id) => byId.get(id))) {
        if (!from) continue;
        const traced = focus?.has(from) === true && focus.has(to);
        const u = Math.min(Math.max((t - (this.#timing(to).on - PULSE)) / PULSE, 0), 1);
        const head = drawEdge(ctx, curveBetween(from, to, box.narrow), {
          u,
          traced,
          dimmed: focus !== undefined && !traced,
          dashed: to.branch,
          edge,
          lit,
        });
        if (head) pulses.push(head);
      }
    }
    const wag = Math.sin(performance.now() / 45) * 1.4;
    for (const pulse of pulses) {
      drawPulse(ctx, pulse, wag, { glow, lit });
      lights.push([pulse[0], pulse[1], 1.4]);
    }
    for (const light of lights) drawLight(ctx, light, glow);
    // The honeycomb sits behind everything drawn so far.
    ctx.globalCompositeOperation = "destination-over";
    drawComb(ctx, box, nodes, lights, { line, glow });
    ctx.globalCompositeOperation = "source-over";
  }
}

customElements.define("decision-net", DecisionNet);
