// Walkthrough pages: the figure follows the step being read, and nodes jump to their step.
import { Figure } from "./figure.js";

const graphs = import.meta.glob("./data/examples/*.json", { eager: true, import: "default" });
const graph = graphs[`./data/examples/${document.body.dataset.graph}.json`];
const figure = new Figure(document.getElementById("figure"));
figure.setGraph(graph.nodes);

const steps = [...document.querySelectorAll(".step")];
const stepFor = (node) => steps.find((s) => s.dataset.node === String(node));
figure.select(Number(steps.find((s) => s.dataset.node)?.dataset.node ?? 0));

// A step becomes current when it crosses the upper third of the viewport.
const io = new IntersectionObserver(
  (entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      steps.forEach((s) => s.classList.toggle("on", s === e.target));
      if (e.target.dataset.node) figure.select(Number(e.target.dataset.node));
    }
  },
  { rootMargin: "-30% 0px -65% 0px" },
);
steps.forEach((s) => io.observe(s));

figure.svg.addEventListener("click", (event) => {
  const box = event.target.closest(".node");
  const i = figure.boxes.indexOf(box);
  if (i >= 0) stepFor(i)?.scrollIntoView({ behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
});
