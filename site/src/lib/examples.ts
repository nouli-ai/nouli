import type { Example } from "./schemas/example";
import type { Network } from "./home/networks";

/** An examples content entry: its slug and its data. */
export interface ExampleEntry {
  id: string;
  data: Example;
}

/** The page URL for an example's slug. */
export const examplePath = (slug: string): string => `/examples/${slug}/`;

/** Examples in index order. */
export const inOrder = <T extends ExampleEntry>(entries: readonly T[]): T[] =>
  entries.toSorted((a, b) => a.data.order - b.data.order);

/** An example's recorded case as a network the site can draw. */
export const networkOf = (entry: ExampleEntry): Network => ({
  label: entry.data.title,
  session: entry.id,
  steps: entry.data.steps,
});

/** The smallest and largest number of cases any example's network ran on. */
export function sampleRange(entries: readonly ExampleEntry[]): { min: number; max: number } {
  const counts = entries.map((entry) => entry.data.sample.count);
  return { min: Math.min(...counts), max: Math.max(...counts) };
}
