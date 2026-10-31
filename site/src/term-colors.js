// Recorded terminal colors drawn exactly as the app sets them. Hex colors from the
// TUI pass through unchanged; named ANSI colors use a standard terminal palette.
export const BACKGROUND = "#15191d";
export const FOREGROUND = "#d1d5db";

const NAMED = {
  black: BACKGROUND,
  red: "#ef4444",
  green: "#22c55e",
  brown: "#eab308",
  blue: "#3b82f6",
  magenta: "#ec4899",
  cyan: "#06b6d4",
  white: FOREGROUND,
  brightblack: "#737373",
  brightwhite: "#ffffff",
};

/** The page color for a recorded foreground or background color name. */
export function color(value, fallback) {
  if (value === "default") return fallback;
  if (/^[a-f0-9]{6}$/i.test(value)) return `#${value}`;
  return NAMED[value] ?? fallback;
}

/** A recorded run of cells as a styled span. */
export function cellSpan(text, fg, bg, bold, reverse) {
  const span = document.createElement("span");
  span.textContent = text;
  const foreground = color(fg, FOREGROUND);
  const background = color(bg, BACKGROUND);
  span.style.color = reverse ? background : foreground;
  if (bg !== "default" || reverse) span.style.backgroundColor = reverse ? foreground : background;
  if (bold) span.style.fontWeight = "700";
  return span;
}
