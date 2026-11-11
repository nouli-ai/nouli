import { z } from "astro/zod";
import { isRefused } from "../graph/question";

const probability = z.number().min(0).max(1);
/** Recorded distributions are rounded; allow this much rounding per entry. */
const rounding = 0.005;
const asked = { id: z.string(), prompt: z.string() };

/** Choose one of the supplied options; `selected` names the winning option. */
const choice = z.strictObject({
  ...asked,
  type: z.literal("choice"),
  options: z
    .array(z.strictObject({ id: z.string(), label: z.string(), p: probability }))
    .min(2)
    .max(255),
  selected: z.string(),
});

/** Yes or no; `probability` is P(yes). */
const noul = z.strictObject({ ...asked, type: z.literal("noul"), probability });

/** An ordered rubric; `expected` is the probability-weighted level index and can be fractional. */
const score = z.strictObject({
  ...asked,
  type: z.literal("score"),
  levels: z
    .array(z.strictObject({ label: z.string(), p: probability }))
    .min(2)
    .max(10),
  expected: z.number().nonnegative(),
});

/** A question the model refused to answer, with its reason. */
const refused = z.strictObject({
  ...asked,
  type: z.enum(["choice", "noul", "score"]),
  refused: z.string(),
});

export const answeredQuestion = z.discriminatedUnion("type", [choice, noul, score]);
export type AnsweredQuestion = z.infer<typeof answeredQuestion>;
export const question = z.union([answeredQuestion, refused]);
export type Question = z.infer<typeof question>;
export type RefusedQuestion = z.infer<typeof refused>;

const near = (value: number, target: number, entries: number): boolean =>
  Math.abs(value - target) <= rounding * entries;

/** Problems with one answered question's distribution, which the readouts rely on. */
function distributionProblems(q: AnsweredQuestion): string[] {
  if (q.type === "noul") return [];
  const ps = q.type === "choice" ? q.options.map(({ p }) => p) : q.levels.map(({ p }) => p);
  const problems: string[] = [];
  if (
    !near(
      ps.reduce((sum, p) => sum + p, 0),
      1,
      ps.length,
    )
  )
    problems.push("probabilities do not sum to 1");
  if (q.type === "choice") {
    if (new Set(q.options.map(({ id }) => id)).size !== q.options.length)
      problems.push("option ids repeat");
    if (!q.options.some(({ id }) => id === q.selected))
      problems.push("selects an option it does not offer");
  } else {
    const mean = q.levels.reduce((sum, { p }, level) => sum + p * level, 0);
    if (q.expected > q.levels.length - 1 || !near(q.expected, mean, 1)) {
      problems.push("expected level does not match its distribution");
    }
  }
  return problems;
}

/** One recorded turn: the new facts, the questions answered, and what the model saw. */
export const graphNode = z.strictObject({
  id: z.string(),
  session: z.string(),
  turn: z.number().int().positive(),
  kind: z.enum(["new", "continued", "branch", "blend"]),
  /** Indices of earlier nodes in the same graph. */
  parents: z.array(z.number().int().nonnegative()),
  state: z.string(),
  questions: z.array(question).min(1),
  /** Earlier turns, by node index, included in the model's context. */
  context: z.array(
    z.strictObject({ turn: z.number().int().nonnegative(), representation: z.string() }),
  ),
  model: z.string(),
  command: z.string(),
});
export type GraphNode = z.infer<typeof graphNode>;

/** A session graph exported from a Nouli store. Parents and context refer to earlier nodes. */
export const graph = z
  .strictObject({ session: z.string(), nodes: z.array(graphNode).min(1) })
  .superRefine(({ nodes }, context) => {
    const report = (index: number, message: string): void => {
      context.addIssue({
        code: "custom",
        path: ["nodes", index],
        message: `node ${index}: ${message}`,
      });
    };
    nodes.forEach((node, index) => {
      if (node.parents.some((parent) => parent >= index))
        report(index, "a parent is not an earlier node");
      if (node.context.some(({ turn }) => turn >= index))
        report(index, "context includes a turn that is not earlier");
      if (new Set(node.questions.map(({ id }) => id)).size !== node.questions.length)
        report(index, "question ids repeat");
      for (const q of node.questions) {
        if (isRefused(q)) continue;
        for (const problem of distributionProblems(q)) report(index, `question ${q.id} ${problem}`);
      }
    });
  });
export type Graph = z.infer<typeof graph>;
