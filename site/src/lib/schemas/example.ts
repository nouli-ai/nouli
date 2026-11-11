import { depthsOf } from "../home/layout";
import { z } from "astro/zod";

const id = z.string().regex(/^[a-z][a-z0-9-]*$/u);
const probability = z.number().min(0).max(1);
const from = z.array(id).min(1);

/** One step of a recorded case, in the shapes the site draws. */
const step = z.discriminatedUnion("kind", [
  z.strictObject({ kind: z.literal("fact"), id, source: z.string(), text: z.string() }),
  z.strictObject({
    kind: z.enum(["choice", "yes/no"]),
    id,
    prompt: z.string(),
    /** Every option the question offered, with its recorded probability. */
    options: z.array(z.tuple([z.string(), probability])).min(2),
    from,
  }),
  z.strictObject({
    kind: z.literal("checks"),
    id,
    prompt: z.string(),
    /** Each yes/no check with its recorded probability of yes. */
    rows: z.array(z.tuple([z.string(), probability])).min(2),
    from,
  }),
  z.strictObject({
    kind: z.enum(["rule", "computed"]),
    id,
    name: z.string(),
    value: z.string(),
    from,
  }),
]);

/**
 * An example: one real case from a public dataset, run through a decision network and recorded
 * by the evaluation that measured the network on the dataset's other cases.
 */
export const example = z
  .strictObject({
    /** Position on the examples index, starting at 1. */
    order: z.number().int().positive(),
    industry: z.string(),
    /** The dataset: its name, a short name for chips, its home page, and its license. */
    source: z.strictObject({
      name: z.string(),
      short: z.string(),
      url: z.url(),
      license: z.string(),
      attribution: z.string().optional(),
    }),
    /** The index card: a short title and the job in one line. */
    title: z.string(),
    job: z.string(),
    /** The example page: its headline and one line on how the network works. */
    heading: z.string(),
    lede: z.string(),
    /** The routine work the network takes over, in a sentence or two. */
    context: z.string(),
    /** How many real cases the network ran on, for "Run on 300 real reports". */
    sample: z.strictObject({ count: z.number().int().positive(), unit: z.string() }),
    /** Measured on the sample: model decisions per case, median seconds per case, and dollars per
     * 1,000 cases at the model's list price. */
    stats: z.strictObject({
      decisions: z.number().positive(),
      medianSeconds: z.number().positive(),
      costPer1000: z.number().positive(),
    }),
    /** Where the network sent every case in the sample. */
    routes: z.strictObject({
      label: z.string(),
      counts: z.array(z.tuple([z.string(), z.number().int().nonnegative()])).min(2),
    }),
    /** The case shown: its name and its record at the source. */
    case: z.strictObject({ label: z.string(), url: z.url() }),
    model: z.string(),
    /** Small print, in order, each a complete sentence or two. */
    notes: z.array(z.string()).min(1),
    steps: z.array(step).min(2),
  })
  .superRefine(({ steps, routes, sample }, context) => {
    if (routes.counts.reduce((sum, [, n]) => sum + n, 0) !== sample.count)
      context.addIssue({
        code: "custom",
        path: ["routes"],
        message: "Routes must cover the sample",
      });
    const report = (message: string): void => {
      context.addIssue({ code: "custom", path: ["steps"], message });
    };
    if (new Set(steps.map((s) => s.id)).size !== steps.length) report("Step IDs must be unique");
    try {
      depthsOf("example", steps);
    } catch (error) {
      report(error instanceof Error ? error.message : "Invalid ancestry");
    }
    for (const s of steps) {
      if ("options" in s) {
        if (Math.abs(s.options.reduce((sum, [, p]) => sum + p, 0) - 1) > 0.025)
          report("Invalid probability total in " + s.id);
        if (new Set(s.options.map(([label]) => label)).size !== s.options.length)
          report("Repeated option in " + s.id);
      }
    }
  });
export type Example = z.infer<typeof example>;
