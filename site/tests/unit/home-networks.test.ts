import { expect, test } from "vitest";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { example } from "../../src/lib/schemas/example";
import { networkOf } from "../../src/lib/examples";
import { homeNetworks } from "../../src/lib/home/teams";
import { sampleRange } from "../../src/lib/examples";
import { answerOf, shownOptions, summaryOf } from "../../src/lib/home/networks";
import type { Question } from "../../src/lib/home/networks";
import { depthsOf, edgePath, estimateHeight, layoutNetwork } from "../../src/lib/home/layout";
import { siteRoot } from "../support/site";
const dir = join(siteRoot, "src/content/examples");
const entries = readdirSync(dir).map((name) => ({
  id: name.slice(0, -5),
  data: example.parse(JSON.parse(readFileSync(join(dir, name), "utf8"))),
}));
test.each(entries)(
  "$id has valid ancestry, complete distributions, and layouts within bounds",
  (entry) => {
    const net = networkOf(entry);
    const layout = layoutNetwork(net, { pad: 0.12, fit: true });
    expect(layout.placed).toHaveLength(net.steps.length);
    for (const point of layout.placed) {
      expect(point.wide.x).toBeGreaterThan(0);
      expect(point.wide.x).toBeLessThan(1);
      expect(point.narrow.y * layout.height.narrow).toBeGreaterThanOrEqual(
        estimateHeight(point.step) / 2,
      );
      expect(point.narrow.y * layout.height.narrow + estimateHeight(point.step) / 2).toBeLessThan(
        layout.height.narrow,
      );
      if ("options" in point.step) {
        expect(shownOptions(point.step)).toEqual(point.step.options);
        expect(point.step.options.reduce((sum, [, p]) => sum + p, 0)).toBeCloseTo(1, 1);
      }
    }
    expect(layout.edges.every((edge) => net.steps.some((s) => s.id === edge.from))).toBe(true);
  },
);
test("depths reject cycles and missing sources", () => {
  expect(() =>
    depthsOf("bad", [{ kind: "computed", id: "a", name: "A", value: "", from: ["b"] }]),
  ).toThrow("unknown b");
  expect(() =>
    depthsOf("bad", [{ kind: "computed", id: "a", name: "A", value: "", from: ["a"] }]),
  ).toThrow("cycle");
});
test("answers keep the most likely option, fractional scores, and all options", () => {
  const q: Question = {
    id: "q",
    kind: "choice",
    prompt: "Pick?",
    from: ["f"],
    options: [
      ["A", 0.1],
      ["B", 0.7],
      ["C", 0.1],
      ["D", 0.05],
      ["E", 0.05],
    ],
  };
  expect(shownOptions(q)).toHaveLength(5);
  expect(answerOf(q)).toEqual({ label: "B", probability: 0.7 });
  expect(summaryOf(q)).toBe("Pick? B (0.70)");
  expect(
    answerOf({
      ...q,
      kind: "score",
      options: [
        ["0", 0.25],
        ["1", 0.75],
      ],
    }).label,
  ).toBe("0.8 of 1");
  expect(edgePath({ x: 0.1, y: 0.2 }, { x: 0.8, y: 0.9 }, "wide")).toBe(
    "M100.0 200.0C450.0 200.0 450.0 900.0 800.0 900.0",
  );
});
test("the home networks are invented, distinct and valid", () => {
  const home = homeNetworks();
  const others = [home.hero, home.build, home.trace, ...home.teams];
  expect(new Set(others.map((n) => n.session)).size).toBe(others.length);
  for (const n of others) {
    expect(entries.map((e) => e.id)).not.toContain(n.session);
    layoutNetwork(n, { pad: 0.12, withBranch: true });
    for (const step of [...n.steps, ...(n.branch ?? [])])
      if ("options" in step)
        expect(Math.abs(step.options.reduce((sum, [, p]) => sum + p, 0) - 1)).toBeLessThan(0.02);
  }
});

test("example schema rejects ambiguous IDs, missing sources, cycles and invalid distributions", () => {
  const data = entries[0]?.data;
  if (!data) throw new Error("Missing example");
  expect(example.safeParse({ ...data, steps: [data.steps[0], data.steps[0]] }).success).toBe(false);
  expect(
    example.safeParse({
      ...data,
      steps: [
        data.steps[0],
        { kind: "computed", id: "bad", name: "Bad", value: "", from: ["missing"] },
      ],
    }).success,
  ).toBe(false);
  expect(
    example.safeParse({
      ...data,
      steps: [
        data.steps[0],
        { kind: "computed", id: "bad", name: "Bad", value: "", from: ["bad"] },
      ],
    }).success,
  ).toBe(false);
  expect(
    example.safeParse({
      ...data,
      steps: [
        data.steps[0],
        {
          kind: "choice",
          id: "bad",
          prompt: "?",
          from: [data.steps[0]?.id],
          options: [
            ["A", 0.8],
            ["B", 0.8],
          ],
        },
      ],
    }).success,
  ).toBe(false);
});

test("the examples index states the case range from the collection", () => {
  expect(sampleRange(entries)).toEqual({ min: 100, max: 300 });
});
