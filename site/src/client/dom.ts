/** The first element under `root` matching `selector`, or an error naming what was missing. */
export function required<T extends Element>(
  root: ParentNode,
  selector: string,
  type: abstract new () => T,
): T {
  const found = root.querySelector(selector);
  if (!(found instanceof type)) {
    throw new TypeError(`Expected ${type.name} matching "${selector}"`);
  }
  return found;
}
