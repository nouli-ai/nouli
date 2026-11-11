import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { expect, test } from "vitest";
import { builtPages, distDir, fileFor } from "../support/site";

test.each(builtPages())("%s authorizes every inline script with a CSP hash", (url) => {
  const html = readFileSync(fileFor(distDir, url), "utf8");
  const policy = /http-equiv="Content-Security-Policy" content="([^"]+)"/iu.exec(html)?.[1];
  expect(policy).toBeDefined();
  for (const script of html.matchAll(/<script\b(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gu)) {
    if (script[0].includes('type="application/ld+json"')) continue;
    const hash = createHash("sha256")
      .update(script[1] ?? "")
      .digest("base64");
    expect(policy).toContain(`sha256-${hash}`);
  }
});
