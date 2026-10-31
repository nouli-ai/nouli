import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";

const read = (name) => readFileSync(new URL("../src/" + name, import.meta.url), "utf8");
const recording = JSON.parse(read("data/cli/tui.json"));
const finalRows = recording.final.rows.map((row) => row.map((run) => run[0]).join(""));
// Whole seconds the replay needs at its 3x playback rate, plus one to settle.
const replaySeconds = Math.ceil(recording.updates.reduce((total, update) => total + update.wait, 0) / 3000) + 1;

// Drive the shipped renderer with controlled display timestamps and terminal cells.
function fixture(reduced = false) {
  const element = (fragment = false) => ({
    fragment, children: [], style: {}, dataset: {}, listeners: new Map(),
    get textContent() { return this.text ?? this.children.map((child) => child.textContent).join(""); },
    set textContent(text) { this.text = text; this.children = []; },
    append(...children) { this.children.push(...children); },
    replaceChildren(...children) { this.children = children.flatMap((child) => child.fragment ? child.children : [child]); },
    setAttribute() {},
    addEventListener(name, fn) { if (!this.listeners.has(name)) this.listeners.set(name, new Set()); this.listeners.get(name).add(fn); },
    removeEventListener(name, fn) { this.listeners.get(name)?.delete(fn); },
    emit(name, event) { for (const fn of this.listeners.get(name) ?? []) fn(event); },
  });
  const screen = element(), root = element(), document = element();
  root.clientWidth = 1344;
  root.querySelector = () => screen;
  document.hidden = false;
  document.createElement = (tag) => tag === "canvas"
    ? { getContext: () => ({ measureText: () => ({ width: 6 }) }) }
    : element();
  document.createDocumentFragment = () => element(true);
  let intersection, now = 0, id = 0;
  const frames = new Map();
  const mount = vm.runInNewContext((read("term-colors.js") + read("cli-replay.js"))
    .replace(/^import .*;\n/gm, "").replace(/\bexport /g, "") + "\nmountCliReplay;", {
    recording, document, matchMedia: () => ({ matches: reduced }),
    getSelection: () => ({ isCollapsed: true }), getComputedStyle: () => ({ fontFamily: "monospace" }),
    ResizeObserver: class { constructor(fn) { this.fn = fn; } observe() { this.fn(); } },
    IntersectionObserver: class { constructor(fn) { intersection = fn; } observe() {} disconnect() {} },
    performance: { now: () => now },
    requestAnimationFrame: (fn) => { frames.set(++id, fn); return id; },
    cancelAnimationFrame: (key) => frames.delete(key),
  });
  mount(root, recording);
  intersection?.([{ isIntersecting: true, intersectionRatio: 1 }]);
  return {
    document, root, screen, frames,
    frame(time) { now = time; const callbacks = [...frames.values()]; frames.clear(); for (const fn of callbacks) fn(time); },
    key(key) { screen.emit("keydown", { key, preventDefault() {} }); },
    hide(hidden) { document.hidden = hidden; document.emit("visibilitychange"); },
    snapshot() {
      const cursor = screen.children.at(-1);
      return {
        rows: screen.children.slice(0, -1).map((row) => row.textContent),
        hidden: cursor.hidden, x: parseFloat(cursor.style.left), y: parseFloat(cursor.style.top),
        playing: root.dataset.playing,
      };
    },
  };
}

function same(actual, expected) {
  assert.deepEqual(actual.rows, expected.rows);
  assert.equal(actual.playing, expected.playing);
  assert.equal(actual.hidden, expected.hidden);
  if (!actual.hidden) {
    assert.ok(Math.abs(actual.x - expected.x) < 1e-7);
    assert.ok(Math.abs(actual.y - expected.y) < 1e-7);
  }
}

test("replay has the same recorded state at shared times on 30, 60, 120 and 144 Hz displays", () => {
  const reference = new Map();
  for (const hz of [60, 30, 120, 144]) {
    const run = fixture();
    let interpolated = 0;
    for (let frame = 0; frame <= hz * replaySeconds; frame++) {
      run.frame(frame * 1000 / hz);
      const snapshot = run.snapshot(), tick = frame * 60 / hz;
      if (hz === 60) reference.set(tick, snapshot);
      else if (Number.isInteger(tick)) same(snapshot, reference.get(tick));
      if (!snapshot.hidden && Math.abs(snapshot.x - Math.round(snapshot.x)) > 1e-7) interpolated++;
    }
    assert.deepEqual(run.snapshot().rows, finalRows);
    assert.equal(run.root.dataset.playing, "false");
    if (hz > 60) assert.ok(interpolated > 0, "High-refresh frames should interpolate the cursor");
  }
});

test("a stalled replay frame catches up by at most 100 ms", () => {
  const stalled = fixture(), bounded = fixture();
  stalled.frame(0); bounded.frame(0);
  stalled.frame(60000); bounded.frame(100);
  same(stalled.snapshot(), bounded.snapshot());
});

test("hidden tabs preserve the fractional tick and consume no hidden time", () => {
  const paused = fixture(), continuous = fixture();
  for (const run of [paused, continuous]) { run.frame(0); run.frame(100); run.frame(175); }
  paused.hide(true); paused.frame(30000);
  same(paused.snapshot(), continuous.snapshot());
  paused.hide(false); paused.frame(60000);
  same(paused.snapshot(), continuous.snapshot());
  paused.frame(60000 + 1000 / 120); continuous.frame(175 + 1000 / 120);
  same(paused.snapshot(), continuous.snapshot());
});

test("keyboard pause resumes without a jump and Enter restarts", () => {
  const paused = fixture(), continuous = fixture();
  for (const run of [paused, continuous]) { run.frame(0); run.frame(100); run.frame(175); }
  const before = paused.snapshot();
  paused.key(" "); paused.frame(30000);
  assert.deepEqual(paused.snapshot().rows, before.rows);
  assert.equal(paused.root.dataset.playing, "false");
  paused.key(" "); paused.frame(60000);
  paused.frame(60000 + 1000 / 120); continuous.frame(175 + 1000 / 120);
  same(paused.snapshot(), continuous.snapshot());
  paused.key("Enter");
  assert.deepEqual(paused.snapshot().rows, recording.initial.rows.map((row) => row.map((run) => run[0]).join("")));
});

test("reduced motion presents the complete terminal without scheduling animation", () => {
  const run = fixture(true);
  assert.deepEqual(run.snapshot().rows, finalRows);
  assert.equal(run.root.dataset.playing, "false");
  assert.equal(run.frames.size, 0);
});

test("text scrambling keeps a 60 Hz cadence and its specified duration", () => {
  for (const hz of [30, 60, 120, 144]) {
    let frame, writes = 0, text = "Nouli";
    const decode = vm.runInNewContext(read("decode.js").replace(/\bexport /g, "") + "\ndecode;", {
      matchMedia: () => ({ matches: false }), performance: { now: () => 0 },
      requestAnimationFrame: (fn) => { frame = fn; },
    });
    decode({ get textContent() { return text; }, set textContent(value) { text = value; writes++; } }, 1000);
    for (let i = 0; i <= hz; i++) { const next = frame; frame = null; next(i * 1000 / hz); }
    assert.equal(writes, Math.min(hz, 60) + 1);
    assert.equal(text, "Nouli");
    assert.equal(frame, null);
  }
});
