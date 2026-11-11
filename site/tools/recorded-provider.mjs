// Replay saved model answers through a local provider, without live calls.
import { createServer } from "node:http";
const MODEL = "jev-1.14.0";
const promptOf = (q) => (typeof q.instructions === "string" ? q.instructions : q.instructions?.question);
const labelsOf = (q) =>
  q.type === "choice" ? q.criteria : (q.criteria ?? []).map((l) => (typeof l === "string" ? l : l.label));

/** Option IDs come from labels in the current CLI, so questions match on prompt and labels. */
const sortedLabels = (labels) => JSON.stringify([...labels].sort());

/** Does a recorded saved question match what the model was asked? */
function sameQuestion(recorded, asked) {
  if (recorded.type !== asked.type || recorded.prompt !== promptOf(asked)) return false;
  if (asked.type === "choice") return sortedLabels(recorded.options.map((o) => o.label)) === sortedLabels(Object.values(asked.criteria));
  if (asked.type === "score") return JSON.stringify(recorded.levels.map((l) => l.label)) === JSON.stringify(labelsOf(asked));
  return true;
}

/** Find the first unused recording that matches. Saved nodes match on prompt and labels;
 * a step that saved nothing has only its printed answers, so it matches on question IDs, and
 * on option labels when the printed bars show them. */
export function match(recorded, asked) {
  const ids = Object.keys(asked).sort();
  return recorded.find((entry) => {
    if (entry.used) return false;
    if (entry.node) {
      const known = entry.node.questions.map((q) => q.id).sort();
      return JSON.stringify(known) === JSON.stringify(ids) && entry.node.questions.every((q) => sameQuestion(q, asked[q.id]));
    }
    return false;
  });
}

export function answer(entry, id, asked) {
  const saved = entry.node.questions.find((q) => q.id === id);
  if (asked.type === "noul") return { type: "noul", noul: saved.probability };
  if (asked.type === "choice") {
    const ids = Object.keys(asked.criteria);
    const probabilities = Object.fromEntries(ids.map((id) => [id, saved.options.find((o) => o.label === asked.criteria[id]).p]));
    const label = saved.options.find((o) => o.id === saved.selected).label;
    return { type: "choice", choice: ids.find((id) => asked.criteria[id] === label), probabilities };
  }
  return { type: "score", score: saved.expected, probabilities: Object.fromEntries(saved.levels.map((l, i) => [String(i), l.p])) };
}

export function startStub(recorded, { tui = false } = {}) {
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
      let aliases = Object.fromEntries(Object.keys(asked).map((id) => [id, id]));
      let entry = match(recorded, asked);
      // The form assigns IDs. Match the same recorded wording/types/labels,
      // then return the original probabilities under the form's actual IDs.
      if (!entry && tui) {
        for (const candidate of recorded.filter((r) => !r.used && r.node)) {
          const pairs = Object.entries(asked).map(([id, q]) => [id, candidate.node.questions.find((saved) => sameQuestion(saved, q))?.id]);
          if (pairs.some(([, id]) => !id) || new Set(pairs.map(([, id]) => id)).size !== candidate.node.questions.length) continue;
          const canonical = Object.fromEntries(pairs.map(([actual, id]) => [id, asked[actual]]));
          if (match([candidate], canonical)) { entry = candidate; aliases = Object.fromEntries(pairs); break; }
        }
      }
      if (!entry) {
        const summary = Object.entries(asked).map(([id, q]) => `${id} (${q.type}): ${promptOf(q)} ${JSON.stringify(q.criteria ?? [])}`).join("; ");
        failures.push(`no recorded result matches this call: ${summary}`);
        return reply(500, { error: "no recording" });
      }
      entry.used = true;
      const answers = Object.fromEntries(Object.entries(asked).map(([id, q]) => [id, answer(entry, aliases[id], q)]));
      reply(200, { model: MODEL, answers, usage: { input_tokens: 100, output_tokens: 20 } });
    });
  });
  return new Promise((done) => server.listen(0, "127.0.0.1", () => done({ server, failures, port: server.address().port })));
}

// ---- running commands ------------------------------------------------------------------------

export function configFor({ live, port }) {
  const provider = live
    ? `base_url = "${process.env.TYPESAFE_BASE_URL || "https://api.typesafe.ai"}"\napi_key_env = "TYPESAFE_TOKEN"`
    : `base_url = "http://127.0.0.1:${port}"`;
  return `default_provider = "typesafe"
default_model = "${live ? "jev-latest" : MODEL}"

[execution]
max_attempts = 1

[providers.typesafe]
protocol = "system-one"
${provider}

[providers.typesafe.connection]
image_encoding = "assets"
min_image_dimension = 280
max_image_dimension = 1568
pixel_patch_size = 28
pixel_multiplier = 4
pixel_overhead = 256

[providers.typesafe.connection.headers]
TypeSafe-Danger-Zone-Images = "true"

[providers.typesafe.models."${live ? "jev-latest" : MODEL}"]
context_budget_tokens = ${live ? 8192 : 100000}
reserved_output_tokens = 128
`;
}

