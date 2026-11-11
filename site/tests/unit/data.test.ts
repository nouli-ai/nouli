import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, test } from "vitest";
import { capture } from "../../src/lib/schemas/capture";
import { graph } from "../../src/lib/schemas/graph";
import type { Graph, GraphNode, Question } from "../../src/lib/schemas/graph";
import { replay } from "../../src/lib/schemas/replay";
import { siteRoot } from "../support/site";

const dataDir = join(siteRoot, "src/data");
const files = (dir: string): string[] =>
  readdirSync(join(dataDir, dir))
    .filter((name) => name.endsWith(".json"))
    .map((name) => join(dir, name));
const read = (file: string): unknown => JSON.parse(readFileSync(join(dataDir, file), "utf8"));

// Collections validate graphs and captures at build time; this also covers the replays,
// which the home page loads in the browser.
describe("recorded data matches its schema", () => {
  test.each(files("graphs"))("%s", (file) => {
    expect(graph.safeParse(read(file)).error?.issues).toBeUndefined();
  });
  test.each(files("captures"))("%s", (file) => {
    expect(capture.safeParse(read(file)).error?.issues).toBeUndefined();
  });
  test.each(["replays/wide.json", "replays/narrow.json"])("%s", (file) => {
    expect(replay.safeParse(read(file)).error?.issues).toBeUndefined();
  });
});

const recorded: Graph = graph.parse(read("graphs/home.json"));
const firstQuestion = (node: GraphNode): Question => {
  const [question] = node.questions;
  if (question === undefined) throw new Error("home.json has a node without questions");
  return question;
};
const indexOf = (type: string): number =>
  recorded.nodes.findIndex((node) => firstQuestion(node).type === type);

/** A copy of the home graph with one node changed in place. */
function withNode(index: number, change: (node: GraphNode) => void): unknown {
  const copy = structuredClone(recorded);
  const node = copy.nodes[index];
  if (node === undefined) throw new Error(`home.json has no node ${index}`);
  change(node);
  return copy;
}

const brokenGraphs: [string, unknown][] = [
  [
    "an option it does not offer",
    withNode(indexOf("choice"), (node) => {
      const q = firstQuestion(node);
      if (q.type === "choice" && !("refused" in q)) q.selected = "missing";
    }),
  ],
  [
    "repeated option ids",
    withNode(indexOf("choice"), (node) => {
      const q = firstQuestion(node);
      if (q.type === "choice" && !("refused" in q) && q.options[0] && q.options[1])
        q.options[1].id = q.options[0].id;
    }),
  ],
  [
    "repeated question ids",
    withNode(0, (node) => {
      node.questions.push(structuredClone(firstQuestion(node)));
    }),
  ],
  [
    "an in-range expected level inconsistent with its distribution",
    withNode(indexOf("score"), (node) => {
      const q = firstQuestion(node);
      if (q.type === "score" && !("refused" in q)) q.expected = 0.5;
    }),
  ],
  [
    "a refused question with answer fields",
    withNode(0, (node) => {
      node.questions = [
        {
          id: "q",
          prompt: "Ready?",
          type: "noul",
          refused: "Not enough facts",
          probability: 0.5,
        },
      ];
    }),
  ],
  [
    "a parent that is not earlier",
    withNode(0, (node) => {
      node.parents = [0];
    }),
  ],
  [
    "context from a later turn",
    withNode(0, (node) => {
      node.context = [{ turn: 1, representation: "full" }];
    }),
  ],
  [
    "an expected level beyond the rubric",
    withNode(indexOf("score"), (node) => {
      const q = firstQuestion(node);
      if (q.type === "score" && !("refused" in q)) q.expected = 7;
    }),
  ],
  [
    "probabilities that do not sum to 1",
    withNode(indexOf("choice"), (node) => {
      const q = firstQuestion(node);
      if (q.type === "choice" && !("refused" in q)) for (const option of q.options) option.p = 0.9;
    }),
  ],
  ["an unknown field", { ...recorded, exported: "today" }],
];

test.each(brokenGraphs)("the graph schema rejects %s", (_, data) => {
  expect(graph.safeParse(data).success).toBe(false);
});

test("the graph schema accepts a refused question", () => {
  const data = withNode(0, (node) => {
    node.questions = [{ id: "q", prompt: "Ready?", type: "noul", refused: "Not enough facts" }];
  });
  expect(graph.safeParse(data).success).toBe(true);
});
