import { experimental_AstroContainer as AstroContainer } from "astro/container";
import { describe, expect, test } from "vitest";
import BaseHead from "../../src/components/site/BaseHead.astro";
import BaseLayout from "../../src/layouts/BaseLayout.astro";
import HomeNav from "../../src/components/hive/HomeNav.astro";
import { serializeJsonLd } from "../../src/lib/seo/json-ld";

const container = await AstroContainer.create();
const at = (path: string): Request => new Request(`https://nouli.ai${path}`);

describe("BaseHead", () => {
  test("builds the canonical URL from the page path", async () => {
    const html = await container.renderToString(BaseHead, {
      props: { title: "Guide · Nouli", description: "How to use Nouli." },
      request: at("/guide/"),
    });
    expect(html).toContain('<link rel="canonical" href="https://nouli.ai/guide/">');
    expect(html).toContain('<meta name="description" content="How to use Nouli.">');
  });

  test("leaves out Open Graph tags unless the page provides them", async () => {
    const html = await container.renderToString(BaseHead, {
      props: { title: "Guide · Nouli", description: "How to use Nouli." },
      request: at("/guide/"),
    });
    expect(html).not.toContain("og:");
    expect(html).not.toContain("twitter:card");
  });

  test("uses the preview card the page chooses", async () => {
    const withImage = await container.renderToString(BaseHead, {
      props: {
        title: "Route · Nouli",
        description: "Banking intents.",
        openGraph: {
          title: "Route",
          image: "https://nouli.ai/models/nouli-route/comparison.png",
          card: "summary_large_image",
        },
      },
      request: at("/models/nouli-route/"),
    });
    expect(withImage).toContain(
      '<meta property="og:url" content="https://nouli.ai/models/nouli-route/">',
    );
    expect(withImage).toContain('content="summary_large_image"');
    const without = await container.renderToString(BaseHead, {
      props: {
        title: "Nouli",
        description: "Decisions.",
        openGraph: { title: "Nouli", card: "summary" },
      },
      request: at("/"),
    });
    expect(without).toContain('<meta name="twitter:card" content="summary">');
  });
});

test("BaseHead writes structured data inline, with < escaped", async () => {
  const html = await container.renderToString(BaseHead, {
    props: { title: "Nouli", description: "Decisions.", structuredData: { name: "</script>" } },
    request: at("/"),
  });
  const json = html.match(/<script type="application\/ld\+json">([^<]*)<\/script>/u)?.[1];
  expect(json).toBe('{"name":"\\u003c/script>"}');
});

test("the skip link targets the page's main element", async () => {
  const html = await container.renderToString(BaseLayout, {
    props: { title: "Nouli", description: "Decisions." },
    request: at("/"),
  });
  const target = html.match(/<a\b[^>]*\bhref="#([^"]+)"[^>]*>Skip to content<\/a>/u)?.[1];
  expect(target).toBeDefined();
  expect(html).toContain(`<main id="${target ?? ""}"`);
});

describe("HomeNav", () => {
  test("marks only the current section", async () => {
    const html = await container.renderToString(HomeNav, { props: { current: "examples" } });
    expect(html.match(/aria-current="page"/gu)).toHaveLength(1);
    expect(html).toMatch(/href="\/examples\/"[^>]*aria-current="page"/u);
  });
});

test("structured data cannot close its script element", () => {
  expect(serializeJsonLd({ name: "</script><b>" })).toBe('{"name":"\\u003c/script>\\u003cb>"}');
});
