import { repository, siteOrigin, huggingFace } from "../site";
export const homeTitle = "Nouli · the platform for decision networks";
export const homeDescription =
  "Nouli is a free, open-source CLI and Rust library for decision networks: chain small AI decisions into big ones, and trace every step.";
/** Structured data for the published terminal application. */
export const homeApplication = {
  "@context": "https://schema.org",
  "@type": "SoftwareApplication",
  "@id": `${siteOrigin}/#software`,
  name: "Nouli",
  url: `${siteOrigin}/`,
  description: homeDescription,
  applicationCategory: "DeveloperApplication",
  operatingSystem: "macOS, Linux",
  license: `${repository}/blob/main/LICENSE`,
  sameAs: [repository, huggingFace],
  publisher: {
    "@type": "Organization",
    name: "Nouli",
    url: `${siteOrigin}/`,
    logo: `${siteOrigin}/brand/bee.svg`,
  },
  softwareHelp: {
    "@type": "CreativeWork",
    url: `${siteOrigin}/guide/`,
  },
};
