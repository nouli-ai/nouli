import { expect, test } from "@playwright/test";

test.describe("model filter", () => {
  test("keeps the chosen category in the URL, and Back restores the previous one", async ({
    page,
  }) => {
    await page.goto("/models/");
    const cards = page.locator("[data-model-category]:visible");
    const all = await cards.count();
    await page.getByRole("button", { name: "Control", exact: true }).click();
    await page.getByRole("button", { name: "Control", exact: true }).click();
    await expect(page).toHaveURL(/\?category=control$/u);
    await expect(page.locator("[data-model-count]")).toHaveText(`${await cards.count()} models`);
    expect(await cards.count()).toBeLessThan(all);
    await page.goBack();
    await expect(cards).toHaveCount(all);
    await expect(page.getByRole("button", { name: "All models" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });

  test("opens a shared filtered link with that filter applied", async ({ page }) => {
    await page.goto("/models/?category=text");
    await expect(page.getByRole("button", { name: "Text", exact: true })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
    await expect(page.locator('[data-model-category="control"]:visible')).toHaveCount(0);
  });

  test("ignores an unknown category", async ({ page }) => {
    await page.goto("/models/?category=unknown");
    await expect(page.getByRole("button", { name: "All models" })).toHaveAttribute(
      "aria-pressed",
      "true",
    );
  });
});

for (const width of [1440, 390]) {
  test(`a shared control filter avoids layout shift at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(() => {
      Object.assign(window, { cls: 0 });
      new PerformanceObserver((list) => {
        const state = window;
        for (const entry of list.getEntries()) {
          if (
            "hadRecentInput" in entry &&
            entry.hadRecentInput === false &&
            "value" in entry &&
            typeof entry.value === "number"
          )
            state.cls += entry.value;
        }
      }).observe({ type: "layout-shift", buffered: true });
    });
    await page.goto("/models/?category=control", { waitUntil: "networkidle" });
    await expect(page.locator('[data-model-category="text"]:visible')).toHaveCount(0);
    expect(await page.evaluate(() => window.cls)).toBeLessThan(0.1);
  });
}

test("an explicit all category keeps every card visible", async ({ page }) => {
  await page.goto("/models/?category=all");
  await expect(page.locator("[data-model-category]:visible")).toHaveCount(9);
});

test("a failed copy tells the reader to select the text", async ({ page }) => {
  await page.addInitScript(() => {
    Object.defineProperty(navigator, "clipboard", {
      value: { writeText: () => Promise.reject(new Error("Clipboard denied")) },
    });
  });
  await page.goto("/models/nouli-route/");
  const block = page.locator("copy-text").first();
  await block.getByRole("button").click();
  await expect(block.getByRole("button")).toHaveText("select it");
  await expect(block.getByRole("status")).toHaveText("Copying failed; select the text instead");
});

test("a copy button copies its code without the trailing newline and confirms", async ({
  page,
  context,
}) => {
  await context.grantPermissions(["clipboard-read", "clipboard-write"]);
  await page.goto("/models/nouli-route/");
  const block = page.locator("copy-text").first();
  const code = (await block.locator("code").textContent()) ?? "";
  const button = block.getByRole("button");
  await button.click();
  await expect(button).toHaveText("copied");
  await expect(block.getByRole("status")).toHaveText("Copied to the clipboard");
  expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(code.trimEnd());
  await expect(button).toHaveText("copy", { timeout: 3000 });
});
