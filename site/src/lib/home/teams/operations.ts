import type { Network } from "../networks";

/** Illustrative operations network for the team picker. */
export const operations: Network = {
  label: "Operations",
  session: "supplier-delay",
  steps: [
    {
      kind: "fact",
      id: "email",
      source: "Supplier email",
      text: "“Resin shortage at our plant. We hope to ship most of PO-7781 in about two weeks.”",
    },
    {
      kind: "fact",
      id: "history",
      source: "Supplier record",
      text: "Last 3 delay estimates slipped",
    },
    { kind: "fact", id: "stock", source: "Inventory", text: "1,450 units · 160 used a day" },
    {
      kind: "fact",
      id: "orders",
      source: "Sales orders",
      text: "Two key accounts ship in 12 days",
    },
    {
      kind: "choice",
      id: "delay",
      prompt: "How long is the delay really?",
      options: [
        ["Under 1 week", 0.04],
        ["1–3 weeks", 0.38],
        ["Over 3 weeks", 0.46],
        ["Unclear", 0.12],
      ],
      from: ["email", "history"],
    },
    {
      kind: "computed",
      id: "cover",
      name: "Stock cover",
      value: "9 days",
      from: ["stock"],
    },
    {
      kind: "yes/no",
      id: "partial",
      prompt: "Is a partial shipment likely?",
      options: [
        ["Yes", 0.67],
        ["No", 0.33],
      ],
      from: ["email", "history"],
    },
    {
      kind: "choice",
      id: "plan",
      prompt: "What should we do?",
      options: [
        ["Wait", 0.08],
        ["Expedite", 0.21],
        ["Second supplier", 0.71],
      ],
      from: ["delay", "partial", "cover"],
    },
    {
      kind: "yes/no",
      id: "warn",
      prompt: "Should we warn customers?",
      options: [
        ["Yes", 0.62],
        ["No", 0.38],
      ],
      from: ["plan", "orders"],
    },
    {
      kind: "computed",
      id: "tasks",
      name: "Tasks",
      value: "Quote from backup supplier · tell 2 accounts",
      from: ["plan", "warn"],
    },
  ],
};
