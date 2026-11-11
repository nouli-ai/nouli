import type { CellRun } from "../schemas/terminal";

/** Terminal default colors and the standard palette for named ANSI colors, as design tokens. */
export const terminalForeground = "var(--term-fg)";
export const terminalBackground = "var(--term-bg)";

const named: Readonly<Record<string, string>> = {
  black: terminalBackground,
  red: "var(--term-red)",
  green: "var(--term-green)",
  brown: "var(--term-brown)",
  blue: "var(--term-blue)",
  magenta: "var(--term-magenta)",
  cyan: "var(--term-cyan)",
  white: terminalForeground,
  brightblack: "var(--term-bright-black)",
  brightwhite: "var(--term-bright-white)",
};

/** A recorded color: "default" and unknown names use the fallback; six hex digits pass through. */
export function terminalColor(value: string, fallback: string): string {
  if (value === "default") return fallback;
  if (/^[a-f0-9]{6}$/iu.test(value)) return `#${value}`;
  return named[value] ?? fallback;
}

/** The inline style that draws a recorded run as the terminal app drew it. */
export function runStyle([, foreground, background, bold, reverse]: CellRun): string {
  const fg = terminalColor(foreground, terminalForeground);
  const bg = terminalColor(background, terminalBackground);
  const declarations = [`color:${reverse ? bg : fg}`];
  if (background !== "default" || reverse)
    declarations.push(`background-color:${reverse ? fg : bg}`);
  if (bold) declarations.push("font-weight:700");
  return declarations.join(";");
}
