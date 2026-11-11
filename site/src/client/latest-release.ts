import { required } from "./dom";
import { repository } from "../lib/site";

/** Resolve the published release at view time; retain the generic link on timeout or failure. */
class LatestRelease extends HTMLElement {
  #controller: AbortController | undefined;
  #timer: ReturnType<typeof setTimeout> | undefined;
  connectedCallback(): void {
    this.#controller = new AbortController();
    this.#timer = setTimeout(() => this.#controller?.abort(), 10_000);
    void this.#load(this.#controller.signal);
  }
  disconnectedCallback(): void {
    this.#controller?.abort();
    clearTimeout(this.#timer);
  }
  async #load(signal: AbortSignal): Promise<void> {
    try {
      const response = await fetch(
        `https://api.github.com/repos/${repository.split("github.com/")[1] ?? ""}/releases/latest`,
        {
          headers: { Accept: "application/vnd.github+json" },
          cache: "no-store",
          signal,
        },
      );
      if (!response.ok) return;
      const release: unknown = await response.json();
      if (
        !signal.aborted &&
        typeof release === "object" &&
        release !== null &&
        "tag_name" in release &&
        typeof release.tag_name === "string" &&
        release.tag_name.trim() !== ""
      )
        required(this, "a", HTMLAnchorElement).textContent = release.tag_name;
    } catch {
      // A failed lookup leaves the generic latest-release link usable.
    } finally {
      clearTimeout(this.#timer);
    }
  }
}
customElements.define("latest-release", LatestRelease);
