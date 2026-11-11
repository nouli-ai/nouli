import { readFileSync } from "node:fs";
import { join, posix } from "node:path";
import { expect, test } from "vitest";
import { builtPages, distDir, fileFor } from "../support/site";

/**
 * Bytes of JavaScript each kind of page may load, bundled or inlined, including modules those
 * scripts import. Pages that match no entry may load none.
 */
const budgets: [pattern: RegExp, bytes: number][] = [
  // The live networks (about 6.6 kB), the terminal replay (6.4 kB), the sky and its pointer comb
  // (4.8 kB), the tabs, inspector, operation cards and reveals (3.3 kB), the floating navigation,
  // copy controls and the release lookup; recordings load on demand.
  [/^\/$/u, 26_000],
  // Every other page has the sky header and floating navigation (about 6 kB), plus:
  // the capture view buttons in the guide,
  [/^\/guide\/$/u, 7500],
  // the catalog's model filter,
  [/^\/models\/$/u, 8500],
  [/^\/examples\/$/u, 7000],
  // the recorded case's network,
  [/^\/examples\/[^/]+\/$/u, 14_000],
  // and copy buttons on model pages.
  [/^\/models\/[^/]+\/$/u, 8500],
];

const budgetFor = (url: string): number => budgets.find(([pattern]) => pattern.test(url))?.[1] ?? 0;

/** A bundled module and every bundled module it imports, by root-relative path. */
function withImports(path: string, seen: Set<string>): Set<string> {
  if (seen.has(path)) return seen;
  seen.add(path);
  const source = readFileSync(join(distDir, path), "utf8");
  for (const match of source.matchAll(/(?:import|from)\s*["'](\.{1,2}\/[^"']+\.js)["']/gu)) {
    withImports(posix.join(posix.dirname(path), match[1] ?? ""), seen);
  }
  return seen;
}

/** Bundled modules the page loads plus scripts inlined into it; structured data is not script. */
function scriptBytes(url: string): number {
  const html = readFileSync(fileFor(distDir, url), "utf8");
  const modules = new Set<string>();
  for (const match of html.matchAll(
    /<(?:script|link)\b[^>]*?(?:src|href)="(\/_astro\/[^"]+\.js)"/gu,
  )) {
    withImports(match[1] ?? "", modules);
  }
  const bundled = [...modules].reduce(
    (total, path) => total + Buffer.byteLength(readFileSync(join(distDir, path))),
    0,
  );
  const inline = [...html.matchAll(/<script\b(?![^>]*\bsrc=)([^>]*)>([\s\S]*?)<\/script>/gu)]
    .filter((match) => !(match[1] ?? "").includes("application/ld+json"))
    .reduce((total, match) => total + Buffer.byteLength(match[2] ?? ""), 0);
  return bundled + inline;
}

test.each(builtPages())("%s stays within its JavaScript budget", (url) => {
  expect(scriptBytes(url)).toBeLessThanOrEqual(budgetFor(url));
});
