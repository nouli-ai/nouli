import type { Network } from "../networks";

/** Illustrative finance network for the team picker. */
export const finance: Network = {
  label: "Finance",
  session: "expense-review",
  steps: [
    { kind: "fact", id: "receipt", source: "Receipt", text: "Harbor Grill · $412 · 6 covers" },
    { kind: "fact", id: "calendar", source: "Calendar", text: "Dinner with Acme buying team" },
    { kind: "fact", id: "note", source: "Employee note", text: "“Closing dinner, Acme renewal”" },
    { kind: "fact", id: "policy", source: "Policy", text: "Client meals up to $90 a head" },
    {
      kind: "choice",
      id: "purpose",
      prompt: "Is the business purpose believable?",
      options: [
        ["Clearly", 0.83],
        ["Possibly", 0.14],
        ["Unlikely", 0.03],
      ],
      from: ["receipt", "calendar", "note"],
    },
    {
      kind: "rule",
      id: "limit",
      name: "Per-head limit",
      value: "$69 a head · within $90",
      from: ["receipt", "policy"],
    },
    {
      kind: "checks",
      id: "flags",
      prompt: "Anything off?",
      rows: [
        ["Personal items", 0.06],
        ["Duplicate claim", 0.03],
        ["Guests don't match", 0.18],
      ],
      from: ["receipt", "purpose"],
    },
    {
      kind: "choice",
      id: "approve",
      prompt: "Should it go through?",
      options: [
        ["Approve", 0.86],
        ["Ask a question", 0.11],
        ["Reject", 0.03],
      ],
      from: ["purpose", "flags", "limit"],
    },
    {
      kind: "computed",
      id: "decision",
      name: "Decision",
      value: "Approve · reimburse Friday",
      from: ["approve"],
    },
  ],
};
