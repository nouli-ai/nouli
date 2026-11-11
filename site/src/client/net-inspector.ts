import { required } from "./dom";

/** Opens a panel showing what a selected step saw, its answer distribution, and its pick. */
class NetInspector extends HTMLElement {
  #controller: AbortController | undefined;
  #opener: HTMLElement | undefined;
  connectedCallback(): void {
    this.#controller = new AbortController();
    const { signal } = this.#controller;
    const panel = required(this, "[data-inspector]", HTMLElement);
    const close = (): void => {
      panel.hidden = true;
      this.#opener?.focus();
    };
    required(panel, "[data-close]", HTMLButtonElement).addEventListener("click", close, { signal });
    this.addEventListener(
      "keydown",
      (event) => {
        if (event.key === "Escape") close();
      },
      { signal },
    );
    this.addEventListener(
      "focusout",
      (event) => {
        if (!(event.relatedTarget instanceof Node) || !this.contains(event.relatedTarget))
          panel.hidden = true;
      },
      { signal },
    );
    this.addEventListener(
      "net:select",
      (event) => {
        if (!(event instanceof CustomEvent) || !(event.target instanceof HTMLElement)) return;
        const id: unknown = event.detail;
        if (typeof id !== "string") return;
        const node = event.target.querySelector<HTMLElement>(`.node[data-id="${CSS.escape(id)}"]`);
        if (!node) return;
        this.#opener = node;
        this.#fill(panel, event.target, node);
        panel.hidden = false;
        required(panel, "[data-close]", HTMLButtonElement).focus();
      },
      { signal },
    );
  }
  disconnectedCallback(): void {
    this.#controller?.abort();
  }
  #fill(panel: HTMLElement, net: HTMLElement, node: HTMLElement): void {
    const text = (selector: string): string => node.querySelector(selector)?.textContent ?? "";
    const fact = node.classList.contains("fact");
    required(panel, "[data-kind]", HTMLElement).textContent = fact ? "Fact" : "Step";
    required(panel, "h3", HTMLElement).textContent = fact
      ? text(".k").replace(/^Fact · /u, "")
      : text(".q") || text(".k");
    const saw = required(panel, "[data-saw]", HTMLUListElement);
    const sources = (node.dataset["from"] ?? "").split(" ").filter(Boolean);
    saw.replaceChildren(
      ...sources.map((id) => {
        const item = document.createElement("li");
        item.textContent =
          net.querySelector<HTMLElement>(`.node[data-id="${CSS.escape(id)}"]`)?.dataset[
            "summary"
          ] ?? id;
        return item;
      }),
    );
    saw.hidden = sources.length === 0;
    required(panel, "[data-saw-label]", HTMLElement).hidden = sources.length === 0;
    const dist = required(panel, "[data-dist]", HTMLUListElement);
    dist.replaceChildren(
      ...[...node.querySelectorAll(".o")].map((row) => {
        const item = document.createElement("li");
        item.className = row.classList.contains("pick") ? "pick" : "";
        const [label, value] = [document.createElement("span"), document.createElement("span")];
        label.textContent = row.querySelector(".ol")?.textContent ?? "";
        value.textContent = Number(row.querySelector<HTMLElement>(".on")?.dataset["p"]).toFixed(2);
        item.append(label, value);
        return item;
      }),
    );
    dist.hidden = node.querySelector(".o") === null;
    required(panel, "[data-dist-label]", HTMLElement).hidden = node.querySelector(".o") === null;
    required(panel, "[data-answer]", HTMLElement).textContent = fact
      ? text(".t")
      : (node.querySelector(".o.pick .ol")?.textContent ?? (text(".sv b") || text(".t")));
  }
}

customElements.define("net-inspector", NetInspector);
