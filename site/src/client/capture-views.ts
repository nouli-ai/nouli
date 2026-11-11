/**
 * Switches a terminal capture between its Form, Answer, and Decisions views. Every view is in
 * the page already; this only changes which one shows and which button is pressed.
 */
class CaptureViews extends HTMLElement {
  #controller: AbortController | undefined;

  connectedCallback(): void {
    this.#controller = new AbortController();
    for (const button of this.querySelectorAll<HTMLButtonElement>("button[data-capture-view]")) {
      button.addEventListener(
        "click",
        () => {
          this.#show(button.dataset["captureView"] ?? "answer");
        },
        { signal: this.#controller.signal },
      );
    }
  }

  disconnectedCallback(): void {
    this.#controller?.abort();
  }

  #show(view: string): void {
    this.dataset["view"] = view;
    for (const button of this.querySelectorAll<HTMLButtonElement>("button[data-capture-view]")) {
      button.setAttribute("aria-pressed", String(button.dataset["captureView"] === view));
    }
  }
}

customElements.define("capture-views", CaptureViews);
