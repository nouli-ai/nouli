import { expect, test } from "vitest";
import {
  builtPages,
  builtUrls,
  distDir,
  publishedUrls,
  redirectPages,
  redirectTarget,
} from "../support/site";

// Files the build emits that are not part of the published URL contract.
const buildOnly = new Set<string>();

test("the build serves exactly the published URLs", () => {
  const built = builtUrls(distDir).filter((url) => !buildOnly.has(url));
  const published = publishedUrls();
  const builtSet = new Set(built);
  const publishedSet = new Set(published);
  expect({
    missing: published.filter((url) => !builtSet.has(url)),
    unlisted: built.filter((url) => !publishedSet.has(url)),
  }).toEqual({ missing: [], unlisted: [] });
});

test.each(redirectPages())("retired %s redirects to a built page", (url) => {
  expect(builtPages()).toContain(redirectTarget(url));
});
