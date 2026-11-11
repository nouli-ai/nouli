import { required } from "./dom";

/** Industry selection is shareable; without scripts every network remains readable. */
class NetTabs extends HTMLElement {
  #controller: AbortController | undefined;
  connectedCallback(): void {
    this.#controller = new AbortController();
    const { signal } = this.#controller;
    const tabs = required(this, "[data-tabs]", HTMLElement);
    const caption = required(this, "[data-team-caption]", HTMLElement);
    const frames = [...this.querySelectorAll<HTMLElement>("[data-team-frame]")];
    const buttons = [...tabs.querySelectorAll<HTMLButtonElement>("button")];
    tabs.hidden = false;
    const render = (): void => {
      const selected = new URL(location.href).searchParams.get("industry");
      const button = buttons.find((b) => b.value === selected) ?? buttons[0];
      for (const frame of frames) frame.hidden = frame.dataset["teamFrame"] !== button?.value;
      for (const other of buttons) other.setAttribute("aria-pressed", String(other === button));
      caption.textContent = button?.dataset["session"] ?? "";
    };
    for (const button of buttons) {
      button.addEventListener(
        "click",
        () => {
          const url = new URL(location.href);
          url.searchParams.set("industry", button.value);
          history.pushState(null, "", url);
          render();
        },
        { signal },
      );
    }
    addEventListener("popstate", render, { signal });
    render();
  }
  disconnectedCallback(): void {
    this.#controller?.abort();
  }
}
customElements.define("net-tabs", NetTabs);
