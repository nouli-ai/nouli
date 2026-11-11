/// <reference types="vitest/config" />
import { getViteConfig } from "astro/config";

export default getViteConfig({
  test: {
    projects: [
      { extends: true, test: { name: "unit", include: ["tests/unit/**/*.test.ts"] } },
      { test: { name: "dist", include: ["tests/dist/**/*.test.ts"], environment: "node" } },
    ],
  },
});
