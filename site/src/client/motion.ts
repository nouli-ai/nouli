const reducedQuery = matchMedia("(prefers-reduced-motion: reduce)");

/** Whether the visitor currently asks for reduced motion; read at the moment of animating. */
export const prefersReducedMotion = (): boolean => reducedQuery.matches;

/**
 * Run `frame` on every animation frame while `element` is on screen, the tab is visible, and
 * motion is allowed. Returns a function that stops it for good.
 */
export function animateWhileVisible(element: Element, frame: (now: number) => void): () => void {
  const controller = new AbortController();
  let onScreen = false;
  let request: number | undefined;
  const loop = (now: number): void => {
    frame(now);
    request = requestAnimationFrame(loop);
  };
  const update = (): void => {
    const run = onScreen && !document.hidden && !prefersReducedMotion();
    if (run && request === undefined) request = requestAnimationFrame(loop);
    if (!run && request !== undefined) {
      cancelAnimationFrame(request);
      request = undefined;
    }
  };
  const observer = new IntersectionObserver(([entry]) => {
    onScreen = entry?.isIntersecting ?? false;
    update();
  });
  observer.observe(element);
  document.addEventListener("visibilitychange", update, { signal: controller.signal });
  reducedQuery.addEventListener("change", update, { signal: controller.signal });
  return () => {
    controller.abort();
    observer.disconnect();
    if (request !== undefined) cancelAnimationFrame(request);
  };
}

/** Animate `step(progress)` from 0 to 1 over `ms`, unless reduced motion is on. */
export function tween(ms: number, step: (progress: number) => void, signal?: AbortSignal): void {
  if (prefersReducedMotion() || signal?.aborted === true) {
    step(1);
    return;
  }
  const start = performance.now();
  let request: number;
  const finish = (): void => {
    cancelAnimationFrame(request);
    reducedQuery.removeEventListener("change", finish);
    signal?.removeEventListener("abort", finish);
    step(1);
  };
  const tick = (now: number): void => {
    const progress = Math.min(1, (now - start) / ms);
    step(progress);
    if (progress < 1) request = requestAnimationFrame(tick);
    else {
      reducedQuery.removeEventListener("change", finish);
      signal?.removeEventListener("abort", finish);
    }
  };
  reducedQuery.addEventListener("change", finish);
  signal?.addEventListener("abort", finish, { once: true });
  request = requestAnimationFrame(tick);
}
