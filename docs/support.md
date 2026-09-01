# antiburn v1 support

What this version of antiburn actually supports, and what it stores. Anything not
listed here is not claimed — a cell that is absent means "not supported", not
"probably works".

## Platforms

| Platform | v1 support |
| --- | --- |
| macOS 13 or later (Apple silicon and Intel) | Supported |
| macOS 12 or earlier | Not supported; the bundle declares macOS 13 as its minimum |
| Windows 11 (x86-64) | Supported |
| Windows 10 | Not tested; no support claimed |
| Linux, mainstream x86-64 desktops with a system tray | Supported; it runs on the X11 backend there — through XWayland on a Wayland session — because it places its own popover and notification windows; a session with no X server leaves that placement to the compositor |
| Linux without a system tray (or an AppIndicator host) | Not supported — antiburn is a tray application |
| Mobile | Out of scope |

## Agents

antiburn reads session data that a coding agent has already written to disk.
Plan limits are separate: antiburn can ask a provider for those figures as
described in [Network](#network).

| Agent | Native (macOS / Windows / Linux) | WSL | Notes |
| --- | --- | --- | --- |
| Claude Code | Supported | Supported | |
| Codex | Supported | Supported | |
| OpenCode | Supported | Supported | Dedicated session analysis and Insights |
| Cursor | Supported | Not supported | |
| GitHub Copilot | Supported | Not supported | |
| Cline | Supported | Not supported | |
| Kiro | Supported | Not supported | |
| Amp | Supported | Not supported | |
| Pi | macOS and Linux only | Not supported | Pi v3 CLI sessions only, including `PI_AGENT_DIR`; dedicated analysis and Insights; excluded on native Windows and WSL; no Pi-specific live plan meter |
| Antigravity | Supported, **disk-only** | Not supported | Dedicated analysis for Antigravity brain JSONL and saved cascade files |
| Windsurf | Supported, **disk-only** | Not supported | Documented local files only |

**Disk-only** means sessions come from the agent's own documented local files; the
live language-server APIs those two editors expose aren't read, so a session that
exists only in memory will not appear.

**Session analysis** — the timeline, phases, context, token, and cost views — need a
transcript format antiburn understands in detail. Where it has only a generic parse,
the session is still listed and the analysis view says so rather than showing an
empty chart that looks like an idle session.

## Cost estimates

Costs are computed on this device from a bundled price list, against the tokens a
transcript recorded. They are **API-equivalent estimates**, not a bill:

- prices are never fetched; the bundled catalog's review date is shown in Settings →
  About;
- a model with no price in the catalog produces no figure rather than a wrong zero,
  and the provider's total is then labelled as a floor;
- work done on another machine is not counted, because antiburn cannot see it.

Provider Usage shows what was *spent* on this machine. It never shows a percentage,
an allowance, a remaining balance, or a reset time: a transcript records spend, and a
denominator would have to be invented.

**Plan limits are a separate thing, from a separate place.** antiburn asks each
provider directly and shows the figures that provider stated — a percentage of a
five-hour or weekly allowance, and when it resets. Those are the provider's numbers,
not ours:

- when the popover opens, antiburn shows its last successful reading immediately
  and asks for a current reading in the background;
- the current response replaces the saved reading. If antiburn has no saved reading,
  the limits section stays absent until the first response arrives;
- every reading shows when antiburn received it. A reading older than an hour is
  marked as stale rather than ageing quietly on screen;
- a figure the provider did not state is shown as unknown, never as zero;
- the limits appear above the spend estimates and never replace them. See
  [Network](#network) for the connections and the switch that controls them.

## What antiburn stores

antiburn keeps its own local data under the application's data directory. Settings →
About shows the exact path. It may retain the session content and derived data it
needs to provide visibility and analysis, including messages, tool activity, file
content recorded in a transcript, session identity and locations, counts, durations,
token totals, phase distributions, cost estimates, skill details, derived session
evidence — bounded facts about which models, tools, skills, and MCP servers a
session used, and any quota limits it recorded hitting, never the transcript's
text — session relations, and the last successful plan-limit reading. This data stays on the
device and is never uploaded.

The coding agents' source transcripts remain their files. antiburn may copy data from
them into its own local store, but it never modifies or deletes the source files.

**Session retention is configurable.** Settings → Privacy can keep antiburn's local
session data for 30 days, 90 days, or forever. Forever is the default and can preserve
history after providers' 30-day retention window. A shorter period keeps the local
index lighter. Deleting a transcript from disk does not immediately delete what
antiburn derived from it; that data follows the selected retention period unless the
session or local index is deleted first. The agents' own files are never touched.

**Deletion.** antiburn removes only records it created itself. It cannot and will not
delete a coding agent's own transcript — that is the agent's file, and removing a
conversation belongs in the agent's own interface.

**Exports** currently carry derived analysis, the paths a session ran in, its title,
and skill descriptions. They do not include transcript bodies. The export flow warns
before it writes and always asks where to put the file.

**Folder permissions (macOS).** macOS guards Documents, Desktop, and Downloads behind
your explicit consent. antiburn never reads one of them until you have allowed it: a
repository recorded in a guarded folder is skipped, and antiburn tells you it was
skipped rather than asking the system for access on its own. The permission dialog you
see is one antiburn asked for because you pressed a button, and what it wants is
narrow — the git repositories your coding agents worked in, read for their names and
locations. If you decline, the folder is simply left alone; you can change your mind
in Settings → Sources, or revoke access in System Settings, and antiburn will notice
the next time it looks.

## Network

antiburn needs no account or backend for its main work. The connections it makes
beyond analytics and updates are yours, not ours: reading
a provider's own figures with your own credentials is traffic between this machine
and a provider you already use. The application's own connection to a service of
ours are analytics and the updater. The updater asks GitHub Releases whether a newer
version exists. When automatic updates are enabled, it downloads and installs the
signed bundle and restarts antiburn. The app never depends on either connection.

- The check sends nothing about you, your machine, or your sessions.
- It runs on a schedule only while "Install updates automatically" is on, and can
  always be run by hand from Settings → About.
- An automatic update downloads, verifies, and installs the new version. antiburn
  restarts as soon as installation succeeds.
- An install verifies the downloaded bundle against the public key in the app before
  it changes the installed application.
- Development builds carry no updater at all.
- Linux AppImage releases update in the app. Debian packages remain install-only
  and require the next package to be installed manually.
- **Anonymised analytics** are the one thing antiburn reports about itself.
  Official release builds start with it on, including during onboarding. The Ready
  screen explains it, and the switch is in Settings → Privacy. Each event carries thirteen fields and
  no others: the constant `desktop`; a random per-message id used to discard
  duplicate deliveries; a random installation identifier replaced every 30 days;
  the event name; the time it happened and the time it was delivered; the
  processor architecture; a count rounded into a range where the event has one;
  a short label naming which setting changed or which kind of failure occurred,
  never the value; the app version; and the operating system. The payload has no
  field able to carry anything else. Because each event is timestamped and the
  identifier lasts up to 30 days, the events do show roughly when the
  application is used within that window; they do not show what it was used on.
  [analytics.md](analytics.md) is the complete account: every field,
  the full event catalog, and how to verify all of it yourself.
  Never sent: sessions, transcripts, prompts, titles, file paths, repository or
  branch names, token counts, costs, or credentials. Switching it off deletes
  the identifier and anything still queued. The endpoint also stores the request
  IP address and user-agent. Raw events are retained until the operator deletes
  them. Default source and development builds exclude the analytics client.
- There is **no third-party analytics, telemetry, or crash-reporting SDK** in this
  application. The channel above is first-party and is the only one.

**One setting makes antiburn go online as you.** Settings → Usage has a switch,
on by default once first-run setup is complete, that lets antiburn ask each
provider directly for your current plan usage — about every ten minutes — using
the credential your coding tool already keeps on this machine (for example, the
Claude CLI's own OAuth credential, or the Codex CLI's own). It runs by default
because this is your own traffic: your usage, from a provider you already use,
with a credential you already hold, over your own connection — no antiburn
server sees the request or the response. When a provider's endpoint cannot be
reached directly, antiburn falls back to asking your coding tool's own local
process the same question, over its own protocol, rather than leaving the
reading blank. Turn the switch off if you want none of this — no background
traffic at all, whatever the reason — and antiburn stops asking, reads no
credential, and has no plan limits to show.

**Notifications are local.** antiburn shows them in its own small notification
window and posts exactly these: an update check that found a newer version, the
first scan failure of a run, free disk space dropping below your threshold, a
usage milestone, the first-run menu-bar location, and the test button's own
sample. Milestones need readings that keep moving, so they fire only while
Settings → Usage is set to refresh; with that off they stay silent. By default,
they fire at every 10% of a limit and compare quota used with time elapsed in
that limit's window. Settings → Notifications offers every 5% step, plus
select-all and clear-all controls. antiburn constructs each notification on this
machine, and nothing about it leaves the machine. The test and first-run
location ignore the master switch because both follow a direct action.

**Notifications respect Focus and Do Not Disturb.** Immediately before an
automated notification appears, antiburn asks your operating system whether
interruptions are welcome — Focus and Do Not Disturb on macOS, fullscreen and
presentation states on Windows, Do Not Disturb on GNOME and KDE Plasma. A
suppressed notification is dropped, not saved: turning Focus off never releases
a backlog of stale alerts. The test button still works during Focus, because
you pressed it. On macOS this needs your permission once — antiburn asks after
setup, reads only whether Focus is on, and nothing about it leaves this Mac; if
you decline, notifications simply stop yielding to Focus. If antiburn is in a
Focus's Allowed Apps list, macOS lets its notifications through, and antiburn
follows that answer. When the system cannot say either way, antiburn delivers
rather than staying silent.

## Reporting a gap

If an agent on this list is not discovered on a supported platform, that is a bug —
please open an issue with the agent, its version, your platform, and where its
session files live. Security issues go to the private channel in
[`SECURITY.md`](../SECURITY.md) instead.
