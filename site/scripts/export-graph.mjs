// Convert a saved Nouli session into the graph data the site renders.
//
//   node scripts/export-graph.mjs --store DIR (--only NAME | --all) [--rename FROM=TO ...] > graph.json
//
// Reads `nouli sessions export`, so every question, probability, and context record
// comes from the ledger. `--all` merges every session in the store into one graph. Set NOULI to choose the executable (default: nouli on PATH).
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

const run = (store, ...args) =>
  JSON.parse(execFileSync(process.env.NOULI || "nouli", ["--store", store, ...args, "--format", "json"], { encoding: "utf8", maxBuffer: 64 << 20 }));

/** Export one session's captured history as site graph data. */
export function exportGraph(store, session, rename = []) {
  return toGraph(run(store, "sessions", "export", session), rename);
}

/** Export every session in a store as one graph, oldest turn first. */
export function exportStore(store, rename = []) {
  const exports = run(store, "sessions", "list").sessions.map((s) => run(store, "sessions", "export", s.id));
  const unique = (items) => [...new Map(items.map((x) => [x.id, x])).values()];
  const turns = unique(exports.flatMap((e) => e.turns)).sort((a, b) => (a.id < b.id ? -1 : 1));
  return toGraph({ session: exports.at(-1).session, turns, provenance: unique(exports.flatMap((e) => e.provenance)) }, rename);
}

function displayName(name, rename) {
  let out = name ?? "(unnamed)";
  for (const [from, to] of rename) out = out.replace(new RegExp(from), to);
  return out;
}

function question(q, outcome) {
  const base = { id: q.id, prompt: q.prompt, type: q.answer_space.type };
  if (outcome.status === "refused") return { ...base, refused: outcome.reason };
  const a = outcome.answer;
  if (a.type === "choice") {
    const p = Object.fromEntries(a.distribution.map((d) => [d.option_id, d.probability]));
    return { ...base, options: q.answer_space.options.map((o) => ({ id: o.id, label: o.description, p: p[o.id] })), selected: a.selected };
  }
  if (a.type === "noul") return { ...base, probability: a.probability };
  const levels = q.answer_space.levels.map((l) => (typeof l === "string" ? l : l.label));
  return { ...base, levels: levels.map((label, i) => ({ label, p: a.probabilities[i] })), expected: a.value };
}

function toGraph(exp, rename) {
  const names = new Map(exp.provenance.map((s) => [s.id, displayName(s.name, rename)]));
  const sources = new Map(exp.provenance.map((s) => [s.id, s.sources?.length ?? 0]));
  const index = new Map(exp.turns.map((t, i) => [t.id, i]));
  // Facts are records added by a turn's updates; context lists them by record, so map each
  // record back to the turn that saved it.
  const added = (t) => (t.updates ?? []).map((u) => u.record ?? u.replacement).filter(Boolean);
  for (const t of exp.turns) for (const r of added(t)) if (!index.has(r.id)) index.set(r.id, index.get(t.id));
  const counts = new Map();
  const nodes = exp.turns.map((t) => {
    const n = (counts.get(t.session) ?? 0) + 1;
    counts.set(t.session, n);
    const parents = t.parents.map((p) => index.get(p));
    const first = !t.parents.some((p) => exp.turns[index.get(p)].session === t.session);
    const src = sources.get(t.session);
    const kind = !first ? "continued" : src > 1 ? "blend" : src === 1 ? "branch" : "new";
    const outcomes = new Map(t.outcome.outcomes.map((o) => [o.question_id, o]));
    return {
      id: t.id,
      session: names.get(t.session),
      turn: n,
      kind,
      parents,
      state: [
        ...t.request.state.map((s) => (s.type === "text" ? s.text : JSON.stringify(s.value))),
        ...added(t).map((r) => (typeof r.value === "string" ? r.value : JSON.stringify(r.value))),
      ].join("\n"),
      questions: t.request.questions.map((q) => question(q, outcomes.get(q.id))),
      // Records Nouli actually placed in this turn's model context, in context order.
      // Earlier turns, each once, whether through its answers or the facts it added.
      context: [
        ...new Map(
          t.manifest.records
            .map((r) => ({ turn: index.get(r.record) ?? null, representation: r.representation }))
            .filter((c) => c.turn !== null && c.turn !== index.get(t.id))
            .map((c) => [c.turn, c]),
        ).values(),
      ],
      model: `${t.outcome.provider.provider}/${t.outcome.provider.model}`,
    };
  });
  for (const node of nodes) {
    const from = node.parents.map((p) => nodes[p].session);
    node.command =
      node.kind === "continued" ? (node.state ? `nouli add ${node.session} '…' -q '…'` : `nouli ask ${node.session} -q '…'`)
      : node.kind === "new" ? `nouli new ${node.session} '…' -q '…'`
      : `nouli new ${node.session} ${from.map((f) => `--from ${f}`).join(" ")} '…' -q '…'`;
  }
  return { session: displayName(exp.session.name, rename), nodes };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  const opt = (k) => args[args.indexOf(k) + 1];
  const rename = args.flatMap((a, i) => (args[i - 1] === "--rename" ? [a.split("=")] : []));
  const graph = args.includes("--all") ? exportStore(opt("--store"), rename) : exportGraph(opt("--store"), opt("--only"), rename);
  process.stdout.write(JSON.stringify(graph, null, 1) + "\n");
}
