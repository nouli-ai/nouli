import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";
import { expect, test } from "vitest";
import { siteRoot } from "../support/site";

const sourceDir = join(siteRoot, "src");

// Astro's `set:html` skips escaping. Templates are not linted, so this search enforces the rule.
// JsonLd.astro is the one exception: it emits JSON with `<` escaped, which entity escaping would break.
const allowed = new Set([join(sourceDir, "components/site/JsonLd.astro")]);

test("no source file renders unescaped HTML", () => {
  const uses = readdirSync(sourceDir, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile() && !allowed.has(join(entry.parentPath, entry.name)))
    .map((entry) => join(entry.parentPath, entry.name))
    .flatMap((path) =>
      readFileSync(path, "utf8")
        .split("\n")
        .flatMap((line, index) =>
          line.includes("set:html") ? [`${relative(siteRoot, path)}:${index + 1}`] : [],
        ),
    );
  expect(uses).toEqual([]);
});
