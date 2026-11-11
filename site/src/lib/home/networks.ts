/**
 * Decision networks as the site draws them: the home page's illustrative networks and the
 * examples' recorded cases share these types.
 */

/** A source the network reads: an email, a report, a record. */
export interface Fact {
  readonly id: string;
  readonly kind: "fact";
  readonly source: string;
  readonly text: string;
}

/** A question with a fixed set of answers and a probability for each. */
export interface Question {
  readonly id: string;
  readonly kind: "choice" | "yes/no" | "score";
  readonly prompt: string;
  /** Options in display order with their probabilities; for a score, the ordered levels. */
  readonly options: readonly (readonly [label: string, probability: number])[];
  readonly from: readonly string[];
}

/** Related yes/no questions drawn together, each with its probability of yes. */
export interface Checks {
  readonly id: string;
  readonly kind: "checks";
  readonly prompt: string;
  readonly rows: readonly (readonly [label: string, yes: number])[];
  readonly from: readonly string[];
}

/** A step run as code rather than by a model: a frozen rule or an exact calculation. */
export interface Code {
  readonly id: string;
  readonly kind: "rule" | "computed";
  readonly name: string;
  readonly value: string;
  readonly from: readonly string[];
}

export type Step = Fact | Question | Checks | Code;

/** Whether a step is a model question with options. */
export const isQuestion = (step: Step): step is Question =>
  step.kind === "choice" || step.kind === "yes/no" || step.kind === "score";

/** Whether a step runs as code: a frozen rule or an exact calculation. */
export const isCode = (step: Step): step is Code =>
  step.kind === "rule" || step.kind === "computed";

/** The steps a step reads. */
export const sourcesOf = (step: Step): readonly string[] => (step.kind === "fact" ? [] : step.from);

/** Every allowed option and its recorded probability. */
export const shownOptions = (question: Question): Question["options"] => question.options;

export interface Network {
  /** The tab label and the panel caption. */
  readonly label: string;
  readonly session: string;
  readonly steps: readonly Step[];
  /** What-if steps shown only when the network demonstrates branching. */
  readonly branch?: readonly Step[];
}

/** The answer a question settles on: the most likely option, or a score's expected level. */
export function answerOf(question: Question): { label: string; probability?: number } {
  if (question.kind === "score") {
    const expected = question.options.reduce((sum, [level, p]) => sum + Number(level) * p, 0);
    const top = question.options.at(-1)?.[0] ?? "";
    return { label: `${expected.toFixed(1)} of ${top}` };
  }
  const [label, probability] = question.options.reduce((best, option) =>
    option[1] > best[1] ? option : best,
  );
  return { label, probability };
}

/** A one-line summary of a step, as later steps see it. */
export function summaryOf(step: Step): string {
  if (step.kind === "fact") return `${step.source}: ${step.text}`;
  if (isCode(step)) return `${step.name}: ${step.value}`;
  if (step.kind === "checks") {
    const yes = step.rows.filter(([, p]) => p >= 0.5).map(([label]) => label);
    return `${step.prompt} ${yes.length === 0 ? "none" : yes.join(", ")}`;
  }
  const { label, probability } = answerOf(step);
  return `${step.prompt} ${label}${probability === undefined ? "" : ` (${probability.toFixed(2)})`}`;
}
