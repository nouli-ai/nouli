import { chargeback, invoiceDispute, refundReview } from "./illustrative";
import type { Network } from "./networks";
import { teamNetworks } from "./team-networks";

export const operations = [
  { mode: "chain", name: "Chain", text: "Each step sees every answer before it." },
  { mode: "branch", name: "Branch", text: "Test a what-if without touching the original." },
  { mode: "blend", name: "Blend", text: "Merge two histories into one decision." },
  { mode: "run", name: "Run", text: "Save it as a workflow and run steps in parallel." },
] as const;

/** Held-out task results; the total comes from the models collection. */
export const modelsAheadOfJev = 5;

/** The home page's networks are all invented and labeled illustrative; the examples hold real data. */
export function homeNetworks(): {
  hero: Network;
  build: Network;
  trace: Network;
  close: Network;
  teams: readonly Network[];
  first: Network;
} {
  const [first] = teamNetworks;
  if (!first) throw new Error("The home page needs a team network");
  return {
    hero: chargeback,
    build: refundReview,
    trace: invoiceDispute,
    close: refundReview,
    teams: teamNetworks,
    first,
  };
}
