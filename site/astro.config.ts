import { defineConfig } from "astro/config";
import { sitemap } from "./integrations/sitemap";
import { siteOrigin } from "./src/lib/site";

export default defineConfig({
  site: siteOrigin,
  trailingSlash: "always",
  build: { format: "directory" },
  markdown: { syntaxHighlight: false },
  // Scope with :where() so component selectors keep the specificity they are written with.
  scopedStyleStrategy: "where",
  integrations: [sitemap()],
  // The walkthroughs the examples replaced; published URLs keep working.
  redirects: {
    "/examples/vulnerability-response/": "/examples/vulnerability-triage/",
    "/examples/conservation/": "/examples/",
    "/examples/credit-card-limit/": "/examples/",
    "/examples/loan-approval/": "/examples/",
    "/examples/meeting/": "/examples/",
    "/examples/recall/": "/examples/",
    "/examples/robot-picking/": "/examples/",
    "/examples/seed-investment/": "/examples/",
  },
  security: {
    // Astro hashes every script and stylesheet it emits into a <meta> policy.
    csp: {
      directives: [
        "default-src 'self'",
        // The paper grain is an inline SVG data URL.
        "img-src 'self' data:",
        "font-src 'self'",
        "media-src 'self'",
        // The home page looks up the latest release.
        "connect-src 'self' https://api.github.com",
        "object-src 'none'",
        "base-uri 'self'",
        "form-action 'none'",
      ],
      scriptDirective: { resources: ["'self'"] },
      // Recorded terminal colors and readout strip widths are style attributes.
      styleDirective: { resources: ["'self'", { resource: "'unsafe-inline'", kind: "attribute" }] },
    },
  },
});
