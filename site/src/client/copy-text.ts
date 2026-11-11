import { required } from "./dom";

const feedbackMs = 1600;

/**
 * Copies the text of its `[data-copy-source]` when its `[data-copy-button]` is pressed. The button
 * confirms visibly and a status region announces the result to assistive technology.
 */
class CopyText extends HTMLElement {
  #controller: AbortController | undefined;
  #timer: ReturnType<typeof setTimeout> | undefined;

  connectedCallback(): void {
    const button = required(this, "[data-copy-button]", HTMLButtonElement);
    const source = required(this, "[data-copy-source]", HTMLElement);
    const status = required(this, "[data-copy-status]", HTMLElement);
    const label = button.textContent;
    this.#controller = new AbortController();
    button.addEventListener(
      "click",
      () => {
        void this.#copy({ button, source, status, label });
      },
      { signal: this.#controller.signal },
    );
  }

  disconnectedCallback(): void {
    this.#controller?.abort();
    clearTimeout(this.#timer);
  }

  async #copy(parts: {
    button: HTMLButtonElement;
    source: HTMLElement;
    status: HTMLElement;
    label: string;
  }): Promise<void> {
    const { button, source, status, label } = parts;
    try {
      await navigator.clipboard.writeText(source.textContent.trimEnd());
      button.textContent = "copied";
      button.dataset["copied"] = "";
      status.textContent = "Copied to the clipboard";
    } catch {
      button.textContent = "select it";
      status.textContent = "Copying failed; select the text instead";
    }
    clearTimeout(this.#timer);
    this.#timer = setTimeout(() => {
      button.textContent = label;
      delete button.dataset["copied"];
      status.textContent = "";
    }, feedbackMs);
  }
}

customElements.define("copy-text", CopyText);
