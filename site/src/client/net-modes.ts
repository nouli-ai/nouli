import { required } from "./dom";

/** Cards that switch the network inside this element between chain, branch, blend, and run. */
class NetModes extends HTMLElement {
  #controller: AbortController | undefined;
  connectedCallback(): void {
    this.#controller = new AbortController();
    const net = required(this, "decision-net", HTMLElement);
    const label = required(this, "[data-mode-label]", HTMLElement);
    const cards = [...this.querySelectorAll<HTMLButtonElement>("button[data-mode]")];
    for (const card of cards) {
      card.addEventListener(
        "click",
        () => {
          const mode = card.dataset["mode"] ?? "chain";
          net.setAttribute("mode", mode);
          label.textContent = mode;
          for (const other of cards) other.setAttribute("aria-pressed", String(other === card));
        },
        { signal: this.#controller.signal },
      );
    }
  }
  disconnectedCallback(): void {
    this.#controller?.abort();
  }
}

customElements.define("net-modes", NetModes);
