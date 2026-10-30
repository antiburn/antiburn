> Status: the 0.2.6 Claude repository candidate retains the runtime tested at
> `143d4b05681938cb362e828ecc27d2e082638176`; its version and bundled documents
> are new. The earlier Terminal smoke and browser checks do not establish full
> native Desktop or hosted-delivery acceptance. See [validation](validation.md).
> Other-agent adapters remain unpublished; the tables below are historical.

# Plugin adapter compatibility

The unpublished discovery adapters check executable availability, not app connection or session analysis. A supported explicit route does not establish automatic startup support. Native host acceptance is separate from static package validation and simulated adapter tests.

## Predefined session checks

Local candidates now include native saved-result delivery hooks. Their protocol,
versioned acceptance record and evaluation limits are documented in
[session checks](session-checks.md). The historical discovery modes below do not
certify these newer routes. Codex/Cursor now have native check-hook entries in
addition to their explicit skills. No other-agent public release is implied.

## Historical discovery candidate modes

| Host        | Candidate integration                         | Invocation                         | Certification boundary                                                                           |
| ----------- | --------------------------------------------- | ---------------------------------- | ------------------------------------------------------------------------------------------------ |
| Claude Code | Native core module and core API namespace     | `/antiburn`                        | Validate against installed generated SDK; terminal and Desktop visuals need separate evidence    |
| Codex       | Skills-only plugin                            | Skill picker or installed `$` name | Explicit use; no public bundled startup hook; client and catalog installation checked separately |
| Cursor      | Native plugin with explicit-only skill        | `/antiburn`                        | Model-mediated explicit fallback; startup notices need separate execution and visibility proof   |
| OpenCode    | Runtime candidate and separate prompt command | `/antiburn` command asset          | Runtime initialization is not proof of session start; command installation is separate           |
| Pi          | Extension package                             | `/antiburn`                        | Native registration/session handler; TUI versus RPC behavior checked separately                  |

No fixed minimum host version is inferred from the date of a feature's documentation. Pin the build used for acceptance. Windows and Linux executable resolution can be covered by synthetic tests without claiming those hosts' native UI support. Remote environments inspect their own local executable search path; this does not establish availability on the user's laptop.

## Availability contract

- `found`: an executable candidate resolved in the host environment. No publisher/version/app state is asserted.
- `not-found`: the bounded search completed and found no executable candidate.
- `unavailable`: the host cannot reliably finish the check, including missing resolver runtime, permissions, or unsupported APIs.

A regular file's existence is not enough to claim executable permission on platforms that enforce it. A directory, broken link, or unresolvable configured path must not produce a false `found`. A permission or search-boundary failure must not silently become `not-found`.

A host may require an external resolver runtime because its sandboxed filesystem API lacks executable permission information. If that runtime is unavailable, report the limited state. Never substitute executing the candidate to test it.

## Presentation limits

The default is 🔥 followed by `antiburn` in code delimiters. Native plain-text APIs preserve literal backticks; Markdown surfaces render inline code. ASCII mode omits the emoji and retains code delimiters. Native orange branding is permitted only on a tested surface with adequate contrast in light and dark themes. Plain system notices cannot be recolored safely by injecting escapes. Keep the brand heading outside fenced diagnostic blocks on Markdown surfaces. Use ASCII when selected or required.

Rendering choices are adapter configuration, not a claim that glyph availability can be detected reliably. Flame emoji is optional. All statuses remain understandable without color. Startup output is one line and deduplicated when the host provides stable session state; explicit status is the fallback when that guarantee cannot be made.

## Distribution

Generated local catalogs use host-specific schemas. They do not modify the shared repository marketplace or the active mod release. Public directory acceptance, package namespace ownership, and remote cached installs are independent release gates. Keep one recommended channel per user; switching channels needs duplicate-name and update/removal testing.

See [Claude installation](installation.md) and the [unpublished adapter runbooks](development-adapters.md) for the local procedures and [README](README.md) for the implementation boundary. Current local validation results and captures belong under ignored `.agent-artifacts/reviews/`; they must not be committed as machine-specific product support claims.
