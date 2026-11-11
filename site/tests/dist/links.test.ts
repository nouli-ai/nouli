import { existsSync, readFileSync, statSync } from "node:fs";
import { parse } from "parse5";
import type { DefaultTreeAdapterMap } from "parse5";
import { describe, expect, test } from "vitest";
import { builtPages, distDir, fileFor, publishedOrigin } from "../support/site";

type Node = DefaultTreeAdapterMap["node"];
type Element = DefaultTreeAdapterMap["element"];

function walk(node: Node, visit: (element: Element) => void): void {
  if ("tagName" in node) visit(node);
  const children =
    "content" in node ? node.content.childNodes : "childNodes" in node ? node.childNodes : [];
  for (const child of children) walk(child, visit);
}

/** URLs an element refers to: links, media, responsive candidates, and Open Graph URLs. */
function urlsOf(element: Element): string[] {
  const attributes = new Map(element.attrs.map(({ name, value }) => [name, value]));
  const urls = ["href", "src", "poster"].flatMap((name) => attributes.get(name) ?? []);
  const srcset = attributes.get("srcset");
  if (srcset !== undefined && srcset !== "")
    urls.push(...srcset.split(",").map((candidate) => candidate.trim().split(/\s+/u)[0] ?? ""));
  const property = attributes.get("property") ?? "";
  const content = attributes.get("content") ?? "";
  if (element.tagName === "meta" && property.startsWith("og:") && /^https?:\/\//u.test(content)) {
    urls.push(content);
  }
  return urls;
}

function readPage(url: string): { urls: string[]; ids: Set<string> } {
  const urls: string[] = [];
  const ids = new Set<string>();
  walk(parse(readFileSync(fileFor(distDir, url), "utf8")), (element) => {
    urls.push(...urlsOf(element));
    const id = element.attrs.find(({ name }) => name === "id")?.value;
    if (id !== undefined) ids.add(id);
  });
  return { urls, ids };
}

const pages = builtPages();
const read = new Map(pages.map((url) => [url, readPage(url)]));
const lastSegment = (pathname: string): string => pathname.split("/").at(-1) ?? "";

/** Why a same-site URL is broken, or null when it resolves to a built file and anchor. */
function problem(target: URL): string | null {
  const { pathname } = target;
  if (!pathname.endsWith("/") && !lastSegment(pathname).includes(".")) {
    return "page URL without a trailing slash";
  }
  const file = fileFor(distDir, pathname);
  if (!existsSync(file) || !statSync(file).isFile()) return `no file for ${pathname}`;
  const anchor = decodeURIComponent(target.hash.slice(1));
  if (anchor && read.get(pathname)?.ids.has(anchor) !== true) return `no #${anchor}`;
  return null;
}

describe.each(pages)("links on %s", (page) => {
  test("each same-site URL reaches a built file and anchor", () => {
    const broken = (read.get(page)?.urls ?? []).flatMap((link) => {
      const target = new URL(link, publishedOrigin + page);
      if (target.origin !== publishedOrigin) return [];
      const reason = problem(target);
      return reason === null ? [] : [`${link} (${reason})`];
    });
    expect(broken).toEqual([]);
  });
});
