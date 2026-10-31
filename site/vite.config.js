import { readdirSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig } from "vite";

const examples = readdirSync(resolve(import.meta.dirname, "examples"), { withFileTypes: true })
  .filter((d) => d.isDirectory())
  .map((d) => [`examples/${d.name}`, resolve(import.meta.dirname, "examples", d.name, "index.html")]);

const pages = {
  main: resolve(import.meta.dirname, "index.html"),
  guide: resolve(import.meta.dirname, "guide/index.html"),
  examples: resolve(import.meta.dirname, "examples/index.html"),
  ...Object.fromEntries(examples),
};
const publishedBase = "https://nouli.ai/";
const pageUrls = new Map(Object.entries(pages).map(([route, filename]) => [
  filename,
  new URL(route === "main" ? "" : route + "/", publishedBase).href,
]));

export default defineConfig({
  // Relative URLs let the same build work at any GitHub Pages path.
  base: "./",
  plugins: [{
    name: "nouli-search-metadata",
    transformIndexHtml(html, context) {
      const url = pageUrls.get(context.filename);
      if (!url) return [];
      if (!/<meta\s+name="description"\s+content="[^"]+"/.test(html)) {
        throw new Error("Missing page description: " + context.filename);
      }
      return [{
        tag: "link",
        attrs: { rel: "canonical", href: url },
        injectTo: "head",
      }];
    },
    generateBundle() {
      const urls = [...pageUrls.values()]
        .map((url) => "  <url><loc>" + url + "</loc></url>").join("\n");
      this.emitFile({
        type: "asset",
        fileName: "sitemap.xml",
        source: '<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n' + urls + "\n</urlset>\n",
      });
    },
  }],
  build: {
    rollupOptions: {
      input: pages,
    },
  },
});
