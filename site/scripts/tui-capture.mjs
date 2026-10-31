// Capture the current full-screen TUI using the existing recorded answers.
import { spawn } from "node:child_process";
import {
  readFileSync,
  writeFileSync,
  mkdirSync,
  mkdtempSync,
  rmSync,
  readdirSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { resolve, join } from "node:path";
import {
  configFor,
  readPage,
  recordings,
  repairQuotes,
  startStub,
} from "./replay.mjs";
const site = resolve(import.meta.dirname, "..");
const PREFIX = "__example__";
const oneLine = (text) => text.replace(/\s*\n\s*/g, " ").trim();
/** The arguments a step passes to nouli, as the shell splits them. */
function argumentsOf(command) {
  return new Promise((done, fail) => {
    const script = `nouli() { for a; do printf '%s\\0' "$a"; done; }\n${repairQuotes(command)}`;
    const child = spawn("sh", ["-c", script], {
      env: { PATH: "/usr/bin:/bin", example: PREFIX },
    });
    let out = "";
    child.stdout.on("data", (chunk) => (out += chunk));
    child.on("close", (code) =>
      code === 0
        ? done(out.split("\0").slice(0, -1))
        : fail(new Error(`cannot split: ${command}`)),
    );
  });
}

/** A step's verb, session, sources, facts, and questions. */
function parse(args) {
  const name = (value) => value.replace(new RegExp(`^${PREFIX}-`), "");
  const step = {
    verb: null,
    session: null,
    from: [],
    facts: [],
    questions: [],
  };
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === "--format") i++;
    else if (arg === "--from") step.from.push(name(args[++i]));
    else if (arg === "-q" || arg === "--question")
      step.questions.push(oneLine(args[++i]));
    else if (arg.startsWith("-"))
      throw new Error(`no interactive form for ${arg}`);
    else if (!step.verb) step.verb = arg;
    else if (!step.session) step.session = name(arg);
    else step.facts.push(oneLine(arg));
  }
  return step;
}

const args = process.argv.slice(2);
const option = (name, fallback) =>
  args.includes(name) ? args[args.indexOf(name) + 1] : fallback;
const wanted = option("--example", "all");
const nouli = resolve(
  option("--nouli", process.env.NOULI || "../target/release/nouli"),
);
const python = option("--python", "python3");
const names =
  wanted === "all"
    ? readdirSync(join(site, "examples"), { withFileTypes: true })
        .filter((entry) => entry.isDirectory())
        .map((entry) => entry.name)
    : [wanted];
const escape = (text) =>
  text
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
for (const name of names) {
  const path = join(site, "examples", name, "index.html");
  const html = readFileSync(path, "utf8");
  const page = readPage(html);
  const graph = JSON.parse(
    readFileSync(
      join(site, "src", "data", "examples", `${page.graph}.json`),
      "utf8",
    ),
  );
  const calls = recordings(page, graph);
  const steps = await Promise.all(
    page.steps.map(async (step, index) => {
      const args = await argumentsOf(step.command);
      const parsed = parse(args);
      return {
        ...parsed,
        args: args.map((arg) => arg.replaceAll(`${PREFIX}-`, "")),
        question: calls[index].node?.questions[0],
        node: step.node,
        heading: index + 1,
      };
    }),
  );
  const stub = await startStub(calls, { tui: true });
  const home = mkdtempSync(join(tmpdir(), "nouli-tui-capture-"));
  try {
    mkdirSync(join(home, ".nouli"));
    writeFileSync(
      join(home, ".nouli/config.toml"),
      configFor({ live: false, port: stub.port }),
    );
    const output = await new Promise((done, fail) => {
      const child = spawn(python, [join(site, "scripts/capture-tui.py")], {
        env: { ...process.env, NOULI_CAPTURE_HOME: home },
      });
      let stdout = "",
        stderr = "";
      child.stdout.on("data", (part) => (stdout += part));
      child.stderr.on("data", (part) => (stderr += part));
      child.on("error", fail);
      child.on("close", (code) =>
        code === 0
          ? done(JSON.parse(stdout))
          : fail(new Error(stderr || stdout)),
      );
      child.stdin.end(JSON.stringify({ nouli, home, steps }));
    });
    if (stub.failures.length || calls.some((call) => !call.used))
      throw new Error(
        `${name}: incomplete replay: ${stub.failures.join("; ")}`,
      );
    const target = join(site, "src/data/tui");
    mkdirSync(target, { recursive: true });
    writeFileSync(join(target, `${name}.json`), JSON.stringify(output) + "\n");
    let updated = html
      .replace(/<pre class="repl"><code>[\s\S]*?<\/code><\/pre>\n/g, "")
      .replace(
        /<pre class="(?:repl-only|tui-only)"><code>[\s\S]*?<\/code><\/pre>\n/g,
        "",
      )
      .replace(/<p class="(?:repl-only|tui-only) note">[\s\S]*?<\/p>\n/g, "")
      .replace(/<div class="tui-snapshot"[^>]*>[\s\S]*?<\/div>\n/g, "");
    let index = 0;
    updated = updated.replace(
      /<section class="step"[^>]*>[\s\S]*?<\/section>/g,
      (section) => {
        const step = steps[index++];
        const insert =
          step.verb === "whatif"
            ? '<p class="tui-only note">This unsaved alternative uses the shell command:</p>\n' +
              `<pre class="tui-only"><code>${escape(page.steps[index - 1].command)}</code></pre>\n`
            : `<div class="tui-snapshot" data-capture="${name}" data-step="${index - 1}"><pre class="tui-static" aria-label="Captured current Nouli TUI"><code>${escape(
                output.frames
                  .find((frame) => frame.step === index - 1)
                  .narrow.rows.map((row) => row.map((run) => run[0]).join(""))
                  .join("\n"),
              )}</code></pre></div>\n`;
        return section.replace(/<pre><code>/, insert + "<pre><code>");
      },
    );
    updated = updated
      .replaceAll('data-view="repl"', 'data-view="tui"')
      .replaceAll('class="repl-only"', 'class="tui-only"');
    updated = updated.replace(
      /<p class="tui-only">Open the first session[\s\S]*?<\/p>/,
      `<p class="tui-only">Open <code>nouli open ${escape(steps[0].session)}</code>. Add each step’s context, question and answers in the form, then press Enter to send. Captures replay the recorded model answers in the current TUI.</p>`,
    );
    writeFileSync(path, updated);
    console.error(`${name}: ${output.frames.length} captured steps`);
  } finally {
    stub.server.close();
    rmSync(home, { recursive: true, force: true });
  }
}
