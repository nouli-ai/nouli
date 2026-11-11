import { defineCollection } from "astro:content";
import { glob } from "astro/loaders";
import { capture } from "./lib/schemas/capture";
import { example } from "./lib/schemas/example";
import { model } from "./lib/schemas/model";

/** Recorded terminal captures, such as the guide's meeting session. */
const captures = defineCollection({
  loader: glob({ pattern: "*.json", base: "./src/data/captures" }),
  schema: capture,
});

/** Published models, one file each, named by the model's URL slug. */
const models = defineCollection({
  loader: glob({ pattern: "*.json", base: "./src/content/models" }),
  schema: model,
});

/** Examples, one recorded case each, named by the example's URL slug. */
const examples = defineCollection({
  loader: glob({ pattern: "*.json", base: "./src/content/examples" }),
  schema: example,
});

export const collections = { captures, models, examples };
