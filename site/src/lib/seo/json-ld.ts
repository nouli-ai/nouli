/**
 * Serialize structured data for a `<script type="application/ld+json">` element. Escaping `<`
 * keeps any string in the data from closing the script element.
 */
export function serializeJsonLd(data: unknown): string {
  return JSON.stringify(data).replaceAll("<", "\\u003c");
}
