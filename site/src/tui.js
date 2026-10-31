// Real PTY screen captures, replaying the site's existing recorded model answers.
const data = import.meta.glob("./data/tui/*.json", { import: "default" });
const colors = {
  default: "#d1d5db",
  black: "#15191d",
  red: "#ef4444",
  green: "#22c55e",
  brown: "#eab308",
  blue: "#3b82f6",
  magenta: "#ec4899",
  cyan: "#06b6d4",
  white: "#d1d5db",
  brightblack: "#737373",
  brightwhite: "#fff",
};
const color = (name, fallback) =>
  name === "default"
    ? fallback
    : /^[a-f0-9]{6}$/i.test(name)
      ? `#${name}`
      : colors[name] || fallback;

export async function capture(name) {
  return data[`./data/tui/${name}.json`]?.();
}

export function showCapture(root, frame) {
  let view = "answer";
  root.replaceChildren();
  const controls = document.createElement("div");
  controls.className = "tui-controls";
  const title = document.createElement("p");
  title.className = "tui-caption";
  title.textContent = frame.title;
  const screen = document.createElement("pre");
  screen.className = "tui-screen";
  screen.tabIndex = 0;
  screen.setAttribute("aria-label", `Captured Nouli terminal: ${frame.title}`);
  const buttons = ["Form", "Answer", "Decisions"].map((label) => {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = label;
    button.addEventListener("click", () => {
      view = label.toLowerCase();
      draw();
    });
    controls.append(button);
    return button;
  });
  const note = document.createElement("span");
  note.textContent = "current TUI · recorded answers";
  controls.append(note);
  root.append(controls, screen, title);
  function draw() {
    const narrow = root.clientWidth < 1000;
    const snapshot =
      view === "decisions"
        ? frame.graph
        : view === "form"
          ? narrow
            ? frame.draftNarrow
            : frame.draft
          : narrow
            ? frame.narrow
            : frame.answer;
    buttons.forEach((button) =>
      button.setAttribute(
        "aria-pressed",
        String(button.textContent.toLowerCase() === view),
      ),
    );
    const grid = document.createElement("span");
    grid.className = "tui-grid";
    grid.style.minWidth = `${snapshot.columns}ch`;
    for (const runs of snapshot.rows) {
      const row = document.createElement("span");
      row.className = "tui-row";
      for (const [text, fg, bg, bold, reverse] of runs) {
        const span = document.createElement("span");
        span.textContent = text;
        const foreground = color(fg, "#d1d5db"),
          background = color(bg, "#15191d");
        span.style.color = reverse ? background : foreground;
        if (bg !== "default" || reverse)
          span.style.backgroundColor = reverse ? foreground : background;
        if (bold) span.style.fontWeight = "700";
        row.append(span);
      }
      grid.append(row);
    }
    screen.replaceChildren(grid);
  }
  const observer = new ResizeObserver(draw);
  observer.observe(root);
  draw();
  return () => observer.disconnect();
}

export function mountCaptures() {
  document
    .querySelectorAll(".tui-snapshot[data-capture]")
    .forEach(async (root) => {
      const frame = (await capture(root.dataset.capture))?.frames.find(
        (frame) => frame.step === Number(root.dataset.step),
      );
      if (frame) showCapture(root, frame);
    });
}
