# Privacy and data handling

antiburn 0.2.5 reads account quota data through the Claude Code host.
Native measurements take precedence, including partial readings. The mod uses
percentages, reset timestamps, measurement status, and the host clock to display
the usage strip and its details.

After Desktop attaches, an empty or unavailable native reading can trigger a GET
to `https://api.anthropic.com/api/oauth/usage` through the host HTTP API. The host
supplies an opaque bearer authorization handle and keeps the actual credential.
The request has no body; its explicit headers are
`anthropic-beta: oauth-2025-04-20` and `accept: application/json`. No session,
prompt, project, or account-usage content is sent by the mod. API-key and absent
authorization do not trigger this request. Host traffic policy can refuse it.

Fallback requests occur only on Desktop attachment, eligible refresh, and
lifecycle reconciliation. There is one concurrent request, 60 seconds of success
freshness, and event-driven failure backoff from 60 seconds to 15 minutes;
a longer valid HTTP 429 Retry-After is respected. Rendering and native
measurements do not fetch account data. There are no usage-fetch or retry timers,
polling, automatic retries, model calls, or prompt mutation. The terminal
Checking indicator uses a lifecycle-scoped 300 ms display timer only while it is
mounted. Desktop dots use CSS animation with reduced-motion handling; the
terminal SDK exposes no reduced-motion setting.

Display state stays in host session atoms. Fallback data and bookkeeping remain
in memory and are cleared on lifecycle resets and detach. The mod does not
persist account readings, create its own database, read transcripts, project
files, CLI caches or credentials, inspect the Keychain, or export account data.
The public artifact contains no activity observer, telemetry, updater, or
developer preview controls. It does not inherit the antiburn desktop app's
analytics or optional third-party checks. There is no app bridge or requirement
to install the cross-vendor core.

Claude Code controls its own API requests, session storage, settings, logs,
plugin cache, and retention. Removing this mod does not by itself erase data
retained by the host. The host's own privacy terms and account policies still
apply. Activating the logo opens the fixed https://antiburn.com address. Desktop uses
a native link. Terminal activation reads the OS name and invokes a bounded
system opener: `cmd.exe /c start "" https://antiburn.com` on Windows, or
`open https://antiburn.com` followed by `xdg-open https://antiburn.com` if needed
on other systems. Each attempt has a five-second timeout. The commands and their
arguments are fixed; network responses never supply program names or arguments.
The plugin displays the address if opening fails. Rendering and hover do
not launch a browser. Website visits use normal browser networking and send no
quota or session data through the mod. Documentation and support links also
open external websites.

The locally generated development variant can contain additional experimental
behavior and is not covered by the public artifact's capability description.
Do not use its behavior as evidence for this release's data handling.

For privacy questions, use the project's
[security reporting instructions](https://github.com/antiburn/antiburn/security/policy).
Do not attach credentials or private session content to public issues.

The public allowlist excludes antiburn executable discovery, CLI invocation,
CLI/app diagnostics, live session checks, fixes, tours and debug fixtures. No
setting or environment flag loads those development modules. Native usage/help/refresh
commands do not contact the app. Usage rendering and host quota reads start
automatically, without an install-time setting. Explicit usage commands can
also read quota through the host. The public artifact bundles no model-mediated
help skill; native commands do not request a model response.
