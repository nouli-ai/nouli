import { cellSpan } from './term-colors.js';
import './cli-replay.css';

// Render the CLI's recorded terminal cells directly as selectable HTML text.
// `onStep(index)` reports the latest answered step (-1 before the first answer);
// the returned `seek(index)` shows the screen just after that step's answer.
export function mountCliReplay(root, recording, { onStep } = {}) {
  const screen = root.querySelector('.cli-replay-screen');
  const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const playbackRate = 3;
  const stepMs = 1000 / 60;
  let observer, pending = true, inView = false, playing = false, animation;
  let next = 0, time = 0, remainder = 0, lastTime = null, step = -1;
  let previousCursor = recording.initial.cursor, currentCursor = previousCursor;
  const steps = recording.steps ?? [];
  const rows = Array.from({length: recording.lines}, () => {
    const row = document.createElement('span');
    row.className = 'cli-replay-row';
    return row;
  });
  const cursor = document.createElement('span');
  cursor.className = 'cli-replay-cursor';
  cursor.setAttribute('aria-hidden', 'true');
  screen.replaceChildren(...rows, cursor);
  let due = 0;
  const timeline = recording.updates.map(update => ({...update, at: due += update.wait}));

  function renderRow(index, runs) {
    const fragment = document.createDocumentFragment();
    for (const run of runs) fragment.append(cellSpan(...run));
    rows[index].replaceChildren(fragment);
  }
  function moveCursor(position) {
    cursor.hidden = !position;
    if (position) {
      cursor.style.left = position[0]+'ch';
      cursor.style.top = (position[1]*1.35)+'em';
    }
  }
  function render(snapshot) {
    snapshot.rows.forEach((runs, index) => renderRow(index, runs));
    moveCursor(snapshot.cursor);
  }
  function report() {
    let reached = -1;
    while (reached + 1 < steps.length && steps[reached + 1].at <= time) reached++;
    if (reached !== step) {
      step = reached;
      onStep?.(step);
    }
  }
  function stopAutoplay() {
    pending = false;
    observer?.disconnect();
    document.removeEventListener('visibilitychange', autoplay);
  }
  function status(active) {
    playing = active;
    root.dataset.playing = String(active);
    // No title tooltip: it would cover the graph pane under the pointer.
    const hint = active ? 'Press Space to pause. Enter restarts.' : 'Press Space to play. Enter restarts.';
    screen.setAttribute('aria-label', 'Nouli TUI replay with its decision graph. '+hint);
  }
  function tick(now) {
    if (!document.hidden) {
      if (lastTime === null) lastTime = now;
      remainder += Math.max(0, Math.min(now - lastTime, 100));
      lastTime = now;
      // Preserve the same playback speed at every display refresh rate.
      while (remainder + 1e-7 >= stepMs) {
        remainder = Math.max(0, remainder - stepMs);
        time += stepMs * playbackRate;
        previousCursor = currentCursor;
        while (next < timeline.length && timeline[next].at <= time) {
          const update = timeline[next++];
          for (const [index, runs] of update.rows) renderRow(index, runs);
          currentCursor = update.cursor;
        }
      }
      report();
      // Interpolate typing within a line; field changes and wraps stay immediate.
      const blend = remainder / stepMs;
      const sameLine = previousCursor && currentCursor && previousCursor[1] === currentCursor[1];
      moveCursor(sameLine && Math.abs(currentCursor[0] - previousCursor[0]) <= 3
        ? [previousCursor[0] + (currentCursor[0] - previousCursor[0]) * blend, currentCursor[1]]
        : currentCursor);
    } else {
      lastTime = null;
    }
    if (next < timeline.length) animation = requestAnimationFrame(tick);
    else { moveCursor(recording.final.cursor); status(false); }
  }
  function restart() {
    next = 0;
    time = remainder = 0;
    previousCursor = currentCursor = recording.initial.cursor;
    render(recording.initial);
    report();
  }
  function play(fromStart = false) {
    stopAutoplay();
    cancelAnimationFrame(animation);
    if (reducedMotion) { render(recording.final); time = due; report(); status(false); return; }
    if (fromStart || next === timeline.length) restart();
    status(true);
    lastTime = null;
    animation = requestAnimationFrame(tick);
  }
  function pause() {
    cancelAnimationFrame(animation);
    lastTime = null;
    stopAutoplay();
    status(false);
  }
  function autoplay() {
    if (pending && inView && !document.hidden) play();
  }
  function toggle() {
    if (playing) pause();
    else play();
  }
  /** Pause on the screen just after step `index` was answered. */
  function seek(index) {
    pause();
    const target = steps[index]?.at ?? 0;
    restart();
    while (next < timeline.length && timeline[next].at <= target) {
      const update = timeline[next++];
      for (const [row, runs] of update.rows) renderRow(row, runs);
      currentCursor = update.cursor;
    }
    time = target;
    remainder = 0;
    previousCursor = currentCursor;
    moveCursor(currentCursor);
    report();
  }
  screen.addEventListener('keydown', event => {
    if (event.key === ' ' || event.key === 'Enter') {
      event.preventDefault();
      if (event.key === 'Enter') play(true);
      else toggle();
    }
  });
  // Fit every recorded column and row without an inner scrollbar. Measure the
  // rendered font (after web fonts load) rather than guessing its advance width.
  // The probe sits in a zero-size clipped box so it never widens the page.
  const shelf = document.createElement('span');
  shelf.setAttribute('aria-hidden', 'true');
  shelf.style.cssText = 'position:absolute;left:0;top:0;width:0;height:0;overflow:hidden;visibility:hidden';
  const probe = document.createElement('span');
  probe.style.cssText = 'position:absolute;white-space:pre;font:inherit;font-family:var(--term)';
  probe.textContent = '0'.repeat(100);
  shelf.append(probe);
  root.append(shelf);
  const measure = document.createElement('canvas').getContext('2d');
  function fit() {
    // One cell's width per pixel of font size: from the rendered probe when the
    // page has laid it out, otherwise from the font's metrics.
    const size = parseFloat(getComputedStyle(root).fontSize);
    const rendered = probe.getBoundingClientRect?.().width / 100 / size;
    measure.font = '10px '+getComputedStyle(screen).fontFamily;
    const unit = rendered > 0 ? rendered : measure.measureText('0').width / 10;
    root.style.fontSize = (Math.floor(root.clientWidth / (recording.columns * unit) * 100) / 100)+'px';
  }
  new ResizeObserver(fit).observe(root);
  document.fonts?.ready.then(fit);
  status(false);
  if (reducedMotion) { render(recording.final); time = due; report(); }
  else render(recording.initial);
  if (!reducedMotion) {
    document.addEventListener('visibilitychange', () => { lastTime = null; });
    observer = new IntersectionObserver(entries => {
      inView = entries.some(entry => entry.isIntersecting && entry.intersectionRatio >= .15);
      autoplay();
    }, {threshold: .15});
    observer.observe(root);
    document.addEventListener('visibilitychange', autoplay);
  }
  return { seek, pause, toggle, get playing() { return playing; }, get step() { return step; } };
}
