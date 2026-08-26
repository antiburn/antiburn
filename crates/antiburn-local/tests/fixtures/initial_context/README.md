# Initial-context fixtures

Transcript fixtures for the initial-context attribution pass in
`src/analysis/initial_context.rs`. They are `include_str!`-ed by that module's
inline tests.

**Every file here is synthetic.** Each one was authored by hand from public
knowledge of the provider's on-disk transcript format and from the parser in
`initial_context.rs` — no real session, user, machine, or organization is
represented. Identities are deliberately fictional: the user is `avery`, the
project is `/home/avery/projects/demo-app`, and skill/MCP names (`orbit-tracker`,
`atlas-notes`, `ledger-sync`, `nebula-docs`) are invented and refer to nothing
that exists.

Keep it that way: if a fixture needs to grow, extend it by hand from the parser's
requirements. Do not paste in a captured transcript.

| File | Agent | Proves |
| --- | --- | --- |
| `claude_realistic.jsonl` | `claude` | A full Claude-shaped initial context: a `skill_listing` attachment with three named bullets (→ three named `skill_instructions` rows), an `mcp_instructions_delta` attachment (→ a named `mcp_instructions` row), and a "Base directory for this skill:" turn that merges into the matching skill row. |
| `codex_realistic.jsonl` | `codex` | A full Codex-shaped initial context: a `## Skills` section closed by `</skills_instructions>` with two named bullets (→ two named `skill_instructions` rows, and nothing derived from the closing tag or the prose after it). |
| `cursor_unsupported.jsonl` | `cursor` | An agent with no initial-context signal. The payload is a well-formed transcript, so the `None` result proves attribution is gated on the *agent label* (only `claude` and `codex` are supported) rather than on the payload failing to parse. |
| `claude_builtin_tools.jsonl` | `claude` | Built-in-tool rows (`source: "builtin_tool"`): a top-level `version`, a `message.model` id repeated enough to win over a single bare `sonnet` alias, a `deferred_tools_delta` attachment naming `Bash`, `CronCreate`, and `CronDelete` as deferred, and `Bash`/`Read` tool calls. Proves version/model resolution, the deferred-token estimate on an unused and a used deferred tool, and that a model-absent tool (`task_create`, not on `claude-fable-5` at this version) is left out. |
| `codex_builtin_tools.jsonl` | `codex` | Built-in-tool rows from `session_meta.payload.cli_version` and `turn_context.payload.model`, with two `apply_patch` function calls and no deferred-tool marker (Codex has none, so every tool loads at its measured cost). |
| `codex_namespaced_tools.jsonl` | `codex` | A 0.149.1/`gpt-5.6-sol` session whose catalogued tools carry namespaced aliases (`functions.exec`, `collaboration.spawn_agent`) that the transcript never calls by their dotted spelling: an `exec` custom-tool-call wraps a nested `tools.read(...)` call, and a plain `function_call` names `spawn_agent`. Proves the display name folds to an alias's bare last segment, and that the real Codex adapter's `wrapper_tool` bookkeeping still counts the `exec` wrapper's own use even though it unwraps into `read` for tool-mix accounting. |

These built-in-tool fixtures are read against the *committed fixture tool
catalogue* (`tests/fixtures/tool_catalog.json`, via `ToolCatalog::from_json`),
not the catalogue embedded in the binary — so their expected token counts stay
fixed regardless of what a local build or CI run happens to regenerate at
`src/analysis/tool_catalog.json`. See that file's own README for the
catalogue's shape and regeneration.

Numeric expectations in the tests are derived from these files, so a change here
means updating the assertions in `initial_context.rs`.
