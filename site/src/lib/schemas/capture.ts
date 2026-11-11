import { z } from "astro/zod";
import { screen } from "./terminal";

/** The terminal screens captured for one walkthrough step, in each view and width. */
export const captureFrame = z.object({
  step: z.number().int().nonnegative(),
  title: z.string(),
  kind: z.enum(["choice", "noul", "score"]),
  session: z.string(),
  draft: screen,
  draftNarrow: screen,
  answer: screen,
  narrow: screen,
  graph: screen,
});
export type CaptureFrame = z.infer<typeof captureFrame>;

/** Captures of the terminal app for a walkthrough, replaying its recorded answers. */
export const capture = z.object({
  version: z.literal(1),
  application: z.string(),
  replayed: z.boolean(),
  frames: z.array(captureFrame).min(1),
});
export type Capture = z.infer<typeof capture>;
