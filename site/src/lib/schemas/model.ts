import { z } from "astro/zod";

/** A published Nouli model: the facts its catalog card, page, and structured data share. */
const facts = {
  /** Position in the catalog, starting at 1. */
  order: z.number().int().positive(),
  /** Short name on cards, such as "Route". */
  name: z.string(),
  /** What the model does, under its name. */
  task: z.string(),
  /** The catalog filter it appears under. */
  category: z.enum(["text", "control"]),
  /** The area shown in the page eyebrow, such as "Text" or "Underwriting". */
  area: z.string(),
  /** The area shown on the catalog card. */
  cardArea: z.string(),
  version: z.string().regex(/^\d+\.\d+\.\d+$/u),
  released: z.iso.date(),
  description: z.string(),
  openGraph: z.object({
    title: z.enum(["title", "fullName"]),
    /** A root-relative image path. */
    image: z.string().startsWith("/").optional(),
    /** The link-preview card; pages choose it, since not every image suits the large card. */
    card: z.enum(["summary", "summary_large_image"]),
  }),
  /** The card's description. */
  summary: z.string().optional(),
  flow: z.object({ input: z.string(), output: z.string() }),
  /** Runtime or size note at the bottom of the card. */
  cardNote: z.string(),
  /** License names shown beside the page. */
  license: z.string(),
  /** A versioned release links to its version tag; a published release to a pinned revision. */
  structuredData: z.object({
    description: z.string(),
    runtimePlatform: z.string(),
    licenses: z.array(z.url()).min(1),
  }),
};

export const model = z
  .discriminatedUnion("release", [
    z.strictObject({ ...facts, release: z.literal("versioned") }),
    z.strictObject({ ...facts, release: z.literal("published"), revision: z.string().min(1) }),
  ])
  .transform((data) => {
    const fullName = `Nouli ${data.name}`;
    const title = `${fullName}: ${data.task}`;
    return {
      ...data,
      fullName,
      title,
      summary: data.summary ?? data.description,
      openGraph: { ...data.openGraph, title: data.openGraph.title === "title" ? title : fullName },
    };
  });
export type Model = z.infer<typeof model>;
