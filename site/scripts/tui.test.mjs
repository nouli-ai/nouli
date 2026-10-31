import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { resolve, join } from "node:path";
import { readPage, startStub } from "./replay.mjs";
const site = resolve(import.meta.dirname, "..");
const read = (path) => readFileSync(join(site, path), "utf8");
const plain = (screen) =>
  screen.rows.map((row) => row.map((run) => run[0]).join("")).join("\n");

test("every saved walkthrough step has a current TUI capture tied to its recorded question", () => {
  const examples = readdirSync(join(site, "examples"), { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name);
  assert.ok(examples.includes("meeting"));
  let count = 0;
  for (const name of examples) {
    const html = read(`examples/${name}/index.html`),
      page = readPage(html);
    assert.ok(
      !html.includes('class="repl"') && !html.includes('data-view="repl"'),
      name,
    );
    const saved = JSON.parse(read(`src/data/examples/${page.graph}.json`));
    const captures = JSON.parse(read(`src/data/tui/${name}.json`));
    assert.equal(captures.replayed, true);
    const expected = page.steps
      .map((step, index) => (step.node === null ? null : index))
      .filter((index) => index !== null);
    assert.deepEqual(
      captures.frames.map((frame) => frame.step),
      expected,
      name,
    );
    for (const frame of captures.frames) {
      const question = saved.nodes[page.steps[frame.step].node].questions[0];
      assert.equal(frame.title, question.prompt);
      assert.equal(frame.kind, question.type);
      assert.ok(html.includes(`data-step="${frame.step}"`), name);
      for (const key of ["draft", "answer", "draftNarrow", "narrow", "graph"]) {
        const screen = frame[key];
        assert.equal(
          screen.columns,
          ["draft", "answer"].includes(key) ? 140 : 80,
        );
        assert.equal(screen.rows.length, 34);
      }
      assert.match(plain(frame.narrow), /Context/);
      assert.match(plain(frame.narrow), /Question/);
      assert.match(plain(frame.graph), /decisions/);
      count++;
    }
  }
  assert.ok(count >= 60, "covers simple and complex walkthroughs");
});

test("public TUI instructions use current fields and shortcuts", () => {
  const home = read("index.html");
  assert.ok(home.includes("examples/meeting/"));
  assert.ok(!home.includes("Start a line with") && !home.includes("/whatif"));
  const guide = read("guide/index.html")
    .split('<h2 id="interactive">')[1]
    .split('<h2 id="first">')[0];
  for (const text of [
    "Shift-Tab",
    "Options/Levels",
    "@budget.xlsx",
    "/rename",
    "Drag the divider",
  ])
    assert.ok(guide.includes(text), text);
  assert.ok(!guide.includes("/whatif") && !guide.includes("<code>?</code>"));
  assert.match(read("examples/index.html"), /Simple · start here/);
  assert.match(read("examples/index.html"), /Complex · build across sessions/);
});

test("form IDs reuse exact recorded answers and unmatched questions fail", async () => {
  const node = JSON.parse(read("src/data/examples/meeting.json")).nodes[0];
  const question = node.questions[0];
  const recording = { node, used: false, printed: new Map() };
  const stub = await startStub([recording], { tui: true });
  try {
    const criteria = Object.fromEntries(
      question.options.map((option, index) => [`form_${index}`, option.label]),
    );
    const response = await fetch(`http://127.0.0.1:${stub.port}/v1/systemone`, {
      method: "POST",
      body: JSON.stringify({
        questions: {
          generated_form_id: {
            type: "choice",
            instructions: question.prompt,
            criteria,
          },
        },
      }),
    });
    assert.equal(response.status, 200);
    const answer = (await response.json()).answers.generated_form_id;
    question.options.forEach((option, index) =>
      assert.equal(answer.probabilities[`form_${index}`], option.p),
    );
    assert.equal(
      answer.choice,
      `form_${question.options.findIndex((option) => option.id === question.selected)}`,
    );
    assert.equal(recording.used, true);
    const unmatched = await fetch(
      `http://127.0.0.1:${stub.port}/v1/systemone`,
      {
        method: "POST",
        body: JSON.stringify({
          questions: {
            unknown: { type: "noul", instructions: "A different question" },
          },
        }),
      },
    );
    assert.equal(unmatched.status, 500);
    assert.equal(stub.failures.length, 1);
  } finally {
    await new Promise((done) => stub.server.close(done));
  }
});
