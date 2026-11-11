import { expect, test } from "@playwright/test";
import { builtPages } from "../support/site";

for (const url of builtPages()) {
  test(`${url} runs without CSP violations or browser errors`, async ({ page }) => {
    const errors: string[] = [];
    page.on("console", (message) => {
      if (message.type() === "error" && /Content Security Policy|violat/iu.test(message.text()))
        errors.push(message.text());
    });
    page.on("pageerror", (error) => errors.push(error.message));
    await page.addInitScript(() => {
      Object.assign(window, { cspViolations: [] });
      document.addEventListener("securitypolicyviolation", (event) => {
        const state = window;
        state.cspViolations.push(`${event.effectiveDirective}: ${event.blockedURI}`);
      });
    });
    await page.route("https://api.github.com/**", (route) =>
      route.fulfill({ json: { tag_name: "v0.5.1" } }),
    );
    await page.goto(url, { waitUntil: "networkidle" });
    expect(await page.evaluate(() => window.cspViolations)).toEqual([]);
    expect(errors).toEqual([]);
  });
}
