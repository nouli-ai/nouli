// Assemble the home-page figure from the sessions recorded by record-hero.sh.
//
//   node scripts/build-hero.mjs --store DIR > src/data/hero.json
//
// Each combination of train status and plan sources is a separate recorded session;
// this lays them out in five fixed slots so the page can switch between them.
import { pathToFileURL } from "node:url";
import { exportGraph } from "./export-graph.mjs";

const TRAIN = ["cancelled", "late", "ontime"];
const SOURCES = ["both", "travel", "budget"];
const RENAME = [["^hero-plan-.*", "plan"], ["^hero-", ""]];
const SLOTS = ["travel", "budget", "travel2", "plan", "plan2"];

function slotOf(node) {
  if (node.session.startsWith("plan")) return node.turn === 1 ? "plan" : "plan2";
  if (node.session === "budget") return "budget";
  return node.questions.some((q) => q.id === "on_time") ? "travel2" : "travel";
}

function place(nodes) {
  const byId = new Map();
  const out = new Array(SLOTS.length);
  for (const n of nodes) {
    const slot = SLOTS.indexOf(slotOf(n));
    byId.set(n.id, slot);
    out[slot] = n;
  }
  return { out, byId };
}

export function buildHero(store) {
  const travel = Object.fromEntries(TRAIN.map((t) => [t, exportGraph(store, t === "cancelled" ? "hero-travel" : `hero-travel-${t}`, RENAME).nodes]));
  const budget = exportGraph(store, "hero-budget", RENAME).nodes;
  const variants = {};
  for (const t of TRAIN) {
    for (const s of SOURCES) {
      const plan = exportGraph(store, s === "budget" ? "hero-plan-budget" : `hero-plan-${t}-${s}`, RENAME).nodes;
      // Sessions outside the plan's history still appear, unconnected, so the layout stays fixed.
      const tr = structuredClone(travel[t]), bu = structuredClone(budget);
      const seen = new Set(plan.map((n) => n.id));
      const all = [...plan, ...[...tr, ...bu].filter((n) => !seen.has(n.id))];
      const { out, byId } = place(all);
      const remap = (i, from) => byId.get(from[i].id);
      for (const n of out) {
        const from = plan.includes(n) ? plan : tr.includes(n) ? tr : bu;
        n.parents = n.parents.map((p) => remap(p, from));
        n.context = n.context.map((c) => ({ ...c, turn: c.turn === null ? null : remap(c.turn, from) }));
      }
      variants[`${t}|${s}`] = out;
    }
  }
  return { slots: SLOTS, train: TRAIN, sources: SOURCES, variants };
}

if (import.meta.url === pathToFileURL(process.argv[1]).href) {
  const args = process.argv.slice(2);
  process.stdout.write(JSON.stringify(buildHero(args[args.indexOf("--store") + 1]), null, 1) + "\n");
}
