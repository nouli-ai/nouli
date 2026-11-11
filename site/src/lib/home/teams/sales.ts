import type { Network } from "../networks";

/** Illustrative sales network for the team picker. */
export const sales: Network = {
  label: "Sales",
  session: "lead-routing",
  steps: [
    {
      kind: "fact",
      id: "form",
      source: "Demo request",
      text: "“We're replacing our current tool before Q3 and have budget approved. Can we talk this week?”",
    },
    { kind: "fact", id: "visits", source: "Web visits", text: "Pricing ×3 · security page ×2" },
    { kind: "fact", id: "company", source: "Enrichment", text: "Logistics · 1,200 staff · Ohio" },
    {
      kind: "checks",
      id: "signals",
      prompt: "Buying signals",
      rows: [
        ["Budget approved", 0.91],
        ["Firm timeline", 0.77],
        ["Replacing a rival", 0.83],
      ],
      from: ["form"],
    },
    {
      kind: "choice",
      id: "fit",
      prompt: "How well do they fit?",
      options: [
        ["Strong", 0.72],
        ["Partial", 0.24],
        ["Poor", 0.04],
      ],
      from: ["company", "form"],
    },
    {
      kind: "choice",
      id: "intent",
      prompt: "Where are they in buying?",
      options: [
        ["Ready to buy", 0.64],
        ["Comparing", 0.29],
        ["Researching", 0.07],
      ],
      from: ["signals", "visits"],
    },
    {
      kind: "choice",
      id: "angle",
      prompt: "What should the first call lead with?",
      options: [
        ["Migration plan", 0.58],
        ["Security review", 0.27],
        ["Pricing", 0.15],
      ],
      from: ["intent", "visits"],
    },
    {
      kind: "computed",
      id: "score",
      name: "Lead score",
      value: "86 of 100",
      from: ["intent", "fit"],
    },
    {
      kind: "computed",
      id: "owner",
      name: "Owner",
      value: "Enterprise AE, Central · call within 1 hour",
      from: ["score", "angle", "company"],
    },
  ],
};
