import { writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import type { AstroIntegration } from "astro";

/** Write /sitemap.xml, linked from robots.txt, listing every built page under the site URL. */
export function sitemap(): AstroIntegration {
  let site: URL | undefined;
  return {
    name: "nouli-sitemap",
    hooks: {
      "astro:config:done": ({ config }) => {
        if (config.site === undefined)
          throw new Error("The sitemap needs `site` in astro.config.ts.");
        site = new URL(config.site);
      },
      "astro:build:done": ({ dir, pages }) => {
        if (site === undefined) throw new Error("The sitemap ran before the config was resolved.");
        const base = site;
        const entries = pages
          .map(({ pathname }) => new URL(pathname, base).href)
          .map((url) => `  <url><loc>${url}</loc></url>`);
        const xml = [
          '<?xml version="1.0" encoding="UTF-8"?>',
          '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">',
          ...entries,
          "</urlset>",
          "",
        ].join("\n");
        writeFileSync(fileURLToPath(new URL("sitemap.xml", dir)), xml);
      },
    },
  };
}
