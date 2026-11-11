import type { Replay } from "../lib/schemas/replay";
import type { CellRun } from "../lib/schemas/terminal";
import { runStyle } from "../lib/terminal/colors";
import { required } from "./dom";
import { prefersReducedMotion } from "./motion";

type Cursor = Replay["initial"]["cursor"] | undefined;
const stepMs = 1000 / 60;
const playbackRate = 3;

/** The background that draws a run of one block character, or undefined for other text. */
function blockFill(text: string, fg: string, bg: string): string | undefined {
  const under = bg === "" ? "transparent" : bg;
  if (/^█+$/u.test(text)) return fg;
  if (/^▀+$/u.test(text)) return `linear-gradient(${fg} 50%, ${under} 50%)`;
  if (/^▄+$/u.test(text)) return `linear-gradient(${under} 50%, ${fg} 50%)`;
  return undefined;
}

/** Recorded terminal cells, played at 3× speed only while visible, with pause and restart buttons. */
export class CliReplay extends HTMLElement {
  #controller: AbortController | undefined;
  #observer: IntersectionObserver | undefined;
  #resize: ResizeObserver | undefined;
  #animation: number | undefined;
  #recording: Replay | undefined;
  #timeline: (Replay["updates"][number] & { at: number })[] = [];
  #rows: HTMLSpanElement[] = [];
  #cursor = document.createElement("span");
  #next = 0;
  #time = 0;
  #remainder = 0;
  #lastTime: number | undefined;
  #step = -1;
  #currentCursor: Cursor;
  #previousCursor: Cursor;
  #inView = false;
  #wantsPlay = true;
  #fittedWidth = 0;

  connectedCallback(): void {
    this.#controller = new AbortController();
    void this.#load(this.#controller.signal);
  }

  async #load(signal: AbortSignal): Promise<void> {
    try {
      const loaded = matchMedia("(max-width: 700px)").matches
        ? await import("../data/replays/narrow.json")
        : await import("../data/replays/wide.json");
      if (signal.aborted) return;
      // These committed recordings are validated against the replay schema by the unit suite.
      const recording = loaded.default;
      // oxlint-disable-next-line typescript/no-unsafe-type-assertion -- unit tests validate these exact JSON imports; no runtime schema ships.
      this.#mount(recording as unknown as Replay, signal);
    } catch {
      // The complete static screen and recording link remain available if loading fails.
    }
  }

  #screen(): HTMLPreElement {
    return required(this, ".cli-replay-screen", HTMLPreElement);
  }

  #mount(recording: Replay, signal: AbortSignal): void {
    this.#recording = recording;
    this.dataset["columns"] = String(recording.columns);
    this.#next = this.#time = this.#remainder = 0;
    this.#step = -1;
    this.#wantsPlay = true;
    this.#rows = Array.from({ length: recording.lines }, () => {
      const row = document.createElement("span");
      row.className = "cli-replay-row";
      return row;
    });
    this.#cursor.className = "cli-replay-cursor";
    this.#cursor.setAttribute("aria-hidden", "true");
    const screen = this.#screen();
    screen.replaceChildren(...this.#rows, this.#cursor);
    let due = 0;
    this.#timeline = recording.updates.map((update) => ({ ...update, at: (due += update.wait) }));
    this.#previousCursor = this.#currentCursor = recording.initial.cursor;
    this.#render(recording.initial);
    this.#listen(signal);
    this.#resize = new ResizeObserver(() => {
      this.#fit();
    });
    this.#resize.observe(this);
    void document.fonts.ready.then(() => {
      if (!signal.aborted) this.#fit();
    });
    this.#fit();
    this.#update();
  }

  #listen(signal: AbortSignal): void {
    required(this, "[data-replay-controls]", HTMLElement).hidden = false;
    const press = (restart: boolean): void => {
      if (restart || this.#next === this.#timeline.length) this.#restart();
      this.#wantsPlay = restart || !this.#wantsPlay;
      this.#update();
    };
    required(this, "[data-replay-toggle]", HTMLButtonElement).addEventListener(
      "click",
      () => {
        press(false);
      },
      { signal },
    );
    required(this, "[data-replay-restart]", HTMLButtonElement).addEventListener(
      "click",
      () => {
        press(true);
      },
      { signal },
    );
    document.addEventListener(
      "visibilitychange",
      () => {
        this.#update();
      },
      { signal },
    );
    matchMedia("(prefers-reduced-motion: reduce)").addEventListener(
      "change",
      () => {
        this.#update();
      },
      { signal },
    );
    this.#observer = new IntersectionObserver(
      (entries) => {
        this.#inView = entries.some(
          (entry) => entry.isIntersecting && entry.intersectionRatio >= 0.15,
        );
        this.#update();
      },
      { threshold: 0.15 },
    );
    this.#observer.observe(this);
  }

  #renderRow(index: number, runs: CellRun[]): void {
    const spans = runs.flatMap((run) =>
      // Block characters become spaces painted with the cell's colors, so they fill whole or half
      // cells exactly, whatever font would have supplied the glyph.
      run[0].split(/(█+|▀+|▄+)/u).flatMap((text) => {
        if (text === "") return [];
        const span = document.createElement("span");
        span.style.cssText = runStyle(run);
        const block = blockFill(text, span.style.color, span.style.backgroundColor);
        if (block === undefined) span.textContent = text;
        else {
          span.style.background = block;
          span.textContent = " ".repeat(text.length);
        }
        return [span];
      }),
    );
    this.#rows[index]?.replaceChildren(...spans);
  }

  #moveCursor(position: Cursor): void {
    this.#cursor.hidden = position === undefined || position === null;
    if (position !== undefined && position !== null) {
      this.#cursor.style.left = `${position[0]}ch`;
      this.#cursor.style.top = `${position[1] * 1.35}em`;
    }
  }

  #render(snapshot: Replay["initial"]): void {
    snapshot.rows.forEach((runs, index) => {
      this.#renderRow(index, runs);
    });
    this.#moveCursor(snapshot.cursor);
  }

  #report(): void {
    const steps = this.#recording?.steps ?? [];
    let reached = -1;
    while ((steps[reached + 1]?.at ?? Infinity) <= this.#time) reached += 1;
    if (reached === this.#step) return;
    this.#step = reached;
    this.dispatchEvent(
      new CustomEvent("replay:step", { bubbles: true, detail: { index: reached } }),
    );
  }

  #restart(): void {
    const recording = this.#recording;
    if (recording === undefined) return;
    this.#next = this.#time = this.#remainder = 0;
    this.#previousCursor = this.#currentCursor = recording.initial.cursor;
    this.#render(recording.initial);
    this.#report();
  }

  #update(): void {
    const recording = this.#recording;
    if (recording === undefined) return;
    if (this.#animation !== undefined) cancelAnimationFrame(this.#animation);
    this.#animation = undefined;
    this.#lastTime = undefined;
    if (prefersReducedMotion()) {
      this.#render(recording.final);
      this.#time = this.#timeline.at(-1)?.at ?? 0;
      this.#next = this.#timeline.length;
      this.#report();
    }
    const active =
      this.#wantsPlay &&
      this.#inView &&
      !document.hidden &&
      !prefersReducedMotion() &&
      this.#next < this.#timeline.length;
    this.#status(active);
    if (active)
      this.#animation = requestAnimationFrame((now) => {
        this.#tick(now);
      });
  }

  #status(active: boolean): void {
    this.dataset["playing"] = String(active);
    required(this, "[data-replay-toggle]", HTMLButtonElement).textContent = active
      ? "Pause"
      : "Play";
  }

  #tick(now: number): void {
    this.#lastTime ??= now;
    this.#remainder += Math.max(0, Math.min(now - this.#lastTime, 100));
    this.#lastTime = now;
    while (this.#remainder + 1e-7 >= stepMs) {
      this.#remainder = Math.max(0, this.#remainder - stepMs);
      this.#time += stepMs * playbackRate;
      this.#previousCursor = this.#currentCursor;
      let update = this.#timeline[this.#next];
      while (update !== undefined && update.at <= this.#time) {
        this.#next += 1;
        for (const [index, runs] of update.rows) this.#renderRow(index, runs);
        this.#currentCursor = update.cursor;
        update = this.#timeline[this.#next];
      }
    }
    this.#report();
    const previous = this.#previousCursor;
    const current = this.#currentCursor;
    this.#moveCursor(
      previous && current && previous[1] === current[1] && Math.abs(current[0] - previous[0]) <= 3
        ? [previous[0] + ((current[0] - previous[0]) * this.#remainder) / stepMs, current[1]]
        : current,
    );
    if (this.#next < this.#timeline.length)
      this.#animation = requestAnimationFrame((next) => {
        this.#tick(next);
      });
    else {
      this.#moveCursor(this.#recording?.final.cursor);
      this.#animation = undefined;
      this.#wantsPlay = false;
      this.#status(false);
    }
  }

  #fit(): void {
    const columns = this.#recording?.columns;
    if (columns === undefined) return;
    const probe = required(this, ".cli-probe", HTMLSpanElement);
    const size = Number(getComputedStyle(this).fontSize.replace("px", ""));
    const unit = probe.getBoundingClientRect().width / 100 / size;
    const fitted = Math.floor((this.clientWidth / (columns * unit)) * 100) / 100;
    // Layout can briefly report a zero width or inconsistent probe while a viewport resizes.
    // Keep the last usable size instead of collapsing the terminal and its measuring probe.
    if (!Number.isFinite(fitted) || fitted <= 0) return;
    const nextSize = `${fitted}px`;
    if (this.style.fontSize === nextSize && this.#fittedWidth === this.clientWidth) return;
    this.#fittedWidth = this.clientWidth;
    this.style.fontSize = nextSize;
    this.dispatchEvent(new CustomEvent("replay:resize", { bubbles: true }));
  }

  disconnectedCallback(): void {
    this.#controller?.abort();
    this.#observer?.disconnect();
    this.#resize?.disconnect();
    if (this.#animation !== undefined) cancelAnimationFrame(this.#animation);
  }
}
customElements.define("cli-replay", CliReplay);
