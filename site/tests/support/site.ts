import { existsSync, readdirSync, readFileSync } from "node:fs";
import { join, relative, sep } from "node:path";

export const siteRoot = join(import.meta.dirname, "../..");
export const distDir = join(siteRoot, "dist");
export const publishedOrigin = "https://nouli.ai";

/** Every URL the site publishes, in the inventory's `/path/` form. */
export function publishedUrls(): string[] {
  return readFileSync(join(siteRoot, "tests/dist/published-urls.txt"), "utf8")
    .split("\n")
    .filter((line) => line.length > 0);
}

export const isPage = (url: string): boolean => url.endsWith("/");

/** The file a URL is served from inside a build directory. */
export function fileFor(dir: string, url: string): string {
  return isPage(url) ? join(dir, url, "index.html") : join(dir, url);
}

/** Every URL a build directory serves, in the inventory's form; bundled assets are left out. */
export function builtUrls(dir: string): string[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => "/" + relative(dir, join(entry.parentPath, entry.name)).split(sep).join("/"))
    .filter(
      (url) =>
        !url.startsWith("/_astro/") && !url.startsWith("/assets/") && !url.endsWith(".DS_Store"),
    )
    .map((url) => (url.endsWith("/index.html") ? url.slice(0, -"index.html".length) : url))
    .toSorted();
}

/** Where a built redirect page sends the reader, or undefined for an ordinary page. */
export function redirectTarget(url: string): string | undefined {
  const html = readFileSync(fileFor(distDir, url), "utf8");
  return /<meta http-equiv="refresh" content="\d+;url=([^"]+)"/u.exec(html)?.[1];
}

/** Pages in the new build, without the redirect pages that keep retired URLs working. */
export function builtPages(): string[] {
  return builtUrls(distDir).filter((url) => isPage(url) && redirectTarget(url) === undefined);
}

/** Retired URLs the build serves as redirect pages. */
export function redirectPages(): string[] {
  return builtUrls(distDir).filter((url) => isPage(url) && redirectTarget(url) !== undefined);
}
