import type { Network } from "../networks";

/** Illustrative compliance network for the team picker. */
export const compliance: Network = {
  label: "Compliance",
  session: "complaint-intake",
  steps: [
    {
      kind: "fact",
      id: "complaint",
      source: "Complaint",
      text: "“You charged my late fee twice and nobody will fix it. I'm filing with the CFPB.”",
    },
    { kind: "fact", id: "account", source: "Account", text: "2 late fees on 3 May · $39 each" },
    {
      kind: "fact",
      id: "contacts",
      source: "Call log",
      text: "2 calls in 10 days · no fix logged",
    },
    {
      kind: "yes/no",
      id: "regulatory",
      prompt: "Is this a regulatory complaint?",
      options: [
        ["Yes", 0.86],
        ["No", 0.14],
      ],
      from: ["complaint"],
    },
    {
      kind: "checks",
      id: "harm",
      prompt: "What harm is claimed?",
      rows: [
        ["Money lost", 0.94],
        ["Unfair treatment", 0.41],
        ["Legal threat", 0.79],
      ],
      from: ["complaint"],
    },
    {
      kind: "yes/no",
      id: "valid",
      prompt: "Does the account back the claim?",
      options: [
        ["Yes", 0.88],
        ["No", 0.12],
      ],
      from: ["harm", "account"],
    },
    {
      kind: "computed",
      id: "deadline",
      name: "Response due",
      value: "Within 15 days",
      from: ["regulatory"],
    },
    {
      kind: "choice",
      id: "severity",
      prompt: "How serious is it?",
      options: [
        ["Routine", 0.09],
        ["Elevated", 0.63],
        ["Critical", 0.28],
      ],
      from: ["regulatory", "valid", "contacts"],
    },
    {
      kind: "computed",
      id: "remedy",
      name: "Remedy",
      value: "Refund $39 · compliance lead signs the letter",
      from: ["severity", "valid"],
    },
  ],
};
