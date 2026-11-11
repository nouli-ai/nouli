import { prefersReducedMotion } from "./motion";

/** Reveal below-screen sections after scrolling; all content starts readable. */
class RevealSections extends HTMLElement {
  #controller: AbortController | undefined;
  #observer: IntersectionObserver | undefined;
  connectedCallback(): void {
    this.#controller = new AbortController();
    const { signal } = this.#controller;
    this.#observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue;
        if (entry.target instanceof HTMLElement) delete entry.target.dataset["waiting"];
        this.#observer?.unobserve(entry.target);
      }
    });
    addEventListener(
      "scroll",
      () => {
        if (prefersReducedMotion()) return;
        const waiting = [...this.querySelectorAll<HTMLElement>("[data-reveal]")].filter(
          (el) => el.getBoundingClientRect().top >= innerHeight + 40,
        );
        for (const el of waiting) {
          el.dataset["waiting"] = "";
          this.#observer?.observe(el);
        }
      },
      { once: true, passive: true, signal },
    );
  }
  disconnectedCallback(): void {
    this.#controller?.abort();
    this.#observer?.disconnect();
  }
}
customElements.define("reveal-sections", RevealSections);
