import { copyButtons } from "./copy.js";

copyButtons();

const filters = document.querySelectorAll("[data-model-filter]");
const cards = document.querySelectorAll("[data-model-category]");
const count = document.querySelector("[data-model-count]");

for (const button of filters) {
  button.addEventListener("click", () => {
    const category = button.dataset.modelFilter;
    for (const filter of filters) {
      filter.setAttribute("aria-pressed", String(filter === button));
    }
    let visible = 0;
    for (const card of cards) {
      card.hidden = category !== "all" && card.dataset.modelCategory !== category;
      if (!card.hidden) visible++;
    }
    if (count) count.textContent = `${visible} models`;
  });
}
