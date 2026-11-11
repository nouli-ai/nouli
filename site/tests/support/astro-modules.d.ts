// Plain TypeScript tools, such as the type-aware linter, cannot read `.astro` files. Tests that
// render components through the Container API import them as component factories.
declare module "*.astro" {
  import type { AstroComponentFactory } from "astro/runtime/server/index.js";
  const component: AstroComponentFactory;
  export default component;
}
