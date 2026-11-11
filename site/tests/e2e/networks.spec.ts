/* eslint-disable no-await-in-loop -- Navigations and interactions on one page must be sequential. */
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

const routes = [
  "vulnerability-triage",
  "injury-claims",
  "device-reports",
  "aviation-safety",
  "contract-review",
];

async function expectNoOverlap(page: Page): Promise<void> {
  const overlaps = await page.locator("decision-net").evaluateAll((nets) =>
    nets.flatMap((net) => {
      if (!net.getBoundingClientRect().width || net.getAttribute("aria-hidden") === "true")
        return [];
      const cards = [...net.querySelectorAll<HTMLElement>(".node")].filter(
        (el) => getComputedStyle(el).display !== "none",
      );
      const boxes = cards.map((el) => ({ id: el.dataset["id"], rect: el.getBoundingClientRect() }));
      return boxes.flatMap((a, i) => {
        const problems = a.rect.left < 0 || a.rect.right > innerWidth ? [`outside ${a.id}`] : [];
        for (const b of boxes.slice(i + 1)) {
          if (
            a.rect.left < b.rect.right - 1 &&
            b.rect.left < a.rect.right - 1 &&
            a.rect.top < b.rect.bottom - 1 &&
            b.rect.top < a.rect.bottom - 1
          )
            problems.push(`${a.id} overlaps ${b.id}`);
        }
        return problems;
      });
    }),
  );
  expect(overlaps).toEqual([]);
}

for (const width of [390, 720, 1440]) {
  test(`every recorded network fits at ${width}px`, async ({ page }) => {
    await page.setViewportSize({ width, height: 900 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    for (const slug of routes) {
      await page.goto(`/examples/${slug}/`);
      await page.evaluate(() => document.fonts.ready);
      await expectNoOverlap(page);
    }
    await page.goto("/");
    for (const name of ["Support", "Finance", "Sales", "Operations", "Compliance"]) {
      await page.locator("net-tabs").getByRole("button", { name, exact: true }).click();
      await expectNoOverlap(page);
    }
    await page
      .locator("net-modes")
      .getByRole("button", { name: /Branch/u })
      .click();
    await expectNoOverlap(page);
  });
}

test("industry state survives reload, Back, and Forward", async ({ page }) => {
  await page.goto("/");
  const tabs = page.locator("net-tabs");
  await tabs.getByRole("button", { name: "Sales", exact: true }).click();
  await expect(page).toHaveURL(/industry=Sales/u);
  await page.reload();
  await expect(tabs.locator('[data-team-frame="Sales"]')).toBeVisible();
  await tabs.getByRole("button", { name: "Operations", exact: true }).click();
  await page.goBack();
  await expect(tabs.getByRole("button", { name: "Sales", exact: true })).toHaveAttribute(
    "aria-pressed",
    "true",
  );
  await page.goForward();
  await expect(tabs.locator('[data-team-frame="Operations"]')).toBeVisible();
});

test("keyboard tracing, static accessible probabilities, and live reduced motion", async ({
  page,
}) => {
  await page.goto("/");
  const net = page.locator("net-inspector decision-net");
  const primary = net.locator('[data-id="verdict"]');
  await primary.focus();
  await expect(primary).toHaveClass(/trace/u);
  await expect(net.locator('[data-id="email"]')).toHaveClass(/trace/u);
  await expect(net.locator(".on").first()).toHaveAttribute("aria-hidden", "true");
  await expect(net.locator(".sr-only").first()).toHaveText("0.88");
  await page.keyboard.press("Enter");
  const inspector = page.locator("[data-inspector]");
  await expect(inspector).toBeVisible();
  await page.locator("net-tabs button").first().focus();
  await expect(inspector).toBeHidden();
  await page.emulateMedia({ reducedMotion: "reduce" });
  await expect(net.locator(".wait, .active")).toHaveCount(0);
  await page.emulateMedia({ reducedMotion: "no-preference" });
});

test("all industry networks remain readable without JavaScript", async ({ browser }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  try {
    const page = await context.newPage();
    await page.goto("/");
    for (const name of ["Support", "Finance", "Sales", "Operations", "Compliance"])
      await expect(page.locator(`[data-team-frame="${name}"]`)).toBeVisible();
    await expect(page.locator("net-modes")).toContainText("Test a what-if");
  } finally {
    await context.close();
  }
});
