import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { test } from "node:test";
import { RECALL_SOURCE, heroSvg, recallSvg, placeGraph, readGraph } from "./readme-hero.mjs";

const source = readFileSync(RECALL_SOURCE, "utf8");
const texts = (svg) => [...svg.matchAll(/<text[^>]*>([\s\S]*?)<\/text>/g)].map((m) => m[1].replace(/<[^>]+>/g, ""));

test("the committed assets match the banner and preserved recorded example", async () => {
  const hero = readFileSync(resolve(import.meta.dirname, "../../.github/readme/hero.svg"), "utf8");
  assert.equal(hero, await heroSvg());
  assert.doesNotMatch(hero, /(?:href|src)="https?:/);
  const committed = readFileSync(resolve(import.meta.dirname, "../../.github/readme/recall.svg"), "utf8");
  assert.deepEqual(texts(committed), texts(await recallSvg(placeGraph(readGraph(source)))));
  assert.doesNotMatch(committed, /(?:href|src)="https?:/);
});

test("the recall diagram counts each blend's earlier decisions from the graph", () => {
  const { place, sources, layers, decisions } = placeGraph(readGraph(source));
  assert.deepEqual([sources, layers, decisions], [3, 2, 7]);
  assert.equal(place["recall-response-updated 1"].behind.length, 6);
  assert.equal(place["recall-response-updated 1"].direct, 2);
  assert.equal(place["recall-customers 2"].p, "0.98");
});

test("the recall diagram refuses a graph or an answer it was not drawn for", () => {
  const answer = source.replace("Contact Harbor · 0<br/><b>▶ Contact Summit · 1</b>", "<b>▶ Contact Harbor · 1</b><br/>Contact Summit · 0");
  assert.throws(() => placeGraph(readGraph(answer)), /answered "Contact Harbor"/);
  const edge = source.replace("  t1 --> t2\n", "");
  assert.throws(() => placeGraph(readGraph(edge)), /recall-stock-plan 1 has parents/);
});
