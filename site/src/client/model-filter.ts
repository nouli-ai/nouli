import { required } from "./dom";

const parameter = "category";

/**
 * Filters the model cards inside it by category. The chosen category lives in the URL as
 * `?category=`, so filtered views can be shared and Back and Forward restore them.
 */
class ModelFilter extends HTMLElement {
  #controller: AbortController | undefined;

  connectedCallback(): void {
    this.#controller = new AbortController();
    const { signal } = this.#controller;
    for (const button of this.#buttons()) {
      button.addEventListener(
        "click",
        () => {
          const category = button.dataset["modelFilter"] ?? "all";
          const url = new URL(location.href);
          if (category === "all") url.searchParams.delete(parameter);
          else url.searchParams.set(parameter, category);
          if (url.href !== location.href) history.pushState(null, "", url);
          this.#show(category);
        },
        { signal },
      );
    }
    addEventListener(
      "popstate",
      () => {
        this.#show(this.#fromUrl());
      },
      { signal },
    );
    this.#show(this.#fromUrl());
  }

  disconnectedCallback(): void {
    this.#controller?.abort();
  }

  #buttons(): HTMLButtonElement[] {
    return [...this.querySelectorAll<HTMLButtonElement>("button[data-model-filter]")];
  }

  #fromUrl(): string {
    const category = new URL(location.href).searchParams.get(parameter);
    return this.#buttons().some((button) => button.dataset["modelFilter"] === category)
      ? (category ?? "all")
      : "all";
  }

  #show(category: string): void {
    this.dataset["category"] = category;
    for (const button of this.#buttons()) {
      button.setAttribute("aria-pressed", String(button.dataset["modelFilter"] === category));
    }
    let visible = 0;
    for (const card of this.querySelectorAll<HTMLElement>("[data-model-category]")) {
      card.hidden = category !== "all" && card.dataset["modelCategory"] !== category;
      if (!card.hidden) visible += 1;
    }
    required(this, "[data-model-count]", HTMLElement).textContent = `${visible} models`;
  }
}

customElements.define("model-filter", ModelFilter);
