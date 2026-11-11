import { z } from "astro/zod";

/** A run of terminal cells sharing one style: text, foreground, background, bold, reverse. */
export const cellRun = z.tuple([z.string(), z.string(), z.string(), z.boolean(), z.boolean()]);
export type CellRun = z.infer<typeof cellRun>;

/** One recorded terminal screen: each row is a list of styled runs. */
export const screen = z.object({
  columns: z.number().int().positive(),
  rows: z.array(z.array(cellRun)),
});
export type Screen = z.infer<typeof screen>;

/** A cursor cell as [column, row], or null when the cursor is hidden. */
export const cursor = z.tuple([z.number().int(), z.number().int()]).nullable();
