import { readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "vitest";
import { startStub } from "../../tools/recorded-provider.mjs";
import { isRefused } from "../../src/lib/graph/question";
import { graph } from "../../src/lib/schemas/graph";
import { siteRoot } from "../support/site";

test("the replay provider preserves saved probabilities when the form assigns new IDs", async () => {
  const data = graph.parse(
    JSON.parse(readFileSync(join(siteRoot, "src/data/graphs/home.json"), "utf8")),
  );
  const question = data.nodes.flatMap((node) => node.questions).find((q) => q.type === "choice");
  if (!question || isRefused(question) || question.type !== "choice")
    throw new Error("The recording needs a choice question");
  const calls = [{ node: { questions: [question] }, used: false }];
  const stub = await startStub(calls, { tui: true });
  try {
    const criteria = Object.fromEntries(question.options.map((o, i) => [`option${i}`, o.label]));
    const response = await fetch(`http://127.0.0.1:${stub.port}/v1/systemone`, {
      method: "POST",
      body: JSON.stringify({
        questions: { formId: { type: "choice", instructions: question.prompt, criteria } },
      }),
    });
    expect(response.ok).toBe(true);
    expect(await response.json()).toMatchObject({
      answers: {
        formId: {
          probabilities: Object.fromEntries(question.options.map((o, i) => [`option${i}`, o.p])),
          choice: `option${question.options.findIndex((o) => o.id === question.selected)}`,
        },
      },
    });
    expect(calls[0]?.used).toBe(true);
    expect(stub.failures).toEqual([]);
    const missing = await fetch(`http://127.0.0.1:${stub.port}/v1/systemone`, {
      method: "POST",
      body: JSON.stringify({
        questions: { unknown: { type: "noul", instructions: "Unrecorded" } },
      }),
    });
    expect(missing.status).toBe(500);
    expect(stub.failures).toHaveLength(1);
  } finally {
    await new Promise<void>((resolve, reject) => {
      stub.server.close((error) => {
        if (error) reject(error);
        else resolve();
      });
    });
  }
});
