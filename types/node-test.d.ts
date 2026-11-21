// The slice of Node's test runner the front-end tests use. The project has no
// @types/node; these declarations let `npm run typecheck` check the tests too.
// Add a member here when a test needs it.
declare module "node:test" {
  export function test(name: string, fn: () => void | Promise<void>): Promise<void>;
}

declare module "node:assert/strict" {
  const assert: {
    ok(value: unknown, message?: string): asserts value;
    equal<T>(actual: unknown, expected: T, message?: string): asserts actual is T;
    deepEqual<T>(actual: unknown, expected: T, message?: string): asserts actual is T;
  };
  export default assert;
}
