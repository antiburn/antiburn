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

Numeric expectations in the tests are derived from these files, so a change here
means updating the assertions in `initial_context.rs`.
