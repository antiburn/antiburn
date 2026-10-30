# Antiburn for Claude Code

Stop hitting your limits! Pace your Claude Code usage for five-hour and weekly limits across sessions. See usage and reset times at a glance, above your prompt in both desktop and terminal.

Antiburn shows account usage reported by Claude Code, including five-hour and
weekly percentages, reset times and even-pace markers. Pace compares usage with
the elapsed part of a limit window; it is a reference, not a forecast or a
guarantee that a limit will not be reached. Missing readings stay unavailable.

Version **0.2.6** keeps Desktop meters and their reset and pace controls aligned
as the window changes. Hover captions stay anchored to their controls in inline
and stacked layouts. It preserves glyph and dot size, other mods above the
prompt, and the existing Terminal rendering. The plugin version is independent of
Antiburn desktop app versions.

## Supported surfaces

Use Claude Code **Terminal** or the **Code tab in Claude Desktop**, with an
enabled mod runtime. Ordinary Claude chat and Cowork are outside this
integration. Account readings depend on the host version, login type and policy;
API-key sessions do not provide the authenticated account fallback.

Historical 0.2.1 native checks covered Claude Code 2.1.304 and Claude Desktop
2.42304.2 on macOS. Those observations retain their original scope. The
[validation record](https://github.com/antiburn/antiburn/blob/main/docs/plugins/validation.md)
distinguishes current packaging checks, historical runtime evidence and remaining
native acceptance work. Other hosts and platforms need their own checks.

## Preview

**Desktop Code · usage states**

![Antiburn usage states above the prompt in Claude Desktop Code](https://raw.githubusercontent.com/antiburn/antiburn/c608678f4b38ec36be0a02e0b361a031fe55b9ef/plugins/assets/claude-code-desktop-usage-states.gif)

Synthetic usage data illustrates the display states; this is not a current account reading.

**Desktop Code · reset and pace details**

![Antiburn reset and pace details in Claude Desktop Code](https://raw.githubusercontent.com/antiburn/antiburn/c608678f4b38ec36be0a02e0b361a031fe55b9ef/plugins/assets/claude-code-desktop-hover-states.gif)

Illustrative hover details from the public plugin guide. These presentation captures do not certify account freshness or release acceptance.

**Terminal · usage states**

![Antiburn usage states above the prompt in Claude Code Terminal](https://raw.githubusercontent.com/antiburn/antiburn/c608678f4b38ec36be0a02e0b361a031fe55b9ef/plugins/assets/claude-code-terminal-usage-states.gif)

Synthetic usage data illustrates the display states. Terminal meters stack when space is limited.

**Terminal · reset and pace details**

![Antiburn reset and pace details in Claude Code Terminal](https://raw.githubusercontent.com/antiburn/antiburn/c608678f4b38ec36be0a02e0b361a031fe55b9ef/plugins/assets/claude-code-terminal-hover-states.gif)

Illustrative hover details from the public plugin guide; not release acceptance evidence.

## Install

Run these commands in your shell:

```sh
claude plugin marketplace add https://github.com/antiburn/antiburn.git
claude plugin install antiburn@antiburn --scope user
```

The equivalent one-step install on Claude Code 2.1.275 or later is:

```text
/plugin install antiburn --marketplace antiburn/antiburn
```

The permanent marketplace follows `main` and selects an immutable artifact
commit. It does not require a release branch, tag or GitHub Release. The
[distribution guide](https://github.com/antiburn/antiburn/blob/main/docs/plugins/distribution.md)
records promotion and submission status; a repository install is separate from
Claude directory availability.

If you previously installed the owned standalone Antiburn findings skill, retire
it in the selected scope before installing the plugin, following the
[migration instructions](https://github.com/antiburn/antiburn/blob/main/docs/antiburn-shell/migration.md#retire-the-owned-standalone-skill).

Open a new session after installing. In an existing session, run
`/reload-plugins`; restart Claude Code if the mod does not appear. In Desktop,
initialize a session in the **Code** tab. The blank new-session landing page has
no active mod UI. No Antiburn configuration flag is required.

## Usage

The strip starts automatically when the mod loads. It displays native host
measurements first, including partial readings. After Desktop attaches, empty
or unavailable native data can fall back to an authenticated account request
through Claude Code. Terminal-only sessions use native measurements.

| Command or control              | Action                                     |
| ------------------------------- | ------------------------------------------ |
| `/antiburn`                     | Show usage, reset and pace details as text |
| `/antiburn refresh`             | Check usage and return a short result      |
| `/antiburn help`                | Explain the usage and refresh commands     |
| A meter's label or pace control | Reveal reset or pace details               |
| Refresh / ↻, when shown         | Check the available usage sources          |

In Terminal, press **Ctrl+X**, then **Tab** to focus the strip. Use **Tab** to
move between controls and **Enter** to activate one. Fullscreen terminal
rendering supports pointer reveals; classic rendering retains the command and
keyboard alternatives. Activate the logo to open https://antiburn.com.

## Privacy

The public plugin reads host quota measurements and can request account usage
from `https://api.anthropic.com/api/oauth/usage` through Claude Code's HTTP API.
The host keeps the credential and supplies an opaque authorization handle.
Usage state stays in memory in the host session; the plugin does not persist
readings, read transcripts or credentials files, or send plugin telemetry.
The account request is a GET with no body. Its explicit headers are
`anthropic-beta: oauth-2025-04-20` and `accept: application/json`. No prompt,
conversation, project file, or quota content is included in the request.
The plugin does not read credential files, the Keychain, or credential environment
variables. Claude Code controls its own storage, logs, networking and retention.

The SVG namespace `http://www.w3.org/2000/svg` identifies local image markup;
it is not a network destination. A separate constant supplies the namespace.
Template expressions in the graphics source format local SVG values; they do
not read environment variables.

Only activating the Terminal logo starts a program. Windows uses
`cmd.exe /c start "" https://antiburn.com`; other systems try
`open https://antiburn.com`, then `xdg-open https://antiburn.com` if needed.
Each attempt has a five-second timeout. These commands open a fixed website;
they do not download or execute code from it. Desktop uses a native link.
No fetched value, prompt, or session content becomes a command argument.

The `command.run` hook handles Antiburn usage/help/refresh and reconciles quota
after clear or resume. It forwards other commands without changes. The
`classic.SessionStart` hook resets quota display after clear and forwards the
host event unchanged. The `engine.create` hook adds Antiburn's description call.
The `AbovePrompt` hook calls the downstream rendering chain once and preserves
its returned UI before adding the usage strip. Composition requires no plugin
identity or capability probe. Surveys and hidden bands pass through unchanged.
The host scrolls the combined tree when it exceeds the available row budget.

Successful fallback readings are reused for 60 seconds. Concurrent refreshes
share one request, and failures defer further account requests with backoff.
Refresh still checks native readings while account requests are deferred.
The plugin does not poll usage or submit model prompts. Read the bundled
[privacy description](PRIVACY.md) for destinations, authorization, retention and
website activation details.

## Troubleshooting

- **No strip:** enable the plugin and mod runtime, initialize a supported
  session, then reload or restart the host. Managed policies can block mods.
- **Checking usage / Usage unavailable:** the host may not have supplied data or
  account access may be unavailable. Try Refresh. Missing data is not zero usage.
- **Account retry at a time:** refresh does not bypass account backoff. The
  displayed time is a retry boundary, not a live countdown.
- **Stale values:** a failed request can retain the last reading with an
  unavailable/stale status. Do not treat those values as a fresh provider reading.
- **After clear or reload:** lifecycle resets can remove the strip until the
  session initializes again. Last readings are not restored from a database.
- **Startup command warning:** historical Desktop checks sometimes displayed
  “isn't a command here” despite a successful initial command. Its cause remains
  unverified. Reloading plugins or invoking help after initialization can help;
  neither is a guaranteed fix.

Desktop animations include reduced-motion CSS. The terminal SDK exposes no
reduced-motion preference. Historical screenshots and synthetic previews do not
certify every light/dark, narrow-layout or accessibility combination.

## Update or remove

```sh
claude plugin marketplace update antiburn
claude plugin update antiburn@antiburn --scope user
```

Open a new session after updating. Use the permanent repository URL without a
branch fragment. To remove the user-scoped plugin while retaining host-managed
plugin data:

```sh
claude plugin uninstall antiburn@antiburn --scope user --keep-data
```

Reload or restart afterward. Removing the plugin does not erase sessions or
other data retained independently by Claude Code.

## Source, support and license

The public package is generated from an explicit allowlist. Contributors edit
`plugins/antiburn-live/release` in the source repository and regenerate
`plugins/antiburn`; do not edit the generated copy directly. The
[development guide](https://github.com/antiburn/antiburn/blob/main/plugins/antiburn-live/README.md)
covers local builds and checks. The local `antiburn-dev` variant and other agent
adapters are outside this public package.

Published by **Antiburn**, owned by **Cadence AI (Vic) Pty Ltd**. See
[antiburn.com/plugin](https://antiburn.com/plugin) for the current guide, the
[changelog](CHANGELOG.md) for versions, and
[GitHub issues](https://github.com/antiburn/antiburn/issues) for support. Include
host/plugin versions and steps to reproduce; never attach credentials or private
session content. For security or privacy concerns, follow the
[security reporting instructions](https://github.com/antiburn/antiburn/security/policy).

Source is covered by the included [MIT license](LICENSE). Bundled Bitcount
lettering retains its [SIL OFL notice](assets/bitcount/OFL.txt).
