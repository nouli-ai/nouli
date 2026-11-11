/** Site-wide facts: where Nouli lives, how to install it, and the shared navigation. */

export const siteOrigin = "https://nouli.ai";
export const repository = "https://github.com/nouli-ai/nouli";
export const huggingFace = "https://huggingface.co/nouli-ai";
export const latestRelease = `${repository}/releases/latest`;

export const install = {
  homebrew: "brew install nouli-ai/tap/nouli",
  script: `curl -fsSL ${siteOrigin}/install.sh | sh`,
} as const;

export interface Link {
  href: string;
  label: string;
}

/** The section of the site a page belongs to, marked as current in the navigation. */
export type Section = "models" | "guide" | "examples";

/** The main navigation. */
export const hiveNavigation = [
  { href: "/examples/", label: "Examples", section: "examples" },
  { href: "/guide/", label: "Guide", section: "guide" },
  { href: "/models/", label: "Models", section: "models" },
] as const;
