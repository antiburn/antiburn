# Anonymised analytics

antiburn sends anonymised events about **the application itself** — which
features get used, and what breaks. This document is the complete account of
that: every field, every event, what is deliberately excluded, what antiburn
cannot promise, and how to verify all of it yourself without trusting this
page.

The control is in **Settings → Privacy**, and the first-run Ready screen shows
it before anything is sent. It is on by default — except in the EU, the EEA,
and the UK, where it starts **off**, because there analytics are something you
opt into rather than out of. That is decided from the locale and time zone your
machine already reports; nothing is looked up, and neither is ever sent. That
default differs from the general default-off policy so consent is collected
before analytics starts in those regions.

## The short version

- Nothing derived from your work is ever sent — no transcript, prompt, title,
  file path, repository or branch name, token count, cost, or credential.
- Nothing is sent until the first run completes, so declining on the Ready
  screen means no event is ever recorded, rather than recorded and withdrawn.
- The installation identifier is random, is not derived from anything about
  your machine, and is replaced every 30 days.
- Turning the control off deletes the identifier and everything queued.
- A build with no endpoint configured sends nothing at all. That includes every
  development build and **every build made from a clean checkout of this
  repository** — the endpoint is injected at build time and is not in the tree.

## Exactly what every event carries

Thirteen fields, and this is the whole list. The payload is a closed Rust struct
([`analytics/event.rs`](../apps/desktop/src-tauri/src/analytics/event.rs))
with no map and no free-form string, so there is nowhere for anything else to
be put.

| Field                | What it is                                                                                                                                                 | Example                   |
| -------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------- |
| `platform`           | Constant. The surface class the collector partitions on.                                                                                                   | `desktop`                 |
| `messageId`          | Random per-event id, so a redelivered event is not counted twice.                                                                                          | `9f2c…`                   |
| `anonymousId`        | The rotating installation identifier.                                                                                                                      | `4b81…`                   |
| `sessionId`          | Identifies one run of the application. Held in memory only, never written to disk, replaced after 30 minutes of inactivity and whenever antiburn restarts. | `7d10…`                   |
| `event`              | The event name, from the closed catalog below.                                                                                                             | `antiburn.scan_completed` |
| `originalTimestamp`  | When it happened, UTC.                                                                                                                                     | `2026-08-19T09:14:02Z`    |
| `sentAt`             | When it was delivered. Added at send, not at capture.                                                                                                      | `2026-08-19T09:15:02Z`    |
| `properties.arch`    | CPU architecture.                                                                                                                                          | `aarch64`                 |
| `properties.bucket`  | A count rounded into a range. Never exact.                                                                                                                 | `10-49`                   |
| `properties.label`   | A key from a closed vocabulary — which setting changed, which agent's session was opened, or which kind of failure. Never the value.                       | `live_usage`              |
| `properties.detail`  | A second value from a closed vocabulary, where one event has two things worth telling apart.                                                               | `native`                  |
| `context.appVersion` | The application version.                                                                                                                                   | `antiburn:0.1.0`          |
| `context.os`         | Operating-system family.                                                                                                                                   | `macos`                   |

### What the two timestamps make possible

Each event is timestamped and the installation identifier lasts up to 30 days,
so these events show roughly **when** antiburn is used within that window. The
`sessionId` additionally groups the events of a single run together, so a run
can be seen as one visit rather than as scattered events. Neither can show what
antiburn was used _on_. This is stated because an enumeration that lists fields
without saying what they enable is not really an enumeration.

### Why there are two identifiers

The receiving server's contract requires both, and they are not equally
durable. `anonymousId` is stored on disk and lasts up to 30 days. `sessionId`
exists only in memory: quitting antiburn ends it, and nothing on your machine
remembers it afterwards. It is the shortest-lived thing in the payload, and it
cannot connect one run of the application to another.

### Which agents you use

`antiburn.session_opened` carries the agent that recorded the session you
opened — `claude-code`, `codex`, `cursor`, and so on, from the fixed list
antiburn knows how to read. Nothing else about the session travels with it: not
its title, not its repository, not its path, and not the name of your WSL
distribution, which you chose and which would identify your machine.

This is called out separately because it is the one field that says something
about your tools rather than about the application. If that is more than you
want to share, the switch turns all of it off.

### Why counts are bucketed

An exact count, reported repeatedly over weeks, identifies a machine on its own
even without an identifier attached. Buckets are `0`, `1-9`, `10-49`, `50-199`,
`200-999`, `1000+`.

## What is never sent

Sessions, transcripts, prompts, messages, tool activity, session titles, file
paths, repository or branch names, working directories, agent identities, token
counts, cost figures, credentials or tokens of any kind, your name, your email
address, your locale, and your hostname or username.

There is also **no third-party analytics, telemetry, or crash-reporting SDK** in
antiburn — no crash reporter, no session replay, no product-metrics vendor. The
channel described here is first-party and is the only one. Dependency policy,
the Tauri content security policy, and analytics behavior tests protect this
design.

## The event catalog

Event names are namespaced `antiburn.*`.

| Event                          | When it fires                                                                                                                                                                                               | Carries                                                                                                                                                                                                     |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `antiburn.app_launched`        | The application starts.                                                                                                                                                                                     | —                                                                                                                                                                                                           |
| `antiburn.onboarding_finished` | The first run completes.                                                                                                                                                                                    | —                                                                                                                                                                                                           |
| `antiburn.scan_completed`      | A discovery pass finishes **and finds a different number of sessions than the last one reported**. antiburn rescans about once a minute while the popover is open; a repeat of the same answer is not sent. | `bucket` — how many sessions                                                                                                                                                                                |
| `antiburn.setting_toggled`     | A preference changes.                                                                                                                                                                                       | `label` — one of `live_usage`, `notifications`, `launch_at_login`, `discovery_paused`. The key only; never the value.                                                                                       |
| `antiburn.session_opened`      | You open a session from the activity list.                                                                                                                                                                  | `label` — which agent recorded it, from the fixed list antiburn supports. `detail` — `native` or `wsl`. **Not** the session, its title, its repository, or the name of your WSL distribution.               |
| `antiburn.usage_viewed`        | You open the usage view.                                                                                                                                                                                    | `bucket` — how many providers had anything to show. `label` — `live` if any provider reported its own limit figures, `estimated_only` if there were only antiburn's estimates, `none` if there was nothing. |
| `antiburn.error_occurred`      | Something failed, and the previous pass had not already reported the same failure.                                                                                                                          | `label` — a category, currently `scan_failed`. No message, no path, no backtrace.                                                                                                                           |

Two of those are deliberately not sent once per occurrence. A scan result that
repeats the last one is dropped, so a machine left running does not report the
same number every minute and a machine stuck failing does not report the same
failure six hundred times a day. What survives is the first pass of each run,
every crossing of a bucket boundary, and every move into or out of failure.

That is the complete list of what this build sends. More events may be added
later, and the table is not a courtesy when they are: a test
(`the_documented_catalog_matches_the_code`) fails the build if an event exists
in the code and not in this table, so what you are reading is enforced rather
than maintained.

## What antiburn cannot promise

How long these events are kept, and whether the receiving server records the IP
address that every internet request carries, are decisions belonging to whoever
operates the endpoint — not to the application. antiburn can only promise what
it sends, which is the list above. Retention and IP handling belong in the
[privacy policy](privacy-policy.md), which is still a draft: it names each of
those open questions explicitly rather than leaving them unstated, and nothing
in the application links to it until they are answered.

This distinction is deliberate. A claim the client cannot keep is exactly the
kind of drift the [deviations register](deviations.md) exists to catch, so the
in-app copy and this document both stop at the boundary of what the code
guarantees.

## Verifying this yourself

Everything above is checkable on your own machine.

**Confirm a development build sends nothing.** Run `pnpm dev` from
`apps/desktop`. Settings → Privacy shows the control disabled, with the reason.
No endpoint was injected, so `analytics::config::configured()` is false
and nothing is queued or sent.

**Watch what a configured build actually sends.** Start a collector that prints
each request:

```bash
python3 -c "
from http.server import BaseHTTPRequestHandler, HTTPServer
class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get('content-length', 0))
        print(self.path, self.rfile.read(n).decode(), flush=True)
        self.send_response(200); self.end_headers()
HTTPServer(('127.0.0.1', 8787), H).serve_forever()"
```

Then build against it. Plain `http` is accepted only on loopback, which exists
for exactly this:

```bash
cd apps/desktop && ANTIBURN_ANALYTICS_URL=http://127.0.0.1:8787 pnpm tauri dev --features distribution --config src-tauri/tauri.debug.conf.json
```

The first delivery is a minute after launch, then every fifteen minutes.

**Read the queue on disk.** Nothing is hidden from you; the events wait in the
app's own database:

```bash
sqlite3 ~/Library/Application\ Support/ai.antiburn.desktop/antiburn-debug.sqlite3 "SELECT id, name, attempts, payload FROM analytics_event; SELECT install_id, minted_at FROM analytics_identity;"
```

**Confirm opting out is a withdrawal, not a pause.** Turn the control off in
Settings → Privacy, then re-run the query above. Both tables are empty: the
queue is discarded and the identifier destroyed, so a later opt-in starts an
identity that cannot be linked to the old one.

## Where this lives in the code

| Concern                                                   | File                                                                       |
| --------------------------------------------------------- | -------------------------------------------------------------------------- |
| Consent gate, queue, delivery                             | [`analytics/mod.rs`](../apps/desktop/src-tauri/src/analytics/mod.rs)       |
| The payload, and the closed field set                     | [`analytics/event.rs`](../apps/desktop/src-tauri/src/analytics/event.rs)   |
| Endpoint configuration, and why a clean checkout is inert | [`analytics/config.rs`](../apps/desktop/src-tauri/src/analytics/config.rs) |
| The setting, and the upgrade rule for existing installs   | [`store/mod.rs`](../apps/desktop/src-tauri/src/store/mod.rs)               |
| The reader-facing copy                                    | [`PrivacyPane.tsx`](../apps/desktop/src/views/settings/PrivacyPane.tsx)    |
