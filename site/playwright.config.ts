import { defineConfig, devices } from "@playwright/test";

export default defineConfig({
  forbidOnly: Boolean(process.env["CI"]),
  reporter: "list",
  use: { ...devices["Desktop Chrome"], baseURL: "http://localhost:4321" },
  projects: [{ name: "e2e", testDir: "tests/e2e" }],
  webServer: {
    // `--ignore-lock` keeps Astro in the foreground; it otherwise backgrounds itself under coding agents.
    command: "npm run preview -- --port 4321 --ignore-lock",
    url: "http://localhost:4321/",
    reuseExistingServer: false,
  },
});
