import { AxeBuilder } from "@axe-core/playwright";
import { expect, test } from "@playwright/test";
import type { AxeResults } from "axe-core";
import { builtPages } from "../support/site";

const wcag22aa = ["wcag2a", "wcag2aa", "wcag21a", "wcag21aa", "wcag22aa"];

const summarize = ({ violations }: AxeResults): string[] =>
  violations.map(
    ({ id, nodes }) => `${id}: ${nodes.map((node) => node.target.join(" ")).join(", ")}`,
  );

for (const width of [1440, 390]) {
  for (const url of builtPages()) {
    test(`${url} at ${width}px has no WCAG 2.2 AA violations`, async ({ page }) => {
      await page.setViewportSize({ width, height: 900 });
      await page.goto(url);
      const results = await new AxeBuilder({ page }).withTags(wcag22aa).analyze();
      expect(summarize(results)).toEqual([]);
    });
  }
}
