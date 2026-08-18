<!-- This Source Code Form is subject to the terms of the Mozilla Public
     License, v. 2.0. If a copy of the MPL was not distributed with this
     file, You can obtain one at https://mozilla.org/MPL/2.0/. -->

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
| Linux, mainstream x86-64 desktops with a system tray | Supported |
| Linux without a system tray (or an AppIndicator host) | Not supported — antiburn is a tray application |
| Mobile | Out of scope |

## Agents

antiburn reads what a coding agent has already written to disk. One opt-in
setting *runs* an agent — see [Network](#network) — but even then antiburn
reads the file the agent writes, not the agent's output.

| Agent | Native (macOS / Windows / Linux) | WSL | Notes |
| --- | --- | --- | --- |
| Claude Code | Supported | Supported | |
| Codex | Supported | Supported | |
| OpenCode | Supported | Supported | |
| Cursor | Supported | Not supported | |
| GitHub Copilot | Supported | Not supported | |
| Cline | Supported | Not supported | |
| Kiro | Supported | Not supported | |
| Amp | Supported | Not supported | |
| Pi | macOS and Linux only | Not supported | Excluded on Windows |
| Antigravity | Supported, **disk-only** | Not supported | Documented local files only |
| Windsurf | Supported, **disk-only** | Not supported | Documented local files only |

**Disk-only** means sessions come from the agent's own documented local files; the
live language-server APIs those two editors expose aren't read, so a session that
exists only in memory will not appear.

**Session analytics** — the timeline, phases, context, token, and cost views — need a
transcript format antiburn understands in detail. Where it has only a generic parse,
the session is still listed and the analytics view says so rather than showing an
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

**Plan limits are a separate thing, from a separate place.** When one of your agents
has cached its own usage reading on this machine, antiburn reads that file and shows
the figures your provider stated — a percentage of a five-hour or weekly allowance,
and when it resets. Those are the provider's numbers, not ours:

- antiburn itself calls no provider to get them: by default it simply reads the
  file the agent already wrote the last time *it* was online, the same way it
  reads everything else. One opt-in setting can ask the agent to refresh that
  file on antiburn's behalf — see [Network](#network);
- every reading is shown with the moment the provider stated it, and a reading older
  than an hour is marked as such rather than ageing quietly on screen;
- a figure the provider did not state is shown as unknown, never as zero;
- they appear above the spend estimates and never replace them. If no agent has
  cached a reading, the limits section is simply absent and the spend estimates are
  unchanged.

## What antiburn stores

antiburn keeps its own local data under the application's data directory. Settings →
About shows the exact path. It may retain the session content and derived data it
needs to provide visibility and analysis, including messages, tool activity, file
content recorded in a transcript, session identity and locations, counts, durations,
token totals, phase distributions, cost estimates, skill details, and session
relations. This data stays on the device and is never uploaded.

The coding agents' source transcripts remain their files. antiburn may copy data from
them into its own local store, but it never modifies or deletes the source files.

**There is no age-based retention limit.** Once a session is indexed, antiburn keeps
its local data until the reader explicitly clears it. The agents' own files are never
touched. Settings → Privacy clears all locally stored session data at once.

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

antiburn needs no connection to any service of ours — no antiburn account, server,
or backend, ever. The connections it makes beyond that are yours, not ours: reading
a provider's own figures with your own credentials is traffic between this machine
and a provider you already use. The application's own connection to a service of
ours is exactly one kind: the updater, which asks GitHub Releases whether a newer
version exists, and which the app never depends on.

- The check sends nothing about you, your machine, or your sessions.
- It runs on a schedule only while "check for updates automatically" is on, and can
  always be run by hand from Settings → About.
- Development builds carry no updater at all.
- There is **no analytics and no telemetry** in this application — no client, no
  consent screen, and no endpoint.

**One setting makes antiburn go online as you.** Settings → Usage has a switch,
off by default, that lets antiburn ask each provider directly for your current
plan usage — about every ten minutes — using the credential your coding tool
already keeps on this machine (for example, the Claude CLI's own OAuth
credential, or the Codex CLI's own). The traffic goes only between this machine
and a provider you already use; no antiburn server sees the request or the
response. When a provider's endpoint cannot be reached directly,
antiburn falls back to asking your coding tool's own local process the same
question, over its own protocol, rather than leaving the reading blank. With the
switch off, none of this runs, no credential is read, and antiburn has no plan
limits to show.

**Notifications are local.** antiburn shows them in its own small notification
window and posts exactly these: an update check that found a newer version, the
first scan failure of a run, free disk space dropping below your threshold, an
hour of unusually fast estimated spend, a usage milestone, and the test button's
own sample. Milestones need readings that keep moving, so they fire only while
Settings → Usage is set to refresh; with that off they stay silent. Every
figure in them is computed on this machine; nothing about a notification leaves
it. All of them can be turned off in Settings → Notifications, together or one at
a time — the test alone ignores the master switch, so you can preview a
notification before allowing any.

## Reporting a gap

If an agent on this list is not discovered on a supported platform, that is a bug —
please open an issue with the agent, its version, your platform, and where its
session files live. Security issues go to the private channel in
[`SECURITY.md`](../SECURITY.md) instead.
