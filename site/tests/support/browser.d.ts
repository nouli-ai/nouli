// oxlint-disable-next-line unicorn/require-module-specifiers -- declares test-only browser globals.
export {};
declare global {
  interface Window {
    cls: number;
    cspViolations: string[];
  }
}
