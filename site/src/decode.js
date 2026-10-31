// Text that scrambles briefly, then resolves left to right.
const GLYPHS = "#%&*+=-/\\|01";
const reduce = matchMedia("(prefers-reduced-motion: reduce)").matches;

export function decode(el, ms = 320) {
  if (reduce) return;
  const text = el.textContent, t0 = performance.now();
  let lastGlyphFrame = -1;
  const step = (now) => {
    const k = (now - t0) / ms;
    const glyphFrame = Math.floor((now - t0) / (1000 / 60) + 1e-7);
    if (k >= 1 || glyphFrame !== lastGlyphFrame) {
      lastGlyphFrame = glyphFrame;
      el.textContent = k >= 1 ? text : [...text].map((c, i) => (c === " " || i / text.length < k ? c : GLYPHS[(Math.random() * GLYPHS.length) | 0])).join("");
    }
    if (k < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

/** Decode each `[data-decode]` element once, when it scrolls into view. */
export function decodeOnView() {
  const io = new IntersectionObserver((entries) => {
    for (const e of entries) if (e.isIntersecting) { io.unobserve(e.target); decode(e.target, 600); }
  }, { threshold: 0.6 });
  document.querySelectorAll("[data-decode]").forEach((el) => io.observe(el));
}
