// Re-run a walkthrough's commands and write their output back into the page.
//
//   node scripts/replay.mjs --example NAME --nouli PATH [--check] [--live]
//
// Offline (the default): a local stub speaks the TypeSafe protocol and answers each model
// call with the result recorded for the matching walkthrough step, so a run is free and
// deterministic. Live (--live): the commands call the provider configured in the current
// directory's .env, which costs money, and the answers can differ from the recorded ones.
//
// Commands come from the page's <section class="step"> blocks. Each one runs in an isolated
// HOME, with the page's `example=...` line exported, and its output replaces the step's
// <pre class="out"> block. The graph data is then exported again from the isolated store.
// With --check nothing is written: the captured output is compared with the page, ignoring
// session and turn IDs, and the script fails when they differ.
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync, readdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { exportStore } from "./export-graph.mjs";

const site = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const MODEL = "jev-1.14.0";
const UUID = /[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/g;

// ---- the page --------------------------------------------------------------------------------

const ENTITIES = { "&amp;": "&", "&lt;": "<", "&gt;": ">", "&quot;": '"', "&#x27;": "'", "&#39;": "'" };
const unescapeHtml = (text) => text.replace(/&(?:amp|lt|gt|quot|#x27|#39);/g, (e) => ENTITIES[e]);
const escapeHtml = (text) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

/** Everything the replay needs from a walkthrough page. */
export function readPage(html) {
  const graph = html.match(/<body[^>]*data-graph="([^"]+)"/)?.[1];
  const steps = [];
  const re = /<section class="step"([^>]*)>([\s\S]*?)<\/section>/g;
  for (const match of html.matchAll(re)) {
    const command = match[2].match(/<pre><code>([\s\S]*?)<\/code><\/pre>/);
    const out = match[2].match(/<pre class="out"><code>([\s\S]*?)<\/code><\/pre>/);
    if (!command || !out) throw new Error(`step ${steps.length + 1} needs a command block and an output block`);
    const node = match[1].match(/data-node="(\d+)"/)?.[1];
    // Absolute positions of the output text, so it can be replaced in place.
    const outStart = match.index + match[0].indexOf(out[0]) + out[0].indexOf("<code>") + "<code>".length;
    steps.push({
      command: unescapeHtml(command[1]),
      output: unescapeHtml(out[1]),
      node: node === undefined ? null : Number(node),
      outStart,
      outEnd: outStart + out[1].length,
    });
  }
  const firstStep = html.indexOf('<section class="step"');
  const setup = [...html.slice(0, firstStep).matchAll(/<pre><code>([\s\S]*?)<\/code><\/pre>/g)]
    .map((m) => unescapeHtml(m[1]))
    .flatMap((block) => block.split("\n"))
    .map((line) => line.match(/^([A-Za-z_]\w*)=(\S*)$/))
    .filter(Boolean);
  const env = Object.fromEntries(setup.map((m) => [m[1], m[2]]));
  if (!graph || !steps.length || !env.example) throw new Error("not a walkthrough page: needs data-graph, steps, and example=");
  return { graph, steps, env };
}

// ---- what was recorded -----------------------------------------------------------------------

/** Parse the printed output of a saved run: selected answers, and native confidence. */
export function parseOutput(text) {
  const answers = new Map();
  let last = null;
  for (const line of text.split("\n")) {
    let m;
    if ((m = line.match(/^(\S+): choice: (\S+) \(.*\), P\(selected\)=(\S+)$/))) {
      last = { type: "choice", selected: m[2], p: Number(m[3]) };
      answers.set(m[1], last);
    } else if ((m = line.match(/^(\S+): noul: P\(yes\)=(\S+)$/))) {
      last = { type: "noul", p: Number(m[2]) };
      answers.set(m[1], last);
    } else if ((m = line.match(/^(\S+): expected score: (\S+), scale /))) {
      last = { type: "score", value: Number(m[2]) };
      answers.set(m[1], last);
    } else if ((m = line.match(/^(\S+): refused: /))) {
      last = { type: "refusal" };
      answers.set(m[1], last);
    } else if (last && (m = line.match(/^ {2}Native confidence \(.*\): value=(\S+?)(?:, answer confidence=(\S+))?$/))) {
      last.confidence = Number(m[1]);
      if (m[2] !== undefined) last.answerConfidence = Number(m[2]);
    }
  }
  return answers;
}

/** One recorded model call per step, in order: the saved node when there is one. */
export function recordings(page, graph) {
  return page.steps.map((step, index) => {
    const printed = parseOutput(step.output);
    const node = step.node === null ? null : graph.nodes[step.node];
    return { step: index + 1, node, printed, used: false };
  });
}

// ---- the stub provider -----------------------------------------------------------------------

const promptOf = (q) => (typeof q.instructions === "string" ? q.instructions : q.instructions?.question);
const labelsOf = (q) =>
  q.type === "choice" ? q.criteria : (q.criteria ?? []).map((l) => (typeof l === "string" ? l : l.label));

/** Does a recorded saved question match what the model was asked? */
function sameQuestion(recorded, id, asked) {
  if (recorded.id !== id || recorded.type !== asked.type || recorded.prompt !== promptOf(asked)) return false;
  const labels = labelsOf(asked);
  if (asked.type === "choice") {
    const have = Object.fromEntries(recorded.options.map((o) => [o.id, o.label]));
    return JSON.stringify(Object.entries(have).sort()) === JSON.stringify(Object.entries(labels).sort());
  }
  if (asked.type === "score") return JSON.stringify(recorded.levels.map((l) => l.label)) === JSON.stringify(labels);
  return true;
}

/** Find the first unused recording that matches. Saved nodes match on prompt and labels;
 * a step that saved nothing has only its printed answers, so it matches on question IDs. */
export function match(recorded, asked) {
  const ids = Object.keys(asked).sort();
  return recorded.find((entry) => {
    if (entry.used) return false;
    if (entry.node) {
      const known = entry.node.questions.map((q) => q.id).sort();
      return JSON.stringify(known) === JSON.stringify(ids) && entry.node.questions.every((q) => sameQuestion(q, q.id, asked[q.id]));
    }
    const known = [...entry.printed.keys()].sort();
    return JSON.stringify(known) === JSON.stringify(ids) && ids.every((id) => entry.printed.get(id).type === asked[id].type || entry.printed.get(id).type === "refusal");
  });
}

const round = (n) => Math.round(n * 1e9) / 1e9;

/** The wire answer for one question. Saved nodes hold every probability. A step that saved
 * nothing printed only the selected one, so the rest is spread evenly across the others. */
export function answer(entry, id, asked) {
  const printed = entry.printed.get(id);
  const out = {};
  if (printed?.confidence !== undefined) out.confidence = printed.confidence;
  if (printed?.answerConfidence !== undefined) out.answer_confidence = printed.answerConfidence;
  const saved = entry.node?.questions.find((q) => q.id === id);
  if (asked.type === "noul") return { type: "noul", noul: saved ? saved.probability : printed.p, ...out };
  if (asked.type === "choice") {
    const ids = Object.keys(asked.criteria);
    let probabilities;
    let choice;
    if (saved) {
      probabilities = Object.fromEntries(saved.options.map((o) => [o.id, o.p]));
      choice = saved.selected;
    } else {
      choice = printed.selected;
      const rest = ids.length > 1 ? round((1 - printed.p) / (ids.length - 1)) : 0;
      probabilities = Object.fromEntries(ids.map((o) => [o, o === choice ? printed.p : rest]));
    }
    return { type: "choice", choice, probabilities, ...out };
  }
  const count = asked.criteria.length;
  let levels;
  let value;
  if (saved) {
    levels = saved.levels.map((l) => l.p);
    value = saved.expected;
  } else {
    value = printed.value;
    const low = Math.min(Math.floor(value), count - 1);
    const high = Math.min(low + 1, count - 1);
    levels = Array(count).fill(0);
    levels[high] = round(value - low);
    levels[low] = round(1 - levels[high]);
  }
  return { type: "score", score: value, probabilities: Object.fromEntries(levels.map((p, i) => [String(i), p])), ...out };
}

function startStub(recorded) {
  const failures = [];
  const server = createServer((request, response) => {
    let body = "";
    request.on("data", (chunk) => (body += chunk));
    request.on("end", () => {
      const reply = (status, value) => {
        response.writeHead(status, { "content-type": "application/json" });
        response.end(JSON.stringify(value));
      };
      if (request.method !== "POST" || request.url !== "/v1/systemone") return reply(404, { error: "unknown route" });
      const asked = JSON.parse(body).questions;
      const entry = match(recorded, asked);
      if (!entry) {
        const summary = Object.entries(asked).map(([id, q]) => `${id}: ${promptOf(q)}`).join("; ");
        failures.push(`no recorded result matches this call: ${summary}`);
        return reply(500, { error: "no recording" });
      }
      entry.used = true;
      const answers = Object.fromEntries(Object.entries(asked).map(([id, q]) => [id, answer(entry, id, q)]));
      reply(200, { model: MODEL, answers, usage: { input_tokens: 100, output_tokens: 20 } });
    });
  });
  return new Promise((done) => server.listen(0, "127.0.0.1", () => done({ server, failures, port: server.address().port })));
}

// ---- running commands ------------------------------------------------------------------------

function configFor({ live, port }) {
  const provider = live
    ? `base_url = "${process.env.TYPESAFE_BASE_URL || "https://api.typesafe.ai"}"\napi_key_env = "TYPESAFE_TOKEN"`
    : `base_url = "http://127.0.0.1:${port}"`;
  return `default_provider = "typesafe"
default_model = "${live ? "jev-latest" : MODEL}"

[execution]
max_attempts = 1

[providers.typesafe]
protocol = "typesafe"
${provider}

[providers.typesafe.models."${live ? "jev-latest" : MODEL}"]
context_budget_tokens = ${live ? 8192 : 100000}
reserved_output_tokens = 128
`;
}

/**
 * Some pages print commands whose single-quoted text holds a bare apostrophe ("the founders'
 * network"), which a shell cannot run. An apostrophe that opens a quoted argument follows an
 * option and a space, and one that closes it ends the line; any other is part of the text.
 */
export function repairQuotes(command) {
  let open = false;
  let out = "";
  for (let index = 0; index < command.length; index++) {
    const char = command[index];
    if (char !== "'") {
      out += char;
    } else if (open && command.startsWith("'\\''", index)) {
      // Already escaped for the shell.
      out += "'\\''";
      index += 3;
    } else if (!open && /(?:^|\s)(?:-[a-z]|--[a-z-]+) $/.test(command.slice(0, index))) {
      open = true;
      out += char;
    } else if (open && /^(?: \\)?(?:\n|$)/.test(command.slice(index + 1))) {
      open = false;
      out += char;
    } else {
      out += open ? "'\\''" : char;
    }
  }
  return out;
}

function syntaxOk(command) {
  return new Promise((done) => {
    const child = spawn("sh", ["-n", "-c", command]);
    child.on("close", (code) => done(code === 0));
  });
}

async function run(command, { cwd, env }) {
  // A stray apostrophe can leave a command that parses but means something else.
  const repaired = repairQuotes(command);
  if (repaired !== command && (await syntaxOk(repaired))) command = repaired;
  return new Promise((done) => {
    const child = spawn("sh", ["-c", command], { cwd, env });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("close", (code) => done({ code, stdout, stderr }));
  });
}

const normalize = (text) => text.replace(UUID, "<id>").replace(/\s+$/, "");

/** Replay one walkthrough. Returns the problems found, and the new page and graph. */
export async function replay({ example, nouli, live = false, check = false }) {
  const pagePath = join(site, "examples", example, "index.html");
  const html = readFileSync(pagePath, "utf8");
  const page = readPage(html);
  const graphPath = join(site, "src", "data", "examples", `${page.graph}.json`);
  const recorded = recordings(page, JSON.parse(readFileSync(graphPath, "utf8")));
  const stub = live ? null : await startStub(recorded);
  const home = mkdtempSync(join(tmpdir(), "nouli-replay-"));
  const problems = [];
  try {
    // The page's commands call `nouli`; give them exactly the binary they were asked to use.
    const bin = join(home, "bin");
    mkdirSync(bin);
    symlinkSync(resolve(nouli), join(bin, "nouli"));
    mkdirSync(join(home, ".nouli"));
    writeFileSync(join(home, ".nouli", "config.toml"), configFor({ live, port: stub?.port }));
    const env = {
      PATH: `${bin}:/usr/bin:/bin`,
      HOME: home,
      ...(live ? { TYPESAFE_TOKEN: process.env.TYPESAFE_TOKEN } : {}),
      ...page.env,
    };
    // Live runs read the provider key from the current directory's .env.
    const cwd = live ? process.cwd() : home;
    if (live) console.error("Live mode: every step calls your provider account and costs money.");
    const outputs = [];
    for (const [index, step] of page.steps.entries()) {
      const result = await run(step.command, { cwd, env });
      if (result.code !== 0) {
        problems.push(`step ${index + 1} exited with ${result.code}: ${result.stderr.trim() || result.stdout.trim()}`);
        outputs.push(null);
        continue;
      }
      outputs.push(result.stdout);
      if (check && normalize(result.stdout) !== normalize(step.output)) {
        problems.push(`step ${index + 1} differs:\n--- page\n${normalize(step.output)}\n--- replay\n${normalize(result.stdout)}`);
      }
    }
    for (const failure of stub?.failures ?? []) problems.push(failure);
    if (stub) for (const entry of recorded.filter((e) => !e.used)) problems.push(`step ${entry.step} recorded a result that no call used`);
    if (problems.length) return { problems };
    const rename = [[`^${page.env.example}-`, ""]];
    process.env.NOULI = resolve(nouli);
    const graph = exportStore(join(home, ".nouli"), rename);
    if (check) {
      const current = JSON.parse(readFileSync(graphPath, "utf8"));
      const same = normalize(JSON.stringify(graph, null, 1)) === normalize(JSON.stringify(current, null, 1));
      if (!same) problems.push("the exported graph data differs from the committed data (ignoring IDs)");
      return { problems };
    }
    let updated = html;
    // Replace from the end so earlier offsets stay valid.
    for (const [index, step] of [...page.steps.entries()].reverse()) {
      updated = updated.slice(0, step.outStart) + escapeHtml(outputs[index].replace(/\n$/, "")) + updated.slice(step.outEnd);
    }
    writeFileSync(pagePath, updated);
    writeFileSync(graphPath, JSON.stringify(graph, null, 1) + "\n");
    return { problems };
  } finally {
    stub?.server.close();
    rmSync(home, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  const opt = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
  const nouli = opt("--nouli") || process.env.NOULI;
  const wanted = opt("--example");
  if (!nouli || !wanted) {
    console.error("usage: node scripts/replay.mjs --example NAME|all --nouli PATH [--check] [--live]");
    process.exit(2);
  }
  const examples = wanted === "all" ? readdirSync(join(site, "examples"), { withFileTypes: true }).filter((e) => e.isDirectory()).map((e) => e.name) : [wanted];
  let failed = false;
  for (const example of examples) {
    const { problems } = await replay({ example, nouli, live: args.includes("--live"), check: args.includes("--check") });
    if (problems.length) {
      failed = true;
      console.error(`${example}: ${problems.length} problem(s)`);
      for (const problem of problems) console.error(`  ${problem.split("\n").join("\n  ")}`);
    } else {
      console.error(`${example}: ok`);
    }
  }
  process.exit(failed ? 1 : 0);
}
