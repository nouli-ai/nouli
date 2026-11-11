import { expect, test } from "vitest";
import { runStyle, terminalColor } from "../../src/lib/terminal/colors";

test("recorded colors resolve like the terminal drew them", () => {
  expect(terminalColor("default", "fallback")).toBe("fallback");
  expect(terminalColor("7dd0c0", "fallback")).toBe("#7dd0c0");
  expect(terminalColor("red", "fallback")).toBe("var(--term-red)");
  expect(terminalColor("unknown", "fallback")).toBe("fallback");
});

test("runs draw foreground, background, bold, and reverse video", () => {
  expect(runStyle(["x", "default", "default", false, false])).toBe("color:var(--term-fg)");
  expect(runStyle(["x", "red", "default", true, false])).toBe(
    "color:var(--term-red);font-weight:700",
  );
  expect(runStyle(["x", "default", "default", false, true])).toBe(
    "color:var(--term-bg);background-color:var(--term-fg)",
  );
  expect(runStyle(["x", "f8c553", "e2b03a", false, false])).toBe(
    "color:#f8c553;background-color:#e2b03a",
  );
});
