// Write each walkthrough step as the lines you would type in `nouli open`, with what it prints.
//
//   node scripts/repl-view.mjs --example NAME|all --nouli PATH [--check]
//
// The steps' commands become interactive lines (`/new`, `/blend A B as NAME`, `/use`, `/drop`,
// facts, then `?question`), and one `nouli open` session runs them all against the offline
// stub, which answers with the result recorded for each step. Nothing calls a provider. Each
// step's share of the transcript goes into a <pre class="repl"> block before its command. A
// what-if that asks its own question has no interactive form, so its step keeps the command.
// Every turn the session prints must match the step's recorded output. With --check nothing is
// written, and the script fails when a page's interactive blocks are out of date.
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { configFor, readPage, recordings, repairQuotes, startStub } from "./replay.mjs";

const site = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const PREFIX = "__example__";
const escapeHtml = (text) => text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
const oneLine = (text) => text.replace(/\s*\n\s*/g, " ").trim();

/** The arguments a step passes to nouli, as the shell splits them. */
function argumentsOf(command) {
  return new Promise((done, fail) => {
    const script = `nouli() { for a; do printf '%s\\0' "$a"; done; }\n${repairQuotes(command)}`;
    const child = spawn("sh", ["-c", script], { env: { PATH: "/usr/bin:/bin", example: PREFIX } });
    let out = "";
    child.stdout.on("data", (chunk) => (out += chunk));
    child.on("close", (code) => (code === 0 ? done(out.split("\0").slice(0, -1)) : fail(new Error(`cannot split: ${command}`))));
  });
}

/** A step's verb, session, sources, facts, questions, and dropped questions. */
function parse(args) {
  const name = (value) => value.replace(new RegExp(`^${PREFIX}-`), "");
  const step = { verb: null, session: null, from: [], facts: [], questions: [], drops: [] };
  for (let i = 0; i < args.length; i++) {
    const arg = args[i];
    if (arg === "--format") i++;
    else if (arg === "--from") step.from.push(name(args[++i]));
    else if (arg === "-q" || arg === "--question") step.questions.push(oneLine(args[++i]));
    else if (arg === "--drop") step.drops.push(args[++i]);
    else if (arg.startsWith("-")) throw new Error(`no interactive form for ${arg}`);
    else if (!step.verb) step.verb = arg;
    else if (!step.session) step.session = name(arg);
    else step.facts.push(oneLine(arg));
  }
  return step;
}

/** The lines a person types for each step, or null for a step that stays a command. */
function interactive(steps) {
  let current = null;
  return steps.map((step, index) => {
    if (step.verb === "whatif") return null;
    const lines = [];
    if (step.verb === "new" && index === 0) {
      // `nouli open` creates the first session.
    } else if (step.verb === "new" && step.from.length > 1) {
      lines.push(`/blend ${step.from.join(" ")} as ${step.session}`);
    } else if (step.verb === "new" && step.from.length === 1) {
      if (current !== step.from[0]) lines.push(`/use ${step.from[0]}`);
      lines.push(`/branch ${step.session}`);
    } else if (step.verb === "new") {
      lines.push(`/new ${step.session}`);
    } else if (step.verb === "add") {
      if (current !== step.session) lines.push(`/use ${step.session}`);
    } else {
      throw new Error(`no interactive form for ${step.verb}`);
    }
    current = step.session;
    lines.push(...step.drops.map((id) => `/drop ${id}`), ...step.facts);
    // A question sends the turn, so a step asks its new questions last.
    lines.push(...step.questions.map((q) => `?${q}`));
    if (!step.questions.length) lines.push("");
    return lines;
  });
}

function open(nouli, home, first, input) {
  return new Promise((done) => {
    const child = spawn(resolve(nouli), ["open", first], { cwd: home, env: { PATH: "/usr/bin:/bin", HOME: home } });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("close", (code) => done({ code, stdout, stderr }));
    child.stdin.end(input);
  });
}

/** Splits the transcript at each step's first typed line. */
function split(transcript, perStep, first) {
  const lines = transcript.replace(/\n$/, "").split("\n");
  const typed = perStep.flatMap((lines, step) => (lines ?? []).map((line) => ({ step, line: line === "" ? "↵" : line })));
  const blocks = perStep.map(() => []);
  let at = 0;
  let step = typed[0].step;
  blocks[step].push(`$ nouli open ${first}`);
  for (const line of lines) {
    const next = typed[at];
    if (next && line.endsWith(` › ${next.line}`)) {
      step = next.step;
      at++;
    } else if (line.endsWith(" › /quit")) {
      break;
    }
    blocks[step].push(line);
  }
  if (at !== typed.length) throw new Error(`the transcript ended after ${at} of ${typed.length} typed lines`);
  return blocks.map((block) => block.join("\n").replace(/\n+$/, ""));
}

/** The turn a step's command printed, without the line naming the model and IDs. */
const turnOf = (output) => output.replace(/\n+$/, "").replace(/\n {2}\S+\/\S+( · session .*)?$/, "");
const turnIn = (block) => block.split("\n").filter((line) => !/ › /.test(line) && !line.startsWith("$ ") && !/^ {2}(queued: |new session |\S+ · )/.test(line) && !/^new session /.test(line) && line !== "").join("\n");

const NOTE = '<p class="repl-only note">This what-if asks a question of its own, so it runs from the shell:</p>';
const VIEWS = (first) => `<div class="views" role="group" aria-label="Show the steps as">
<span>show the steps as</span><button type="button" data-view="repl" aria-pressed="true">interactive</button><button type="button" data-view="commands" aria-pressed="false">commands</button>
<p class="repl-only">Open the first session with <code>nouli open ${first}</code>, then type each step's lines at the prompt. A question sends the turn.</p>
</div>`;

/** The page with its interactive blocks replaced. */
function write(html, blocks, first) {
  let out = html
    .replace(/<pre class="repl"><code>[\s\S]*?<\/code><\/pre>\n/g, "")
    .replace(/<pre class="repl-only"><code>[\s\S]*?<\/code><\/pre>\n/g, "")
    .replace(new RegExp(`${NOTE.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\n`, "g"), "")
    .replace(/<div class="views"[\s\S]*?<\/div>\n/, "");
  let index = 0;
  out = out.replace(/<section class="step"[^>]*>[\s\S]*?<\/section>/g, (section) => {
    const block = blocks[index++];
    // A what-if keeps its command, with the sessions named as the interactive steps name them.
    const command = () => section.match(/<pre><code>([\s\S]*?)<\/code><\/pre>/)[1].replaceAll("&quot;$example-", "&quot;").replaceAll('"$example-', '"');
    const insert = block === null ? `${NOTE}\n<pre class="repl-only"><code>${command()}</code></pre>\n` : `<pre class="repl"><code>${escapeHtml(block)}</code></pre>\n`;
    return section.replace(/<pre><code>/, `${insert}<pre><code>`);
  });
  // Interactive is the default view, also without scripts. The session prefix only applies to
  // the commands.
  out = out.replace(/<body data-graph="([^"]+)">/, '<body data-graph="$1" data-view="repl">');
  out = out.replace(/(<p>Set a session prefix[\s\S]*?<\/p>\n<pre><code>example=[^<]*<\/code><\/pre>)\n(?!<\/div>)/, '<div class="cmd-only">\n$1\n</div>\n');
  return out.replace(/<section class="step"/, `${VIEWS(first)}\n<section class="step"`);
}

export async function replView({ example, nouli, check = false }) {
  const pagePath = join(site, "examples", example, "index.html");
  const html = readFileSync(pagePath, "utf8");
  const page = readPage(html);
  const graph = JSON.parse(readFileSync(join(site, "src", "data", "examples", `${page.graph}.json`), "utf8"));
  const steps = await Promise.all(page.steps.map(async (step) => parse(await argumentsOf(step.command))));
  if (steps[0].verb !== "new" || steps[0].from.length) throw new Error("the first step must create a session from nothing");
  const perStep = interactive(steps);
  const first = steps[0].session;
  const input = [...perStep.flat().filter((line) => line !== null), "/quit", ""].join("\n");
  const stub = await startStub(recordings(page, graph));
  const home = mkdtempSync(join(tmpdir(), "nouli-repl-view-"));
  const problems = [];
  try {
    mkdirSync(join(home, ".nouli"));
    writeFileSync(join(home, ".nouli", "config.toml"), configFor({ live: false, port: stub.port }));
    const result = await open(nouli, home, first, input);
    for (const failure of stub.failures) problems.push(failure);
    if (result.code !== 0) problems.push(`nouli open exited with ${result.code}: ${result.stderr.trim()}`);
    if (problems.length) return { problems };
    const blocks = split(result.stdout, perStep, first);
    blocks.forEach((block, index) => {
      if (perStep[index] === null) return;
      if (turnIn(block) !== turnOf(page.steps[index].output)) {
        problems.push(`step ${index + 1} printed a different turn:\n--- command\n${turnOf(page.steps[index].output)}\n--- interactive\n${turnIn(block)}`);
      }
    });
    const nulls = blocks.map((block, index) => (perStep[index] === null ? null : block));
    const updated = write(html, nulls, first);
    if (check) {
      if (updated !== html) problems.push("the interactive blocks are out of date");
    } else if (!problems.length) {
      writeFileSync(pagePath, updated);
    }
    return { problems };
  } finally {
    stub.server.close();
    rmSync(home, { recursive: true, force: true });
  }
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  const opt = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : undefined);
  const nouli = opt("--nouli") || process.env.NOULI;
  const wanted = opt("--example");
  if (!nouli || !wanted) {
    console.error("usage: node scripts/repl-view.mjs --example NAME|all --nouli PATH [--check]");
    process.exit(2);
  }
  const examples = wanted === "all" ? readdirSync(join(site, "examples"), { withFileTypes: true }).filter((e) => e.isDirectory()).map((e) => e.name) : [wanted];
  let failed = false;
  for (const example of examples) {
    const { problems } = await replView({ example, nouli, check: args.includes("--check") });
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
