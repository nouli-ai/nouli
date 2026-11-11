import type { Model } from "./schemas/model";
import { huggingFace } from "./site";

export interface ModelSectionInfo {
  id: string;
  title: string;
}

/**
 * A lookup from section id to its heading, from the same list that builds the page's table of
 * contents, so the contents and the headings cannot drift apart.
 */
export function sectionLookup(
  sections: readonly ModelSectionInfo[],
): (id: string) => ModelSectionInfo {
  return (id) => {
    const found = sections.find((entry) => entry.id === id);
    if (found === undefined) throw new Error(`No section "${id}" in this model page's contents`);
    return found;
  };
}

export const modelPath = (slug: string): string => `/models/${slug}/`;
export const modelRepository = (slug: string): string => `${huggingFace}/${slug}`;

/** What a release offers and where its files are, by release kind. */
export function releaseDetails(slug: string, data: Model) {
  const revision = data.release === "versioned" ? `v${data.version}` : data.revision;
  const files = `${modelRepository(slug)}/tree/${revision}`;
  return data.release === "versioned"
    ? {
        offers:
          "Weights, local serving code, a Nouli CLI workflow, and curated provenance. Upstream notices apply.",
        files,
        filesLabel: `Browse v${data.version} files ↗`,
        note: "Inference runs on your machine. Hugging Face hosts the files.",
      }
    : {
        offers:
          "Published model files, local inference instructions, recorded examples and upstream notices.",
        files,
        filesLabel: "Browse published files ↗",
        note: "Inference runs on your machine. Follow the model's documented hardware requirements.",
      };
}

/** A release date as the pages print it, such as "2 November 2026". */
export const releaseDate = (isoDate: string): string =>
  new Intl.DateTimeFormat("en-GB", {
    day: "numeric",
    month: "long",
    year: "numeric",
    timeZone: "UTC",
  }).format(new Date(`${isoDate}T00:00:00Z`));

/** Two-digit catalog position, such as "06". */
export const catalogNumber = (order: number): string => String(order).padStart(2, "0");

const words = [
  "Zero",
  "One",
  "Two",
  "Three",
  "Four",
  "Five",
  "Six",
  "Seven",
  "Eight",
  "Nine",
  "Ten",
  "Eleven",
  "Twelve",
];

/** A count as a capitalized word, for copy such as "Nine open-weight models". */
export function countWord(count: number): string {
  const word = words[count];
  if (word === undefined) throw new Error(`No word for ${count}`);
  return word;
}
