/**
 * Floats the page navigation once the page scrolls, and marks whether a sky panel or the light
 * page is behind it, so the glass tint and text color follow the background.
 */
class FloatNav extends HTMLElement {
  #controller: AbortController | undefined;
  #frame: number | undefined;
  connectedCallback(): void {
    this.#controller = new AbortController();
    const { signal } = this.#controller;
    const schedule = (): void => {
      this.#frame ??= requestAnimationFrame(() => {
        this.#frame = undefined;
        this.#update();
      });
    };
    addEventListener("scroll", schedule, { passive: true, signal });
    addEventListener("resize", schedule, { passive: true, signal });
    this.#update();
  }
  disconnectedCallback(): void {
    this.#controller?.abort();
    if (this.#frame !== undefined) cancelAnimationFrame(this.#frame);
    this.#frame = undefined;
  }
  #update(): void {
    this.toggleAttribute("data-floating", scrollY > 24);
    const bar = this.firstElementChild?.getBoundingClientRect();
    if (!bar) return;
    const x = bar.left + bar.width / 2;
    const y = bar.top + bar.height / 2;
    const overSky = [...document.querySelectorAll(".sky")].some((sky) => {
      const r = sky.getBoundingClientRect();
      return x >= r.left && x <= r.right && y >= r.top && y <= r.bottom;
    });
    this.dataset["behind"] = overSky ? "sky" : "page";
  }
}
customElements.define("float-nav", FloatNav);
