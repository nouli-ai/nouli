import { modelPath, modelRepository } from "../models";
import type { Model } from "../schemas/model";
import { siteOrigin } from "../site";

const publisher = { "@type": "Organization", name: "Nouli AI", url: `${siteOrigin}/` };

/** Structured data for one model page. */
export const modelSourceCode = (slug: string, data: Model) => {
  const [first, ...more] = data.structuredData.licenses;
  return {
    "@context": "https://schema.org",
    "@type": "SoftwareSourceCode",
    name: data.fullName,
    description: data.structuredData.description,
    url: siteOrigin + modelPath(slug),
    codeRepository: modelRepository(slug),
    version: data.version,
    license: more.length === 0 ? first : data.structuredData.licenses,
    runtimePlatform: data.structuredData.runtimePlatform,
    author: publisher,
  };
};

/** Structured data for the model catalog, listing models in catalog order. */
export const modelCatalog = (models: readonly { id: string; data: Model }[]) => ({
  "@context": "https://schema.org",
  "@type": "CollectionPage",
  name: "Nouli open-source decision models",
  url: `${siteOrigin}/models/`,
  mainEntity: {
    "@type": "ItemList",
    itemListElement: models.map(({ id, data }, index) => ({
      "@type": "ListItem",
      position: index + 1,
      name: data.fullName,
      url: siteOrigin + modelPath(id),
    })),
  },
});
