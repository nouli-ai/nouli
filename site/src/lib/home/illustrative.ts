import type { Network } from "./networks";

/**
 * Invented networks for the home page's demonstrations, labeled illustrative where they appear.
 * Each one makes judgment calls a fixed rule can't (intent, tone, credibility, fit) from more than
 * one source, and leaves exact work such as limits, dates and lookups to code.
 */

/** Support: a refund request, with a what-if on the customer's order history. */
export const refundReview: Network = {
  label: "Support",
  session: "refund-review",
  steps: [
    {
      kind: "fact",
      id: "ticket",
      source: "Support email",
      text: "“Box came crushed and the lamp base is cracked. Honestly I just want my money back.”",
    },
    {
      kind: "fact",
      id: "carrier",
      source: "Carrier",
      text: "Delivered Tue 2:14 pm · left at door",
    },
    { kind: "fact", id: "orders", source: "Order history", text: "$84 lamp · 1 return in 2 years" },
    {
      kind: "choice",
      id: "intent",
      prompt: "What does the customer want?",
      options: [
        ["Refund", 0.81],
        ["Replacement", 0.12],
        ["Store credit", 0.05],
        ["Just venting", 0.02],
      ],
      from: ["ticket"],
    },
    {
      kind: "choice",
      id: "arrival",
      prompt: "How did the order arrive?",
      options: [
        ["Damaged", 0.84],
        ["Never arrived", 0.03],
        ["Fine", 0.13],
      ],
      from: ["ticket", "carrier"],
    },
    {
      kind: "yes/no",
      id: "abuse",
      prompt: "Does this look like refund abuse?",
      options: [
        ["No", 0.93],
        ["Yes", 0.07],
      ],
      from: ["ticket", "orders"],
    },
    {
      kind: "choice",
      id: "resolve",
      prompt: "How should we resolve it?",
      options: [
        ["Refund now", 0.78],
        ["Send replacement", 0.14],
        ["Escalate", 0.08],
      ],
      from: ["intent", "arrival", "abuse"],
    },
    {
      kind: "computed",
      id: "action",
      name: "Action",
      value: "Refund $84 to card · reply drafted",
      from: ["resolve", "orders"],
    },
  ],
  branch: [
    {
      kind: "fact",
      id: "orders-what-if",
      source: "What if",
      text: "4 damage claims in 90 days",
    },
    {
      kind: "yes/no",
      id: "abuse-what-if",
      prompt: "Does this look like refund abuse?",
      options: [
        ["No", 0.31],
        ["Yes", 0.69],
      ],
      from: ["ticket", "orders-what-if"],
    },
    {
      kind: "choice",
      id: "resolve-what-if",
      prompt: "How should we resolve it?",
      options: [
        ["Refund now", 0.12],
        ["Send replacement", 0.2],
        ["Escalate", 0.68],
      ],
      from: ["intent", "arrival", "abuse-what-if"],
    },
    {
      kind: "computed",
      id: "action-what-if",
      name: "Action",
      value: "Hold refund · send to fraud review",
      from: ["resolve-what-if"],
    },
  ],
};

/** Finance: a vendor disputes a short payment; the vendor's tone skips straight to the action. */
export const invoiceDispute: Network = {
  label: "Finance",
  session: "invoice-dispute",
  steps: [
    {
      kind: "fact",
      id: "email",
      source: "Vendor email",
      text: "“INV-2291 was short-paid by $3,000. We never agreed to a 6% discount. Please fix this week.”",
    },
    {
      kind: "fact",
      id: "contract",
      source: "Contract",
      text: "2% discount if paid within 10 days",
    },
    { kind: "fact", id: "payments", source: "Payments", text: "Paid $47,000 of $50,000 on day 14" },
    {
      kind: "choice",
      id: "claim",
      prompt: "What is the vendor disputing?",
      options: [
        ["Short payment", 0.88],
        ["Late fee", 0.05],
        ["Invoice amount", 0.04],
        ["Something else", 0.03],
      ],
      from: ["email"],
    },
    {
      kind: "choice",
      id: "terms",
      prompt: "Do the terms allow our deduction?",
      options: [
        ["No", 0.74],
        ["Partly", 0.2],
        ["Yes", 0.06],
      ],
      from: ["claim", "contract"],
    },
    {
      kind: "computed",
      id: "window",
      name: "Discount window",
      value: "Paid day 14 · window closed day 10",
      from: ["contract", "payments"],
    },
    {
      kind: "choice",
      id: "tone",
      prompt: "How firm is the vendor?",
      options: [
        ["Routine", 0.21],
        ["Firm", 0.66],
        ["May stop supply", 0.13],
      ],
      from: ["email"],
    },
    {
      kind: "choice",
      id: "verdict",
      prompt: "Who is right?",
      options: [
        ["Vendor", 0.82],
        ["Us", 0.07],
        ["Unclear", 0.11],
      ],
      from: ["claim", "terms", "window"],
    },
    {
      kind: "computed",
      id: "next",
      name: "Next step",
      value: "Pay $3,000 balance this week · AP lead approves",
      from: ["verdict", "tone", "payments"],
    },
  ],
};

/** Hero: a merchant decides whether to fight a chargeback. */
export const chargeback: Network = {
  label: "Payments",
  session: "chargeback-review",
  steps: [
    {
      kind: "fact",
      id: "dispute",
      source: "Chargeback",
      text: "“I never received this order.”",
    },
    { kind: "fact", id: "carrier", source: "Carrier", text: "Delivered · signed by J. Ortiz" },
    {
      kind: "choice",
      id: "proof",
      prompt: "Does the delivery proof hold up?",
      options: [
        ["Strong", 0.81],
        ["Weak", 0.15],
        ["None", 0.04],
      ],
      from: ["dispute", "carrier"],
    },
    {
      kind: "yes/no",
      id: "fraud",
      prompt: "Is this friendly fraud?",
      options: [
        ["Yes", 0.68],
        ["No", 0.32],
      ],
      from: ["dispute", "carrier"],
    },
    {
      kind: "choice",
      id: "fight",
      prompt: "Fight or accept?",
      options: [
        ["Fight", 0.77],
        ["Accept", 0.23],
      ],
      from: ["proof", "fraud"],
    },
    {
      kind: "computed",
      id: "response",
      name: "Response",
      value: "Evidence pack sent to the bank",
      from: ["fight", "carrier"],
    },
  ],
};
