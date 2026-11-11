import type { Network } from "../networks";

/** Illustrative support network for the team picker. */
export const support: Network = {
  label: "Support",
  session: "ticket-triage",
  steps: [
    {
      kind: "fact",
      id: "ticket",
      source: "Ticket",
      text: "“Exports have failed all morning. Third time this month. We renew soon and I'm out of patience.”",
    },
    { kind: "fact", id: "status", source: "Status page", text: "Exports degraded since 9:40 am" },
    { kind: "fact", id: "account", source: "CRM", text: "Enterprise · renews in 21 days" },
    {
      kind: "choice",
      id: "issue",
      prompt: "What kind of issue is it?",
      options: [
        ["Outage", 0.71],
        ["Bug", 0.22],
        ["How-to", 0.04],
        ["Billing", 0.03],
      ],
      from: ["ticket"],
    },
    {
      kind: "choice",
      id: "mood",
      prompt: "How is the customer feeling?",
      options: [
        ["Calm", 0.04],
        ["Frustrated", 0.38],
        ["Angry", 0.58],
      ],
      from: ["ticket"],
    },
    {
      kind: "yes/no",
      id: "incident",
      prompt: "Is it the open incident?",
      options: [
        ["Yes", 0.89],
        ["No", 0.11],
      ],
      from: ["issue", "status"],
    },
    {
      kind: "choice",
      id: "churn",
      prompt: "How likely are they to leave?",
      options: [
        ["Low", 0.12],
        ["Medium", 0.31],
        ["High", 0.57],
      ],
      from: ["mood", "account", "ticket"],
    },
    {
      kind: "computed",
      id: "route",
      name: "Route",
      value: "Link to incident · notify on fix",
      from: ["incident"],
    },
    {
      kind: "choice",
      id: "reply",
      prompt: "Who should reply?",
      options: [
        ["Agent", 0.18],
        ["Senior agent", 0.27],
        ["Account manager", 0.55],
      ],
      from: ["churn", "incident"],
    },
    {
      kind: "computed",
      id: "sla",
      name: "Reply due",
      value: "Within 1 hour · account manager",
      from: ["reply", "account"],
    },
  ],
};
