import { isQuestion, shownOptions, sourcesOf } from "./networks";
import type { Network, Step } from "./networks";

export interface Placed {
  readonly step: Step;
  readonly depth: number;
  readonly branch: boolean;
  /** Position as fractions of the panel: across (x) and down (y), wide, narrow and stacked. */
  readonly wide: Point;
  readonly narrow: Point;
  readonly stack: Point;
}

export interface Point {
  readonly x: number;
  readonly y: number;
}

export interface Edge {
  readonly from: string;
  readonly to: string;
  readonly branch: boolean;
}

export interface Layout {
  readonly placed: readonly Placed[];
  readonly edges: readonly Edge[];
  /** Columns in the wide layout, which is rows in the narrow one. */
  readonly depths: number;
  /** The most steps sharing one depth. */
  readonly breadth: number;
  /** Panel heights in pixels: the narrow one always fits; the wide one fits the tallest column. */
  readonly height: { wide: number; narrow: number; stack: number };
}

/** Space between stacked cards, and above and below them, in pixels. */
const GAP = 18;
const EDGE = 36;
/** Narrow layouts put at most this many cards side by side; stacked layouts put one. */
const ACROSS = 2;

/** Lines a card's text takes; wide cards fit about 26 characters to a line. */
/** Conservative word wrapping, including breaks inside unusually long words. */
const lines = (text: string, width = 20): number => {
  let count = 1,
    used = 0;
  for (const word of text.split(/\s+/u)) {
    if (used > 0 && used + 1 + word.length > width) {
      count++;
      used = 0;
    }
    if (used > 0) used++;
    count += Math.floor((word.length - 1) / width);
    used += ((word.length - 1) % width) + 1;
  }
  return count;
};

/** A card's height in pixels, estimated from what it shows. */
export function estimateHeight(step: Step): number {
  if (isQuestion(step))
    return (
      50 +
      18 * lines(step.prompt) +
      shownOptions(step).reduce((h, [label]) => h + 16 * lines(label, 12), 0)
    );
  if (step.kind === "checks")
    return (
      50 + 18 * lines(step.prompt) + step.rows.reduce((h, [label]) => h + 16 * lines(label, 12), 0)
    );
  if (step.kind === "fact") return 44 + 18 * lines(step.text);
  return 44 + 18 * lines(step.value);
}

/** Each step's depth: the longest chain of sources before it. Throws on cycles and unknown ids. */
export function depthsOf(label: string, steps: readonly Step[]): Map<string, number> {
  const byId = new Map(steps.map((step) => [step.id, step]));
  const depths = new Map<string, number>();
  const visit = (step: Step, seen: readonly string[]): number => {
    const known = depths.get(step.id);
    if (known !== undefined) return known;
    if (seen.includes(step.id)) throw new Error(`Network ${label} has a cycle at ${step.id}`);
    const parents = sourcesOf(step).map((id) => {
      const source = byId.get(id);
      if (source === undefined) throw new Error(`Network ${label}: ${step.id} reads unknown ${id}`);
      return visit(source, [...seen, step.id]);
    });
    const depth = parents.length === 0 ? 0 : 1 + Math.max(...parents);
    depths.set(step.id, depth);
    return depth;
  };
  for (const step of steps) visit(step, []);
  return depths;
}

/** Centers of cards stacked with `GAP` between them, as fractions of `total`. */
function stacked(heights: readonly number[], total: number): number[] {
  const used = heights.reduce((sum, h) => sum + h, 0) + GAP * (heights.length - 1);
  let top = (total - used) / 2;
  return heights.map((h) => {
    const center = top + h / 2;
    top += h + GAP;
    return center / total;
  });
}

/**
 * Narrow positions: each depth's cards in rows of up to `ACROSS`, rows stacked by their tallest
 * card, with extra space between depths.
 */
function narrowRows(
  heights: readonly (readonly number[])[],
  limit: number,
): {
  height: number;
  at: (depth: number, index: number) => Point;
} {
  const centers: Point[][] = [];
  let top = EDGE;
  heights.forEach((column, depth) => {
    const across = Math.min(limit, column.length);
    const points: Point[] = [];
    for (let start = 0; start < column.length; start += across) {
      const row = column.slice(start, start + across);
      const center = top + Math.max(...row) / 2;
      for (let offset = 0; offset < row.length; offset++) {
        points.push({ x: (offset + 0.5) / across, y: center });
      }
      top += Math.max(...row) + GAP;
    }
    centers[depth] = points;
    top += GAP;
  });
  const height = top - GAP + EDGE;
  return {
    height,
    at: (depth, index) => {
      const point = centers[depth]?.[index] ?? { x: 0.5, y: height / 2 };
      return { x: point.x, y: point.y / height };
    },
  };
}

/**
 * Place each step by its depth. Wide layouts run left to right with depths as columns; narrow
 * layouts run top to bottom with depths as rows. With `fit`, cards in a column stack by their
 * estimated heights and the layout reports the heights the panel needs.
 */
export function layoutNetwork(
  network: Network,
  options: { pad: number; withBranch?: boolean; fit?: boolean },
): Layout {
  const steps = [...network.steps, ...(options.withBranch === true ? (network.branch ?? []) : [])];
  const branchIds = new Set((network.branch ?? []).map((step) => step.id));
  const depths = depthsOf(network.label, steps);
  const columns: Step[][] = [];
  for (const step of steps) (columns[depths.get(step.id) ?? 0] ??= []).push(step);
  const heights = columns.map((column) => column.map((step) => estimateHeight(step)));
  const wideHeight =
    2 * EDGE + Math.max(...heights.map((h) => h.reduce((a, b) => a + b, 0) + GAP * (h.length - 1)));
  const narrow = narrowRows(heights, ACROSS);
  const stack = narrowRows(heights, 1);
  const fit = options.fit === true;
  const last = Math.max(columns.length - 1, 1);
  const placed = columns.flatMap((column, depth) => {
    const ys = fit
      ? stacked(heights[depth] ?? [], wideHeight)
      : column.map((_, index) => (index + 1) / (column.length + 1));
    return column.map((step, index): Placed => ({
      step,
      depth,
      branch: branchIds.has(step.id),
      wide: {
        x: columns.length === 1 ? 0.5 : options.pad + (depth / last) * (1 - 2 * options.pad),
        y: ys[index] ?? 0.5,
      },
      narrow: narrow.at(depth, index),
      stack: stack.at(depth, index),
    }));
  });
  const edges = steps.flatMap((step) =>
    sourcesOf(step).map((from): Edge => ({ from, to: step.id, branch: branchIds.has(step.id) })),
  );
  return {
    placed,
    edges,
    depths: columns.length,
    breadth: Math.min(ACROSS, Math.max(...columns.map((column) => column.length))),
    height: { wide: wideHeight, narrow: narrow.height, stack: stack.height },
  };
}

const coordinate = (n: number): string => n.toFixed(1);

/** An SVG path from one placed step to another in a 1000 × 1000 box, curving between columns. */
export function edgePath(from: Point, to: Point, direction: "wide" | "narrow" | "stack"): string {
  const [ax, ay, bx, by] = [from.x, from.y, to.x, to.y].map((n) => coordinate(n * 1000));
  if (direction === "wide") {
    const mx = coordinate(((from.x + to.x) / 2) * 1000);
    return `M${ax} ${ay}C${mx} ${ay} ${mx} ${by} ${bx} ${by}`;
  }
  const my = coordinate(((from.y + to.y) / 2) * 1000);
  return `M${ax} ${ay}C${ax} ${my} ${bx} ${my} ${bx} ${by}`;
}
