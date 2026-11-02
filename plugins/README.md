# antiburn plugin

Native tools for getting more from your coding-agent sessions. antiburn is vendor-agnostic, with a shared foundation and an adapter for each agent. **Claude Code is the first rollout:** account usage, reset times and pace above its prompt, in both Terminal and the Code tab of Claude Desktop.

The plugin works without the antiburn desktop app or CLI. The current repository candidate is **0.2.6**; other agent adapters are not published yet.

[Install](#install) · [Usage](#use-the-claude-mod) · [Supported agents](#supported-agents) · [Privacy](#privacy)

## The first rollout: Claude Code

The Claude plugin carries a **mod**: a native runtime that draws inside Claude Code and handles its commands. It shows:

- **Five-hour and weekly usage** as percentages and dot meters.
- **Reset times** so you can see when each window renews.
- **Even pace markers** to compare usage with how much of the window has elapsed.
- **Readable usage details and Refresh**, without sending a model prompt.

These are account usage limits reported by Claude, not context-window fill or a cost estimate. Pace is a reference point, not a forecast. Missing readings stay unavailable; a failed refresh can retain the last known reading with a stale status.

**Desktop Code · usage states**

![antiburn usage-state animation above the prompt in Claude Desktop Code](https://raw.githubusercontent.com/antiburn/antiburn/c608678f4b38ec36be0a02e0b361a031fe55b9ef/plugins/assets/claude-code-desktop-usage-states.gif)

**Desktop Code · hover details**

![antiburn reset and pace hover details in Claude Desktop Code](https://raw.githubusercontent.com/antiburn/antiburn/c608678f4b38ec36be0a02e0b361a031fe55b9ef/plugins/assets/claude-code-desktop-hover-states.gif)

**Terminal · usage states**

![antiburn usage-state animation above the prompt in Claude Code Terminal](https://raw.githubusercontent.com/antiburn/antiburn/c608678f4b38ec36be0a02e0b361a031fe55b9ef/plugins/assets/claude-code-terminal-usage-states.gif)

**Terminal · hover details**

![antiburn reset and pace hover details in Claude Code Terminal](https://raw.githubusercontent.com/antiburn/antiburn/c608678f4b38ec36be0a02e0b361a031fe55b9ef/plugins/assets/claude-code-terminal-hover-states.gif)

The usage-state demos use synthetic preview data; the hover examples show reset and pace details. Each capture focuses on the strip and prompt. Terminal meters switch to separate rows when space is limited.

## Supported agents

| Agent                          | Rollout status                 | What is available                                                                       |
| ------------------------------ | ------------------------------ | --------------------------------------------------------------------------------------- |
| Claude Code                    | 0.2.6 repository candidate     | Native usage mod in Terminal and Desktop Code; install below                            |
| Codex, Cursor, OpenCode and Pi | Unpublished adapter candidates | Shared development foundation; no public installation or native compatibility claim yet |

Each adapter connects the shared product to its agent's package format, lifecycle and UI. Features depend on what that host exposes. Claude's usage meters do not imply equivalent quota access on other vendors.

For Claude, use a build with an enabled mod runtime. Focused acceptance covered Claude Code **2.1.304** in Terminal and Claude Desktop **2.42304.2** in its Code tab, on macOS. Other versions and platforms need their own checks. Ordinary Claude chat and Cowork are outside this integration. Account availability also depends on the host's login type and policy; API-key sessions do not provide the authenticated account fallback.

## Install

Run these commands in your shell:

```sh
claude plugin marketplace add https://github.com/antiburn/antiburn.git
claude plugin install antiburn@antiburn --scope user
```

Open a new Claude Code session. In an existing session, run `/reload-plugins`; restart Claude Code if the mod does not appear. Installation needs no antiburn configuration flags. In Desktop, use the **Code** tab. The mod appears once the session initializes; the blank new-session landing page has no active mod UI.

The repository candidate is **0.2.6**, with responsive Desktop meters and
anchored reset/pace hover details. Its candidate catalog selects artifact
`f9dabe20b7dac6d33a4e8fecad88e93f1fff87d5`. The catalog on `main`
determines the version installed by these commands; preparing a candidate does
not promote it. The submitted 0.2.4 artifact remains unchanged. Installation
does not depend on the feature branch. See the
[distribution guide](../docs/plugins/distribution.md) for promotion requirements
and the [validation scope](../docs/plugins/validation.md) for remaining native
Desktop and hosted-delivery acceptance.

## Use the Claude mod

The strip starts automatically when the mod loads. No model response is needed.

| Command or control              | Action                                     |
| ------------------------------- | ------------------------------------------ |
| `/antiburn`                     | Show usage, reset and pace details as text |
| `/antiburn refresh`             | Check usage and return a short result      |
| `/antiburn help`                | Explain the usage and refresh commands     |
| A meter's label or pace control | Reveal its reset or pace details           |
| Refresh / ↻, when shown         | Refresh without sending a message          |

In Terminal, press **Ctrl+X**, then **Tab** to focus the usage strip. Use **Tab** to move between controls and **Enter** to activate one. `/antiburn` provides the same information as text.

## Privacy

The public Claude mod reads native host quota measurements first. In Desktop Code, if those readings are empty or unavailable, it can request account usage from Anthropic through Claude Code's authenticated HTTP API. Claude Code keeps the credential; the mod receives an opaque authorization handle.

- No model calls, prompt submissions, usage polling or automatic retry timers.
- No reading of transcripts, project files, CLI caches or credentials files.
- No antiburn analytics, app connection or separate account database.
- Usage state stays in the host session; the mod does not persist readings.

Refresh shares in-flight requests and respects freshness and retry delays. Claude Code's own storage, networking and privacy policies still apply. The logo opens the antiburn website only when activated. Read the [full privacy description](antiburn/PRIVACY.md) for network destinations and capability boundaries before installing.

## Troubleshooting and known limitations

- **No strip in Desktop:** use the Code tab and initialize a session. Check that the plugin and mod runtime are enabled, then reload or restart the host.
- **“Isn't a command here” during startup:** an initial plugin command can show this warning despite executing successfully. Its cause remains under investigation. In focused checks it cleared after a later recognized public command; initializing through `/reload-plugins` did not show it. This is not a confirmed host bug or a guaranteed workaround.
- **Checking usage / Usage unavailable:** the host may not have supplied data, or account access may be unavailable. Try Refresh. Missing data is not zero usage; repeated refreshes do not bypass an account retry delay.
- **After clear or reload:** host lifecycle resets can remove the UI until the session initializes again. Last readings are not restored from a database.

Focused 0.2.1 checks covered automatic display, numeric usage, help and refresh in Desktop and Terminal, with no model output in the owned test sessions. They also covered dark full and split/reduced-width layouts. The smallest Desktop fallback breakpoint and the complete light/dark/reduced-motion matrix were not repeated in that pass. See the [validation scope](../docs/plugins/validation.md).

## Update or remove

For the marketplace source you already installed:

```sh
claude plugin marketplace update antiburn
claude plugin update antiburn@antiburn --scope user
```

Open a new session after updating. The permanent marketplace source is `https://github.com/antiburn/antiburn.git`, without a branch fragment.

To remove only the user-scoped plugin while retaining any host-managed plugin data:

```sh
claude plugin uninstall antiburn@antiburn --scope user --keep-data
```

Reload or restart the host afterward. Removal does not erase sessions or data that Claude Code retains independently.

## Development and contributing

To try the public candidate from a source checkout without installing it:

```sh
claude --plugin-dir ./plugins/antiburn
```

For contributor documentation, see the [development guide](antiburn-live/README.md), [shared core](../packages/antiburn-plugin-core/README.md) and [plugin architecture](../docs/plugins/architecture.md).

This is the plugin's page within the **antiburn/antiburn** repository. The [bundled 0.2.6 README](antiburn/README.md) describes the current repository candidate; the catalog's exact artifact SHA identifies the reviewed snapshot. This page is the current installation and usage guide. Plugin versions are independent of desktop app versions.

[Open an issue](https://github.com/antiburn/antiburn/issues) with the plugin and host versions, the affected surface and steps to reproduce. Leave credentials and private session content out of public reports. Contributions follow [CONTRIBUTING.md](../CONTRIBUTING.md). Licensed under [MIT](../LICENSE); bundled Bitcount lettering retains its [SIL OFL notice](antiburn/assets/bitcount/OFL.txt).

Claude directory submission is a separate step. The [directory submission guidance](../docs/plugins/distribution.md#claude-directory-submission) describes the source, validation gates and publication settings.
