import { z } from "astro/zod";
import { cellRun, cursor, screen } from "./terminal";

const snapshot = z.object({ rows: screen.shape.rows, cursor });

/** A terminal session recorded over time: an initial screen, then row updates after each wait. */
export const replay = z.object({
  version: z.literal(3),
  application: z.string(),
  columns: z.number().int().positive(),
  lines: z.number().int().positive(),
  initial: snapshot,
  updates: z.array(
    z.object({
      /** Milliseconds since the previous update, at recorded speed. */
      wait: z.number().nonnegative(),
      /** Changed rows as [row index, runs]. */
      rows: z.array(z.tuple([z.number().int().nonnegative(), z.array(cellRun)])),
      cursor: cursor.optional(),
    }),
  ),
  final: snapshot,
  /** When each answer appears, in recorded milliseconds, for linking the replay to its graph. */
  steps: z.array(
    z.object({ at: z.number().nonnegative(), session: z.string(), title: z.string() }),
  ),
});
export type Replay = z.infer<typeof replay>;
