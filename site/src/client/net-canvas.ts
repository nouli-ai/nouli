/** Canvas drawing for `<decision-net>`: edges, pulses, glow, and the honeycomb around steps. */

export type Curve = [number, number, number, number, number, number, number, number];
/** A point with a weight: a light's strength, or a pulse's heading in radians. */
export type Weighted = [x: number, y: number, weight: number];

export interface Box {
  w: number;
  h: number;
  /** The canvas's position on the page, which fixes the honeycomb grid to the page. */
  left: number;
  top: number;
}

const HEX = 20;

/** The point at `u` along a cubic Bézier curve. */
export function at(c: Curve, u: number): [number, number] {
  const v = 1 - u;
  return [
    v * v * v * c[0] + 3 * v * v * u * c[2] + 3 * v * u * u * c[4] + u * u * u * c[6],
    v * v * v * c[1] + 3 * v * v * u * c[3] + 3 * v * u * u * c[5] + u * u * u * c[7],
  ];
}

/** A curve between two steps that leaves along the layout's direction. */
export function curveBetween(
  a: { x: number; y: number },
  b: { x: number; y: number },
  narrow: boolean,
): Curve {
  if (narrow) {
    const my = (a.y + b.y) / 2;
    return [a.x, a.y, a.x, my, b.x, my, b.x, b.y];
  }
  const mx = (a.x + b.x) / 2;
  return [a.x, a.y, mx, a.y, mx, b.y, b.x, b.y];
}

/**
 * Draw an edge and, once its answer starts travelling (`u` > 0), the lit part. Returns the pulse
 * head while the answer is still on its way.
 */
export function drawEdge(
  ctx: CanvasRenderingContext2D,
  c: Curve,
  look: { u: number; traced: boolean; dimmed: boolean; dashed: boolean; edge: string; lit: string },
): Weighted | undefined {
  ctx.lineWidth = look.traced ? 2 : 1.25;
  ctx.setLineDash(look.dashed ? [5, 5] : []);
  ctx.strokeStyle = look.edge;
  ctx.beginPath();
  ctx.moveTo(c[0], c[1]);
  ctx.bezierCurveTo(c[2], c[3], c[4], c[5], c[6], c[7]);
  ctx.stroke();
  if (look.u <= 0) return undefined;
  ctx.strokeStyle = look.lit;
  ctx.globalAlpha = look.dimmed ? 0.25 : 1;
  ctx.beginPath();
  ctx.moveTo(c[0], c[1]);
  for (let i = 1; i <= 24 * look.u; i++) ctx.lineTo(...at(c, i / 24));
  const head = at(c, look.u);
  ctx.lineTo(...head);
  ctx.stroke();
  ctx.globalAlpha = 1;
  ctx.setLineDash([]);
  if (look.u >= 1) return undefined;
  const back = at(c, Math.max(look.u - 0.02, 0));
  return [head[0], head[1], Math.atan2(head[1] - back[1], head[0] - back[0])];
}

/** A pulse: a small capsule with a slight side-to-side waggle, a nod to the waggle dance. */
export function drawPulse(
  ctx: CanvasRenderingContext2D,
  [x, y, angle]: Weighted,
  wag: number,
  colors: { glow: string; lit: string },
): void {
  ctx.save();
  ctx.translate(x, y);
  ctx.rotate(angle);
  const trail = ctx.createLinearGradient(-26, 0, 0, 0);
  trail.addColorStop(0, `rgba(${colors.glow},0)`);
  trail.addColorStop(1, `rgba(${colors.glow},.55)`);
  ctx.strokeStyle = trail;
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.moveTo(-26, 0);
  ctx.quadraticCurveTo(-12, -wag * 2, 0, wag);
  ctx.stroke();
  ctx.fillStyle = colors.lit;
  ctx.beginPath();
  ctx.ellipse(0, wag, 5, 2.6, 0, 0, 7);
  ctx.fill();
  ctx.restore();
}

/** A soft glow around a step or a pulse. */
export function drawLight(ctx: CanvasRenderingContext2D, [x, y, k]: Weighted, glow: string): void {
  const g = ctx.createRadialGradient(x, y, 0, x, y, 90 * k);
  g.addColorStop(0, `rgba(${glow},${0.16 * k})`);
  g.addColorStop(1, `rgba(${glow},0)`);
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(x, y, 90 * k, 0, 7);
  ctx.fill();
}

/** A hexagon path centered at (x, y). */
export function hexagon(ctx: CanvasRenderingContext2D, x: number, y: number): void {
  ctx.beginPath();
  for (let i = 0; i < 6; i++) {
    const angle = (Math.PI / 3) * i + Math.PI / 6;
    ctx.lineTo(x + (HEX - 1.5) * Math.cos(angle), y + (HEX - 1.5) * Math.sin(angle));
  }
  ctx.closePath();
}

/**
 * Call `cell(x, y)` for every honeycomb cell in `box`, on a grid fixed to the page so separate
 * canvases share one comb.
 */
export function forEachCell(box: Box, cell: (x: number, y: number) => void): void {
  const hw = Math.sqrt(3) * HEX;
  const rowH = HEX * 1.5;
  for (let row = Math.floor(box.top / rowH) - 1; row * rowH - box.top < box.h + rowH; row++) {
    const shift = (row % 2) * (hw / 2);
    const firstCol = Math.floor((box.left - shift) / hw) - 1;
    for (let col = firstCol; col * hw + shift - box.left < box.w + hw; col++) {
      cell(col * hw + shift - box.left, row * rowH - box.top);
    }
  }
}

/** The honeycomb around the steps, fading toward the edges and warmed by the lights. */
export function drawComb(
  ctx: CanvasRenderingContext2D,
  box: Box,
  steps: readonly { x: number; y: number }[],
  lights: readonly Weighted[],
  colors: { line: string; glow: string },
): void {
  ctx.lineWidth = 1;
  forEachCell(box, (x, y) => {
    let near = 0;
    for (const step of steps) near += Math.exp(-((x - step.x) ** 2 + (y - step.y) ** 2) / 16_000);
    const edge = Math.min(Math.max(Math.min(x, box.w - x, y, box.h - y) / 120, 0), 1);
    const fade = edge * Math.min(near * 1.4, 1);
    if (fade < 0.02) return;
    let k = 0;
    for (const [lx, ly, s] of lights) k += s * Math.exp(-((x - lx) ** 2 + (y - ly) ** 2) / 7000);
    hexagon(ctx, x, y);
    ctx.strokeStyle = `rgba(${colors.line},${(0.22 + Math.min(k, 1) * 0.25) * fade})`;
    ctx.stroke();
    if (k > 0.04) {
      ctx.fillStyle = `rgba(${colors.glow},${Math.min(k * 0.35, 0.45) * fade})`;
      ctx.fill();
    }
  });
}
