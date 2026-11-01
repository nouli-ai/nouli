// Record the home-page replay: drive the full-screen app through the workshop
// session in a PTY, answering from src/data/cli/graph.json through a local stub,
// then write the replay (tui.json), refreshed graph data and provenance.
//
//   node scripts/record-cli-replay.mjs --nouli ../target/release/nouli --python /path/to/python [--columns 64]
//
// The default 160 columns write tui.json, with the decision graph beside the
// transcript, and refresh graph.json. Narrower recordings write tui-narrow.json
// for small screens, which show only the transcript.
// The Python interpreter needs pyte. No provider credentials or live calls are used.
import { execFileSync, spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { chmodSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { configFor, startStub } from "./replay.mjs";
import { exportStore } from "./export-graph.mjs";

const site = resolve(import.meta.dirname, "..");
const data = join(site, "src/data/cli");
const args = process.argv.slice(2);
const option = (name) => {
  const at = args.indexOf(name);
  if (at < 0 || !args[at + 1]) throw new Error(`Missing ${name}`);
  return resolve(args[at + 1]);
};
const nouli = option("--nouli");
const python = option("--python");
const columns = args.includes("--columns") ? Number(args[args.indexOf("--columns") + 1]) : 160;
const wide = columns >= 116;
const tour = args.includes("--tour");
const name = tour ? "tour" : wide ? "wide" : "narrow";

// Keep the header, form, and configurable status line in the recording.
const PLAYBACK_START_MS = 150;

/** Recorded frames as the page's replay: the first ready screen, then only changed rows. */
export function toReplay(recording, application) {
  const crop = (frame) => ({
    rows: frame.rows,
    cursor: frame.cursor,
  });
  const frames = recording.frames;
  let previous = crop(frames[0]), previousAt = frames[0].t, elapsed = 0;
  const updates = [], times = [];
  for (const frame of frames.slice(1)) {
    const next = crop(frame);
    const rows = next.rows.flatMap((row, index) => (JSON.stringify(row) === JSON.stringify(previous.rows[index]) ? [] : [[index, row]]));
    if (!rows.length && JSON.stringify(next.cursor) === JSON.stringify(previous.cursor)) continue;
    let wait = Math.max(16, Math.round((frame.t - previousAt) * 1000));
    if (!updates.length) wait = Math.min(wait, PLAYBACK_START_MS);
    elapsed += wait;
    updates.push({ wait, rows, cursor: next.cursor });
    times.push([frame.t, elapsed]);
    previous = next;
    previousAt = frame.t;
  }
  // Each step's answer is on screen by the last update recorded before its marker.
  const steps = recording.markers
    .filter((marker) => marker.phase === "answer")
    .map((marker) => ({ session: marker.session, title: marker.title, at: times.findLast(([t]) => t <= marker.t)[1] }));
  const initial = crop(frames[0]);
  return { version: 3, application, columns: recording.columns, lines: recording.lines, initial, updates, final: previous, steps };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const graph = JSON.parse(readFileSync(join(data, "graph.json"), "utf8"));
  const calls = graph.nodes.map((node) => ({ node: { questions: node.questions }, printed: new Map(), used: false }));
  const stub = await startStub(calls, { tui: true });
  const dir = mkdtempSync(join(tmpdir(), "nouli-cli-replay-"));
  chmodSync(dir, 0o700);
  try {
    writeFileSync(join(dir, "config.toml"), configFor({ live: false, port: stub.port }), { mode: 0o600 });
    await new Promise((done, fail) => {
      const child = spawn(python, [join(site, "scripts/record-cli-replay.py")], {
        stdio: "inherit",
        env: { ...process.env, NOULI_BIN: nouli, REPLAY_DIR: dir, REPLAY_COLUMNS: String(columns), REPLAY_TOUR: tour ? "1" : "0" },
      });
      child.on("error", fail);
      child.on("exit", (code) => (code === 0 ? done() : fail(new Error(`Recording failed: ${code}`))));
    });
    if (stub.failures.length) throw new Error(stub.failures.join("\n"));
    if (calls.some((call) => !call.used)) throw new Error("Not every saved answer was replayed");
    const version = execFileSync(nouli, ["--version"], { encoding: "utf8" }).trim();
    const recording = JSON.parse(readFileSync(join(dir, "recording.json"), "utf8"));
    const replay = toReplay(recording, version);
    writeFileSync(join(data, tour ? "tui-tour.json" : wide ? "tui.json" : "tui-narrow.json"), JSON.stringify(replay));
    if (wide && !tour) {
      process.env.NOULI = nouli;
      writeFileSync(join(data, "graph.json"), JSON.stringify(exportStore(join(dir, "store")), null, 1) + "\n");
    }
    const recorded = {
      application: version,
      binary_sha256: createHash("sha256").update(readFileSync(nouli)).digest("hex"),
      capture: "Native PTY replay of saved TypeSafe answers through a local stub",
      live_provider_calls: 0,
      terminal: { columns: recording.columns, lines: recording.lines, displayed_lines: recording.lines },
      steps: replay.steps.length,
      updates: replay.updates.length,
      replay_milliseconds: replay.updates.reduce((total, update) => total + update.wait, 0),
      recorded_at: new Date().toISOString().replace(/\.\d+Z$/, "Z"),
    };
    const path = join(data, "provenance.json");
    let provenance = {};
    try {
      provenance = JSON.parse(readFileSync(path, "utf8"));
    } catch {}
    if (!provenance.wide && !provenance.narrow) provenance = {};
    provenance[name] = recorded;
    writeFileSync(path, JSON.stringify(provenance, null, 2) + "\n");
    console.log(JSON.stringify({ [name]: recorded.updates, steps: recorded.steps, liveProviderCalls: 0 }));
  } finally {
    stub.server.close();
    rmSync(dir, { recursive: true, force: true });
  }
}
