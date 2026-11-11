/* eslint-disable no-await-in-loop -- Navigations on one page must be sequential. */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";
import { builtPages } from "../support/site";

/** Whether the element matching `selector` is the topmost element at its own center. */
const onTop = (page: Page, selector: string): Promise<boolean> =>
  page
    .locator(selector)
    .first()
    .evaluate((el) => {
      const r = el.getBoundingClientRect();
      const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
      return hit !== null && (hit === el || el.contains(hit));
    });

for (const width of [600, 720]) {
  test(`no page scrolls sideways at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    for (const url of builtPages()) {
      await page.goto(url);
      const overflow = await page.evaluate(
        () => document.documentElement.scrollWidth - document.documentElement.clientWidth,
      );
      expect(overflow, url).toBeLessThanOrEqual(0);
    }
  });
}

for (const width of [1440, 390]) {
  test(`the floating nav stays above network panels at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    for (const url of ["/", "/examples/contract-review/"]) {
      await page.goto(url);
      const panel = page.locator(".sky.panel").first();
      await panel.evaluate((el) => {
        scrollTo(0, el.getBoundingClientRect().top + scrollY - 20);
      });
      await expect(page.locator("float-nav")).toHaveAttribute("data-floating", "");
      expect(await onTop(page, "float-nav .install"), url).toBe(true);
    }
  });
}

test("the skip link is visible when focused", async ({ page }) => {
  for (const url of ["/", "/guide/"]) {
    await page.goto(url);
    await page.keyboard.press("Tab");
    expect(await onTop(page, ".skip"), url).toBe(true);
  }
});
