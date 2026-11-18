# Testing

Use Nextest for Rust tests and Vitest for TypeScript and React tests. Keep browser end-to-end checks in the existing browser suite. Exact commands belong in [contributing](../CONTRIBUTING.md) and the [desktop guide](../apps/desktop/README.md).

## Rust test files

Keep a small test block inline when the full source file stays short and readable. Move larger blocks beside their feature:

```text
src/
  source.rs
  tests/
    source_tests.rs
    source_integration_tests.rs
tests/
  capture_integration_tests.rs
```

Every test file under `src/` needs a module declaration; Cargo does not discover
these files automatically. Use a path-mounted module when tests need private access:

```rust
#[cfg(test)]
#[path = "tests/source_tests.rs"]
mod tests;
```

Put tests that use only public APIs in the external `tests/` target.

A test that opens a test database connection is an integration test, including a disposable SQLite store. Name the file `tests/<file>_integration_tests.rs`. Pure validation and rule tests use `<file>_tests.rs`. Use an external test target or a path-mounted module as appropriate; do not expose private production functions solely to move tests.

Keep synthetic fixtures and shared helpers near the tests that need them. Never use installed provider stores or real session content as automated fixtures. Preserve the same cases when moving tests.

## TypeScript and React test files

Use colocated `*.test.ts` and `*.test.tsx` files for small modules and components. Larger feature suites can use a nearby `__tests__/` folder. Keep fixtures and `test-utils.tsx` near that feature rather than building a large global test helper.

Use [Testing Library's custom render pattern](https://testing-library.com/docs/react-testing-library/setup/#custom-render) when several tests need the same React providers. Query observable behavior and accessible controls, not component internals.

Name real API, database, or native-service tests `*.integration.test.ts(x)`. Mocked HTTP and jsdom component tests do not qualify those boundaries. Keep browser E2E tests in their existing suite.

[Vitest projects](https://vitest.dev/guide/projects) can separate environments or service setup when needed. Do not add projects or duplicate discovery merely to organize files. Keep using the package's existing runner and cleanup.

Split tests by behavior and boundary. Apply this layout to new or substantially changed suites without unrelated repository-wide moves.
