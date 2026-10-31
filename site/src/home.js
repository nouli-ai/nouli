// Home page: the recorded TUI on its bench, with the session's graph beneath it.
// The graph follows the replay as each answer arrives. Clicking a box in the TUI's
// graph pane lightens that box and selects the same turn below; the replay keeps playing.
import graph from "./data/cli/graph.json";
import { Figure } from "./figure.js";
import { copyButtons } from "./copy.js";
import { mountCliReplay } from "./cli-replay.js";
import { showLatestRelease } from "./release.js";

copyButtons();
void showLatestRelease();

const bench = document.querySelector("[data-bench]");
const term = bench.querySelector("[data-cli-replay]");
const screen = term.querySelector(".cli-replay-screen");
const mark = term.querySelector(".term-mark");
const figure = new Figure(bench.querySelector("[data-figure]"));

/** The box the TUI drew for `node` in its graph pane, in terminal cells. */
function findBox(node) {
  const rows = [...term.querySelectorAll(".cli-replay-row")].map((row) => row.textContent);
  const label = `${node.session} · ${node.id.slice(0, 10)}`;
  const top = rows.findIndex((row) => row.includes(label));
  if (top < 0) return null;
  const left = rows[top].lastIndexOf("┌", rows[top].indexOf(label));
  const right = rows[top].indexOf("┐", left);
  let bottom = top + 1;
  while (bottom < rows.length && rows[bottom][left] !== "└") bottom++;
  return bottom < rows.length && left >= 0 && right > left ? { left, right, top, bottom } : null;
}

/** One terminal cell's size in pixels, measured from the drawn text. */
function cell() {
  const row = [...term.querySelectorAll(".cli-replay-row")].find((r) => r.textContent.length);
  if (!row) return { w: 0, h: 0 };
  const range = document.createRange();
  range.selectNodeContents(row);
  return { w: range.getBoundingClientRect().width / row.textContent.length, h: row.getBoundingClientRect().height };
}

/** Lighten the selected turn's box in the TUI, if the replay has drawn it. */
function markBox() {
  const box = figure.cur >= 0 && findBox(graph.nodes[figure.cur]);
  const { w, h } = cell();
  mark.hidden = !box || !w;
  if (mark.hidden) return;
  Object.assign(mark.style, {
    left: `${box.left * w}px`,
    top: `${box.top * h}px`,
    width: `${(box.right - box.left + 1) * w}px`,
    height: `${(box.bottom - box.top + 1) * h}px`,
  });
}

// Every selection in the graph below, by the replay, the TUI or the graph itself,
// also marks the box in the TUI.
const select = figure.select.bind(figure);
figure.select = (i, options) => {
  select(i, options);
  markBox();
};
figure.setGraph(graph.nodes);
figure.select(0);

/** The turn whose box in the TUI's graph pane is under the pointer. */
function boxAt(event) {
  const { w, h } = cell();
  if (!w || !h) return -1;
  const rect = screen.getBoundingClientRect();
  const col = (event.clientX - rect.left) / w, row = (event.clientY - rect.top) / h;
  return graph.nodes.findIndex((node) => {
    const box = findBox(node);
    return box && col >= box.left && col < box.right + 1 && row >= box.top && row < box.bottom + 1;
  });
}

const wide = !matchMedia("(max-width: 700px)").matches;
const load = wide ? import("./data/cli/tui.json") : import("./data/cli/tui-narrow.json");
load.then(({ default: recording }) => {
  term.dataset.columns = recording.columns;
  mountCliReplay(term, recording, {
    onStep: (step) => (step >= 0 ? figure.select(step) : markBox()),
  });
  term.addEventListener("click", (event) => {
    const hit = boxAt(event);
    if (hit < 0) return;
    figure.select(hit);
  });
  term.addEventListener("pointermove", (event) => {
    term.dataset.hover = String(boxAt(event) >= 0);
  });
  new ResizeObserver(markBox).observe(term);
});
