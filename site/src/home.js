// Home page: the recorded travel session, switchable between its recorded variants.
import hero from "./data/hero.json";
import { Figure } from "./figure.js";
import { copyButtons } from "./copy.js";
import { playTranscript } from "./repl.js";
import session from "./data/repl-session.txt?raw";

copyButtons();
playTranscript(document.getElementById("screen"), session, document.getElementById("replay"));

const POSITIONS = [{ x: 0, y: 0 }, { x: 290, y: 0 }, { x: 0, y: 170 }, { x: 145, y: 340 }, { x: 145, y: 510 }];
const PLAN = 4;

const root = document.createElement("div");
const panel = document.getElementById("panel");
panel.before(root);
const figure = new Figure(root, { positions: POSITIONS });
root.appendChild(panel);

let train = "cancelled";
const sources = () => {
  const on = [...document.querySelectorAll("#sources input:checked")].map((i) => i.value);
  return on.length === 2 ? "both" : on[0];
};
let current = hero.variants[`${train}|${sources()}`];
figure.setGraph(current);
figure.select(PLAN);

function update() {
  const next = hero.variants[`${train}|${sources()}`];
  const sig = (n) => JSON.stringify([n.parents, n.questions]);
  const changed = next.flatMap((n, i) => (sig(n) !== sig(current[i]) ? [i] : []));
  current = next;
  figure.setGraph(next, changed);
  if (changed.length) figure.ripple(Math.min(...changed));
}

document.querySelectorAll("#train button").forEach((b) =>
  b.addEventListener("click", () => {
    train = b.dataset.v;
    document.querySelectorAll("#train button").forEach((x) => x.setAttribute("aria-pressed", String(x === b)));
    update();
  }),
);
document.querySelectorAll("#sources input").forEach((c) =>
  c.addEventListener("change", () => {
    if (!document.querySelector("#sources input:checked")) c.checked = true;
    update();
  }),
);
