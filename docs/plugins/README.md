# antiburn plugin architecture

Start with [how the plugin pieces fit together](architecture.md): shared core,
host adapters, maintained sources, assembly outputs and marketplace selection.
That document owns the system map. The guides below own operational procedures
and acceptance records.

| Guide                                                        | Use it for                                                                       |
| ------------------------------------------------------------ | -------------------------------------------------------------------------------- |
| [Plugin page](../../plugins/README.md)                       | Current public scope, installation and usage.                                    |
| [Session checks](session-checks.md)                          | App-owned evaluation, CLI delivery, native adapters and Claude development pane. |
| [Architecture](architecture.md)                              | Where to change code and how it reaches an artifact.                             |
| [Script tooling](script-tooling.md)                          | JavaScript/TypeScript execution, extension decisions and type-check boundaries.  |
| [Installation](installation.md)                              | Claude source loading and isolated lifecycle validation.                         |
| [Development adapters](development-adapters.md)              | Contributor testing of unpublished vendor candidates and visual audits.          |
| [Distribution](distribution.md)                              | Catalog selection, immutable pins and promotion gates.                           |
| [Compatibility](compatibility.md)                            | Host contracts and the limits of candidate certification.                        |
| [Validation](validation.md)                                  | Recorded Claude acceptance evidence and outstanding limits.                      |
| [Development source](../../plugins/antiburn-live/README.md)  | Local development build, checks and debug controls.                              |
| [Shared core](../../packages/antiburn-plugin-core/README.md) | Package checks and actual SDK type prerequisites.                                |

Claude Code is the first rollout. Its public artifact contains usage UI and
native help/refresh commands; CLI discovery, live checks and debug previews stay
in development builds. Other vendor candidates remain unpublished. The sections
below retain the development discovery contract and existing documentation anchors.

## Development adapter output

The default marker is 🔥, followed by the brand name in code delimiters. A startup notice is one line:

```text
🔥 `antiburn` · CLI found
```

The other states are `CLI not found` after a completed search and `CLI check unavailable` when the host cannot finish the check. Neither blocks the coding session. An explicit status request can repeat the check.

Expanded status always states the connection limit:

```text
🔥 `antiburn`
CLI: Found
App connection: Not checked
```

Markdown surfaces render the brand name as inline code. Plain native APIs preserve literal backticks. ASCII output omits the emoji and retains the code delimiters. Native orange branding is optional; status remains normal foreground text. Words carry meaning without color. No ANSI escapes are emitted into model text. The flame emoji is the default; --ascii omits it and --flame remains a compatibility alias. See the artifact's options for overrides.

A startup event, explicit command, and model-mediated skill have different guarantees. Skills and prompt commands need the host's permitted tools and can consume model capacity; they cannot guarantee automatic startup. Read the [Claude installation guide](installation.md), [unpublished adapter runbooks](development-adapters.md) and [compatibility](compatibility.md) for each route's limits.

## Build and check

The product name is **antiburn plugin**; **mod** describes the host-native runtime. Build script names and the generated output path below retain the earlier internal `antiburn-shell` name. Use these exact paths when running the tools.

From the repository root:

```sh
node scripts/assemble-antiburn-shell.mjs
node --test packages/antiburn-plugin-core/test/*.test.mjs
node scripts/assemble-antiburn-shell.mjs --verify
```

For formatter, linter, and pinned host SDK type checks, follow [development checks](../../packages/antiburn-plugin-core/README.md).

For output locations, source ownership and catalog boundaries, see the
[architecture map](architecture.md#from-source-to-runnable-artifacts). The older
internal `antiburn-shell` name remains in these commands.

Use the generated artifact for host validation. A simulated host does not prove native UI, trusted loading, marketplace approval, Windows/Linux behavior, or remote-host support. Record those separately in the compatibility ledger.

## Privacy and boundaries

Discovery inspects only candidate paths and bounded executable search locations. It must never run `antiburn --version` or another guessed flag: today's executable may launch the desktop app. App-bundle presence alone is insufficient. GUI-launched hosts can have a different PATH from a terminal.

Discovery adds no analytics and does not establish app connectivity. The separate
[session-check delivery mode](session-checks.md) invokes the existing CLI with a
versioned protocol and reads saved app results. Its explicit status can report
protocol connectivity, with freshness always unevaluated. It adds no analytics
containing session IDs, check messages or evidence.

See [distribution channels](distribution.md) for public, team, private, and local release gates.
