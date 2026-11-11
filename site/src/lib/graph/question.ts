import type { Question, RefusedQuestion } from "../schemas/graph";

/** Whether the model refused the question instead of answering it. */
export const isRefused = (question: Question): question is RefusedQuestion => "refused" in question;
