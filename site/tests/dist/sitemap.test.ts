import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "vitest";
import { builtPages, distDir, publishedOrigin } from "../support/site";

test("the sitemap lists every built page", () => {
  const listed = [
    ...readFileSync(join(distDir, "sitemap.xml"), "utf8").matchAll(/<loc>([^<]+)<\/loc>/gu),
  ]
    .map((match) => match[1] ?? "")
    .toSorted();
  expect(listed).toEqual(
    builtPages()
      .map((page) => publishedOrigin + page)
      .toSorted(),
  );
});
