# Plugin script tooling

This guide records the JavaScript/TypeScript execution boundaries and the
script-extension assessment. Start with the [architecture](architecture.md) for
how sources, builds and catalogs connect.

## Execution and build constraints

The root and core package declare Node `>=22`. Contributor commands execute
JavaScript directly, while CI selects Node 24. Native TypeScript stripping was
added in Node 22.6 and enabled by default in 22.18, so direct `.mts` execution is
not available throughout the declared range. Node strips types without checking
them, does not read tsconfig settings, and does not support TSX this way. See
[Node's TypeScript execution documentation](https://nodejs.org/docs/latest-v22.x/api/typescript.html).
Claude's host-managed TypeScript/TSX runtime is a separate execution boundary.

The existing `check.mjs` uses `allowJs`, `checkJs`, `NodeNext` and `noEmit`.
This checks JSDoc against actual SDK types; it does not emit JavaScript for
assembly. Its lint filters and the package/CI test commands select `.mjs`.
The plugin workflow does not install a TypeScript runner. These facts make a
rename-only conversion incompatible with current tooling.

## File-by-file extension assessment

The assessment covers the 22 added or changed `.mjs` files in the initial plugin
rollout. Filenames in the second column are relative to the directories in the first.

| Directory                                                      | Files retained as `.mjs`                                                                        | Reason                                                                                                                                                                     |
| -------------------------------------------------------------- | ----------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `packages/antiburn-plugin-core/`                               | `product.mjs`                                                                                   | Copied byte-for-byte into the frozen public artifact. Preserve the source and generated import contract.                                                                   |
| `plugins/antiburn/lib/`                                        | `product.mjs`                                                                                   | Part of the immutable 24-file public artifact, not a separate editable source.                                                                                             |
| `packages/antiburn-plugin-core/`                               | `core.mjs`, `discovery.mjs`, `probe.mjs`                                                        | Copied directly into development/Pi/OpenCode candidates. The development Claude adapter invokes Node with a fixed `lib/probe.mjs` argument.                                |
| `packages/antiburn-plugin-core/`                               | `content.mjs`                                                                                   | Direct assembler import; no compilation step exists.                                                                                                                       |
| `integrations/antiburn/pi/`, `integrations/antiburn/opencode/` | `antiburn.mjs` in each                                                                          | Assembly rewrites imports and emits named JavaScript entry points. Adapter tests load the source as JavaScript data URLs. Host SDK types do not certify `.mts` loading.    |
| `packages/antiburn-plugin-core/`                               | `check.mjs`                                                                                     | Direct Node bootstrap for lint/format/type checks. Its collector and lint filters do not select `.mts`; the type check has no emit stage.                                  |
| `packages/antiburn-plugin-core/`                               | `eslint.config.mjs`                                                                             | JavaScript config loads without a TypeScript config loader; its rules target JavaScript files.                                                                             |
| `scripts/`                                                     | `assemble-antiburn-core-plugin.mjs`, `assemble-claude-mod.mjs`                                  | Direct contributor/CI entry points; tests import both, and the cross-vendor assembler imports the Claude builder. No runner or emit stage exists.                          |
| `scripts/`                                                     | `audit-antiburn-core-plugin.mjs`, `check-claude-mod-lifecycle.mjs`                              | Direct Node commands importing the JavaScript assemblers; documented invocations use these paths.                                                                          |
| `scripts/`                                                     | `assemble-claude-mod.test.mjs`                                                                  | Explicit CI test path using `node --test`; imports the assembler directly.                                                                                                 |
| `packages/antiburn-plugin-core/test/`                          | `adapters.test.mjs`, `assembly.test.mjs`, `audit.test.mjs`, `core.test.mjs`, `product.test.mjs` | Package scripts and CI explicitly discover `*.test.mjs`. Current JSDoc type checking excludes `/test/`; renaming would not add checked coverage.                           |
| `scripts/`                                                     | `classify-ci-changes.mjs`, `classify-ci-changes.test.mjs`                                       | Existing repository-wide CI entry points. The plugin change adds only a catalog dependency and regression case; converting the control plane is a separate tooling change. |

Retain these extensions and the current JSDoc checks. A future `.mts` change
needs an explicit runtime minimum or compiler/runner decision, compatible
imports and emitted paths, updated checks and contributor/CI commands, and
separate validation of each affected host. Do not infer that compatibility from
Node 24 CI or introduce an extra build system solely to rename files.

## Type checks and runtime tests are separate

The [core check script](../../packages/antiburn-plugin-core/check.mjs) uses
JSDoc and actual SDK declarations to check JavaScript core modules, selected
integration entries and assembly/audit scripts. Its file list excludes the
JavaScript `test/` folder and does not include the native Claude TypeScript/TSX
entry points or test suites. The compiler can follow imports from its entry
files, but supplying Claude declarations alone does not add those native suites.

The [Claude source SDK check](../../plugins/antiburn-live/README.md#development)
uses the source mod's tsconfig and host-generated declarations. Run it separately
in an isolated test setup. The generated public and development runtime test
stages use `claude plugin test`; executing tests is not a TypeScript check.
See [core prerequisites](../../packages/antiburn-plugin-core/README.md) for the
SDK paths and [validation](validation.md) for recorded acceptance limits.

Keep this assessment current when runtime minimums, imports, file discovery,
compiler/runner choices or generated entry paths change.
