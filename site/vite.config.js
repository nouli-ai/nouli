import { readdirSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { defineConfig } from "vite";

const examples = readdirSync(resolve(import.meta.dirname, "examples"), { withFileTypes: true })
  .filter((d) => d.isDirectory())
  .map((d) => [`examples/${d.name}`, resolve(import.meta.dirname, "examples", d.name, "index.html")]);

// The release the install commands fetch, from the workspace manifest.
const version = readFileSync(resolve(import.meta.dirname, "../Cargo.toml"), "utf8").match(/^version = "([^"]+)"/m)[1];

export default defineConfig({
  plugins: [{ name: "nouli-version", transformIndexHtml: (html) => html.replaceAll("%NOULI_VERSION%", version) }],
  // Relative URLs let the same build work at any GitHub Pages path.
  base: "./",
  build: {
    rollupOptions: {
      input: {
        main: resolve(import.meta.dirname, "index.html"),
        guide: resolve(import.meta.dirname, "guide/index.html"),
        examples: resolve(import.meta.dirname, "examples/index.html"),
        ...Object.fromEntries(examples),
      },
    },
  },
});
