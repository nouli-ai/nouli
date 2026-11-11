import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { expect, test } from "vitest";
import { countWord, releaseDate, releaseDetails, sectionLookup } from "../../src/lib/models";
import { model } from "../../src/lib/schemas/model";
import { modelSourceCode } from "../../src/lib/seo/structured-data";
import { siteRoot } from "../support/site";

const directory = join(siteRoot, "src/content/models");
const models = readdirSync(directory).map((file) =>
  model.parse(JSON.parse(readFileSync(join(directory, file), "utf8"))),
);
const sample = models[0];
if (sample === undefined) throw new Error("No model fixtures");

test("catalog positions are unique and consecutive", () => {
  expect(models.map(({ order }) => order).toSorted((a, b) => a - b)).toEqual(
    models.map((_, i) => i + 1),
  );
});
test("model names, titles, and fallback summaries derive from facts", () => {
  for (const data of models) {
    expect(data.fullName).toBe(`Nouli ${data.name}`);
    expect(data.title).toBe(`${data.fullName}: ${data.task}`);
    expect([data.title, data.fullName]).toContain(data.openGraph.title);
    if (data.release === "versioned")
      expect(releaseDetails("sample", data).files.endsWith(`/tree/v${data.version}`)).toBe(true);
  }
});
test("dates, counts and section lookup retain the published copy", () => {
  expect(releaseDate("2026-11-02")).toBe("2 November 2026");
  expect(countWord(9)).toBe("Nine");
  expect(() => countWord(13)).toThrow("No word");
  const sections = [{ id: "use", title: "Use the model" }];
  expect(sectionLookup(sections)("use")).toEqual(sections[0]);
  expect(() => sectionLookup(sections)("missing")).toThrow('No section "missing"');
});
test("structured data emits a single license as a string and several as an array", () => {
  const license = "https://example.com/license";
  const data = { ...sample, structuredData: { ...sample.structuredData, licenses: [license] } };
  expect(modelSourceCode("sample", data).license).toBe(license);
  data.structuredData.licenses.push("https://example.com/upstream");
  expect(modelSourceCode("sample", data).license).toEqual(data.structuredData.licenses);
});
