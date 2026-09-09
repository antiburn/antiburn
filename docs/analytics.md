# Anonymised analytics

antiburn sends anonymised product events — which features get used, what
breaks, a coarse diagnostic of Claude's session-limit reset, how close a
connected provider's own usage windows run to their limit, and coarse hourly
ranges for antiburn's own resource use. This document is
the complete account of that: every field, every event, what is deliberately
excluded, what antiburn cannot promise, and how to verify all of it yourself
without trusting this page.

Official release builds start with analytics on. This includes the launch and
fixed onboarding-step events. The first-run Ready screen explains the channel,
and the permanent opt-out is in **Settings → Privacy**. Set
`ANTIBURN_ANALYTICS_ENABLED=false` in the app's launch environment for a second,
process-level opt-out.

## The short version

- No work content is ever sent — no transcript, prompt, title, file path,
  repository or branch name, token count, cost, or credential. Resource ranges
  can reveal coarse app work intensity and local data volume.
- Official builds can record launch and onboarding progress before setup ends.
- The installation identifier is random, is not derived from anything about
  your machine, and is replaced every 30 days.
- Turning the control off deletes the identifier and everything queued.
- A build with no endpoint configured sends nothing at all. That includes every
  development build and **every build made from a clean checkout of this
  repository** — the endpoint is injected at build time and is not in the tree.

## Exactly what the event schema can carry

Twenty-seven fields, and this is the whole list. Fourteen are the established
event envelope and general properties. The nine Claude reset fields are optional
and appear only on `antiburn.claude_limit_reset_observed`. The three limit-factor
fields are optional and appear only on `antiburn.limit_factor_observed`. One
nested resource summary appears only on `antiburn.resource_usage_observed`. The
payload is a closed Rust struct
([`analytics/event.rs`](../apps/desktop/src-tauri/src/analytics/event.rs))
with no map and no free-form string, so there is nowhere for anything else to
be put.

| Field                           | What it is                                                                                                                                                                                                                                                                      | Example                                 |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------- |
| `platform`                      | Constant. The surface class the collector partitions on.                                                                                                                                                                                                                        | `desktop`                               |
| `messageId`                     | Random per-event id, so a redelivered event is not counted twice.                                                                                                                                                                                                               | `9f2c…`                                 |
| `anonymousId`                   | The rotating installation identifier.                                                                                                                                                                                                                                           | `4b81…`                                 |
| `sessionId`                     | Groups a window of captured analytics events. Its generator is held in memory, but the value is written into each event queued on disk. Replaced after 30 minutes without a captured analytics event, whenever antiburn restarts, and when the installation identifier rotates. | `7d10…`                                 |
| `event`                         | The event name, from the closed catalog below.                                                                                                                                                                                                                                  | `antiburn.scan_completed`               |
| `originalTimestamp`             | When it happened, UTC.                                                                                                                                                                                                                                                          | `2026-08-19T09:14:02Z`                  |
| `sentAt`                        | When it was delivered. Added at send, not at capture.                                                                                                                                                                                                                           | `2026-08-19T09:15:02Z`                  |
| `properties.arch`               | CPU architecture.                                                                                                                                                                                                                                                               | `aarch64`                               |
| `properties.bucket`             | A count rounded into a range. Never exact.                                                                                                                                                                                                                                      | `10-49`                                 |
| `properties.label`              | A key from a closed vocabulary — which surface, Settings pane, provider, setting, agent category, or failure category. Never work content or a user-selected value.                                                                                                             | `activity`                              |
| `properties.detail`             | A second value from a closed vocabulary, such as a visible state, `user` or `automatic` origin, `native` versus `wsl`, or a usage window's `short` or `long` role.                                                                                                              | `ready`                                 |
| `properties.origin`             | Whether a state appeared after a `user` or `automatic` exposure. Optional and present only on `antiburn.surface_state_observed`.                                                                                                                                                | `user`                                  |
| `properties.usageBand`          | How close a usage window runs to its limit: `below_80`, `80_to_under_100`, `at_limit`, or `unknown`. On the Claude reset event, Claude's five-hour usage from the same reset-check response; on `antiburn.usage_observed`, the window named by `detail`.                        | `80_to_under_100`                       |
| `properties.responseShape`      | Whether `juniper_tide` was an `object`, `missing`, `null`, or `malformed`; also `invalid_json`, `malformed_envelope`, `not_received`, `unreadable`, or `not_requested` for request and envelope outcomes.                                                                       | `object`                                |
| `properties.eligibility`        | Claude's `eligible` boolean as `eligible` or `ineligible`, or `missing`, `null`, or `malformed`.                                                                                                                                                                                | `eligible`                              |
| `properties.ineligibleReason`   | An allowlisted Claude reason: `tier`, `tenure`, `surface`, `mobile`, `cli_version`, `not_at_wall`, `weekly_limit`, `no_weekly_limit`, `other_experiment`, `extra_usage`, `unavailable`, `unknown`, or `other`; also `missing`, `null`, or `malformed`.                          | `not_at_wall`                           |
| `properties.experiment`         | Claude's `in_experiment` boolean as `in_experiment` or `not_in_experiment`, or `missing`, `null`, or `malformed`.                                                                                                                                                               | `in_experiment`                         |
| `properties.resetArm`           | Claude's experiment arm as `reset`, `control`, or `other`, or `missing`, `null`, or `malformed`.                                                                                                                                                                                | `reset`                                 |
| `properties.resetAvailability`  | Claude's `available` boolean as `available` or `unavailable`, or `missing`, `null`, or `malformed`.                                                                                                                                                                             | `available`                             |
| `properties.resetsPerWeek`      | Claude's reset count as `0`, `1`, or `2_plus`, or `missing`, `null`, or `malformed`.                                                                                                                                                                                            | `1`                                     |
| `properties.nextResetAvailable` | Whether `next_available_at` was `present`, `missing`, `null`, or `malformed`. The timestamp itself is never sent.                                                                                                                                                               | `present`                               |
| `properties.plan`               | A learned limit factor's plan, mapped to `free`, `pro`, `max`, `team`, `enterprise`, `plus`, `business`, `edu`, `unknown` (none reported), or `other` (anything else). Never the provider's raw plan string. Optional and present only on `antiburn.limit_factor_observed`.     | `max`                                   |
| `properties.factorBand`         | A learned limit factor's dollars-per-percent value, banded by powers of two: `under_1`, `1_to_under_2`, `2_to_under_4`, `4_to_under_8`, `8_to_under_16`, `16_to_under_32`, `32_to_under_64`, `64_to_under_128`, or `128_and_over`. Optional and present only on `antiburn.limit_factor_observed`. | `4_to_under_8`                          |
| `properties.residualBand`       | How far the meter and the factor's own estimate for the current period disagree: `within_5`, `within_20`, `over_20`, or `unknown` (no residual yet). Optional and present only on `antiburn.limit_factor_observed`.                                                            | `within_5`                              |
| `properties.resourceUsage`      | A closed hourly summary of antiburn's own process CPU, memory, read/write I/O, and logical database and WAL file sizes. Every measurement is a fixed band; coverage is `none`, `partial`, or `full`. The complete nested shape and bands are below.                             | `{ "cpuAverage": "under1_percent", … }` |
| `context.appVersion`            | The application version.                                                                                                                                                                                                                                                        | `antiburn:0.1.0`                        |
| `context.os`                    | Operating-system family.                                                                                                                                                                                                                                                        | `macos`                                 |

### What the two timestamps make possible

Each event is timestamped and the installation identifier lasts up to 30 days,
so these events show roughly **when events were captured** within that window.
The `sessionId` groups captured events separated by less than 30 minutes of
analytics-event inactivity. Background events can keep it active, and a quiet
app process can receive more than one, so it does not define a user visit or
time spent. None of these fields can show the content or identity of what
antiburn was used _on_. Resource ranges can indicate coarse work intensity and
data volume. This is stated because an enumeration that lists fields without
saying what they enable is not really an enumeration.

### Why there are two identifiers

The receiving server's contract requires both, and they are not equally
durable. `anonymousId` is stored on disk and lasts up to 30 days. The live
`sessionId` generator exists only in memory. Each serialized event also contains
its `sessionId`, so a queued event keeps that value on disk until it is sent or
removed. Restarting antiburn or rotating the installation identifier creates a
new value for newly captured events. Events already in the queue keep their
original value.

### Agent and provider categories

`antiburn.session_opened` carries the agent that recorded the session you
opened — `claude-code`, `codex`, `cursor`, and so on, from the fixed list
antiburn knows how to read. Nothing else about the session travels with it: not
its title, not its repository, not its path, and not the name of your WSL
distribution, which you chose and which would identify your machine.

`antiburn.live_usage_state_observed`, `antiburn.usage_observed`, and
`antiburn.limit_factor_observed` each carry one of three provider categories:
`anthropic`, `openai`, or `google`. The Claude reset event reveals that Claude
is enabled; `usage_observed` and `limit_factor_observed` reveal more broadly
which of the three providers are enabled and visible, since each fires for
whichever ones an ordinary pass produces a reading for. These are the only
analytics fields that identify an agent or provider category. If that is more
than you want to share, the switch turns all analytics off.

### Why counts are bucketed

An exact count, reported repeatedly over weeks, identifies a machine on its own
even without an identifier attached. Buckets are `0`, `1-9`, `10-49`, `50-199`,
`200-999`, `1000+`.

### Resource summary shape and platform meaning

`properties.resourceUsage` has exactly thirteen nested fields:

| Nested field                        | Meaning                                                                                                                                                      |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `memoryMean`, `memoryMax`           | Mean and highest sampled memory band in the reporting window. The maximum is a sampled maximum, not the process's true peak between samples.                 |
| `memoryCoverage`                    | Coverage of the memory samples.                                                                                                                              |
| `cpuAverage`, `cpuCoverage`         | Elapsed-time-weighted CPU average across valid sampled intervals and its coverage. One core is 100%, so the value can exceed 100%.                           |
| `readRateAverage`, `readCoverage`   | Elapsed-time-weighted process-read rate across independently valid intervals and its coverage.                                                               |
| `writeRateAverage`, `writeCoverage` | Elapsed-time-weighted process-write rate across independently valid intervals and its coverage.                                                              |
| `databaseSize`, `databaseCoverage`  | Latest successful logical size band for antiburn's own SQLite database in the window and its coverage.                                                       |
| `walSize`, `walCoverage`            | Latest successful logical size band for that database's write-ahead log in the window and its coverage. An absent WAL is a successful zero-size observation. |

Memory and file-size bands are `unavailable`, `under50_mib`,
`from50_to_under100_mib`, `from100_to_under250_mib`,
`from250_to_under500_mib`, `from500_to_under1024_mib`,
`from1_to_under2_gib`, `from2_to_under4_gib`, or `at_least4_gib`. CPU bands
are `unavailable`, `under1_percent`, `from1_to_under5_percent`,
`from5_to_under10_percent`, `from10_to_under25_percent`,
`from25_to_under50_percent`, `from50_to_under100_percent`,
`from100_to_under200_percent`, or `at_least200_percent`. Read and write rate
bands are `unavailable`, `zero`, `under1_kib_per_second`,
`from1_to_under10_kib_per_second`, `from10_to_under100_kib_per_second`,
`from100_to_under1024_kib_per_second`, `from1_to_under10_mib_per_second`, or
`at_least10_mib_per_second`.

On macOS, memory means physical footprint. On Linux, it means resident set size.
On Windows, it means working set size. CPU and memory describe the antiburn shell
process, not renderer processes or whole-app totals. CPU is the shell process's
user plus system CPU time on all three systems. macOS I/O counters cover physical
storage bytes attributed to the shell process. Linux `/proc/self/io` counters
cover physical storage bytes attributed to the shell and can also include I/O
inherited from child processes after the shell waits for them. The Linux kernel
implements the proc value in
[`fs/proc/base.c`](https://github.com/torvalds/linux/blob/master/fs/proc/base.c)
and adds a waited-for child's accounting in
[`kernel/exit.c`](https://github.com/torvalds/linux/blob/master/kernel/exit.c).
Windows counters cover all shell-process I/O transfer bytes, which can include
non-disk I/O; they do not measure physical disk traffic. None of these are
whole-machine measurements. The event does not enumerate renderer processes or
report a renderer count.

`unavailable` and coverage `none` mean that the measurement was not available;
they do not mean zero. Coverage `partial` means that only some sampling
opportunities produced a usable measurement. Coverage `full` means all attempted
samples or intervals succeeded, not that every instant was measured or that no
scheduled tick was missed. A first counter sample establishes a baseline only. Counter resets, zero or
greater-than-ten-minute intervals, and unavailable counters do not create zero
readings. Surviving intervals and opted-in installations can differ
systematically from missing intervals and people who turn analytics off, so
the summaries are not an unbiased picture of every antiburn installation.

The runtime bounds measurement overhead structurally: one awaited blocking
worker collects process counters and two file sizes, runs do not overlap, and
missed five-minute ticks are skipped. Timing, cancellation fencing, and
database/WAL file behavior have automated tests. Two ignored local timing tests
measure native counters alone and counters with synthetic database/WAL metadata
reads. One debug-test run on a developer Mac measured 1,000 full observations in
3.625 ms total, or about 3.625 microseconds each. This repeated local-file test
excludes blocking-worker scheduling, aggregation, queueing, and normal app
contention. It is not a CI performance budget or a percentage-overhead guarantee
for other hardware, filesystems, or operating systems. The cadence and
single-flight rules are the cross-platform operational bound.

## What is never sent

Session content, transcripts, prompts, messages, tool activity, session titles, file
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

| Event                                    | When it fires                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | Carries                                                                                                                                                                                                                                                                                                                                                                                                      |
| ---------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `antiburn.app_launched`                  | The application starts.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                      | —                                                                                                                                                                                                                                                                                                                                                                                                            |
| `antiburn.onboarding_started`            | A new, restarted, or resumed incomplete setup flow first becomes visible in an app process. A quit and later resume can report another start.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | `label` — `new` or `restart`, preserved when an incomplete flow resumes.                                                                                                                                                                                                                                                                                                                                     |
| `antiburn.onboarding_finished`           | A pending setup flow commits its settings and completes. Reported once per pending-to-complete transition.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | `label` — `new` or `restart`. Earlier app versions sent no label; a missing label must not be treated as `new`.                                                                                                                                                                                                                                                                                              |
| `antiburn.onboarding_step_viewed`        | A fixed setup step becomes visible. Each step is recorded at most once per onboarding flow.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                  | `label` — one of `welcome`, `agents_detected`, `sources_and_repos`, `ready`.                                                                                                                                                                                                                                                                                                                                 |
| `antiburn.scan_completed`                | A full discovery pass finishes and its session-count bucket differs from the previous reported outcome. The first full pass in a run is reported. Automatic full reconciliation runs at startup and about every five minutes; file watchers usually cause scoped passes, which emit no scan analytics.                                                                                                                                                                                                                                                                                                                       | `bucket` — how many sessions                                                                                                                                                                                                                                                                                                                                                                                 |
| `antiburn.setting_toggled`               | A preference changes.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | `label` — one of `live_usage`, `notifications`, `launch_at_login`, `discovery_paused`. The key only; never the value.                                                                                                                                                                                                                                                                                        |
| `antiburn.session_opened`                | You open a session from the activity list.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | `label` — which agent recorded it, from the fixed list antiburn supports. `detail` — `native` or `wsl`. **Not** the session, its title, its repository, or the name of your WSL distribution.                                                                                                                                                                                                                |
| `antiburn.surface_viewed`                | A native product surface becomes visible after a successful show or navigation transition. Prewarming, refreshes, remounts, resizes, and repeated show requests emit nothing.                                                                                                                                                                                                                                                                                                                                                                                                                                                | `label` — `activity`, `session_detail`, `provider_preview`, `checks_preview`, `hud`, `hud_detail`, or `settings`. `detail` — `user` or `automatic`.                                                                                                                                                                                                                                                          |
| `antiburn.settings_pane_viewed`          | A Settings pane is selected and visible. Repeating the current pane emits nothing. A direct pane link reports only the resulting pane.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | `label` — `general`, `appearance`, `sources`, `privacy`, `notifications`, `usage`, `insights`, or `about`.                                                                                                                                                                                                                                                                                                   |
| `antiburn.surface_state_observed`        | A `ready`, `empty`, `error`, or ten-second visible `loading_timeout` state is presented on a visible surface. Each distinct state is reported at most once per exposure; a later `ready` state can follow a timeout.                                                                                                                                                                                                                                                                                                                                                                                                         | `label` — the `surface_viewed` surface list plus `insights`. `detail` — `ready`, `empty`, `error`, or `loading_timeout`. `origin` — `user` or `automatic`.                                                                                                                                                                                                                                                   |
| `antiburn.live_usage_state_observed`     | A provider state is presented on Activity, in a provider preview, or in a user-opened HUD. Automatic HUD restoration emits nothing.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | `label` — `anthropic`, `openai`, or `google`. `detail` — `fresh`, `stale`, `authentication`, `rate_limited`, `unavailable`, or `no_credentials`. The last value is reserved but no current call site emits it.                                                                                                                                                                                               |
| `antiburn.error_occurred`                | A full discovery pass fails, and the previous full pass had not already reported the same failure.                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                           | `label` — a category, currently `scan_failed`. No message, no path, no backtrace.                                                                                                                                                                                                                                                                                                                            |
| `antiburn.unrecognized_records_observed` | Settings → Insights returns a cohort containing unknown record vocabulary, and its outcome differs from the last one reported during this run.                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | `bucket` — sessions containing unknown types. `label` — `inert_only`, `inert_capped`, or `evidence_bearing`. No discriminator, payload, session identifier, or second dimension.                                                                                                                                                                                                                             |
| `antiburn.claude_limit_reset_observed`   | After an ordinary Claude usage refresh, when analytics and Claude live usage are both enabled and the observation differs from the last one queued during this run. The probe uses a separate five-minute cooldown.                                                                                                                                                                                                                                                                                                                                                                                                          | `label` — `success`, `authentication`, `rateLimited`, `unavailable`, `credential_absent`, `credential_expired`, or `credential_unavailable`. The nine reset fields listed above. No response body, credential, account identifier, exact usage percentage, or date.                                                                                                                                          |
| `antiburn.usage_observed`                | An ordinary live-usage refresh — a background tick or a visible one — publishes a usage window for a provider that is online and not hidden. Reports the first observation for each `(provider, window role)` pair in a run, then only when that pair's band changes. A provider that fails this pass has no window to report and its last reported band is left alone. Reveals which providers are enabled, at worst a handful of times per install per day.                                                                                                                                                                | `label` — `anthropic`, `openai`, or `google`, the same vocabulary `live_usage_state_observed` uses. `detail` — `short` or `long`, the window's role. `usageBand` — `below_80`, `80_to_under_100`, `at_limit`, or `unknown`. A window without an authoritative figure still reports; the band is coarse enough to stay useful either way. No percentage, timestamp, window id, scope, account, or model name. |
| `antiburn.limit_factor_observed` | A background factor-learning pass produces a learned dollars-per-percent factor for a `(provider, lane)` pair. Reports the first observation for each pair in this run, then only when its `(plan, factor band, residual band)` tuple changes, and never more than once every 24 hours for the same pair even across a genuine change. Reveals a coarse sense of plan mix and estimate accuracy per provider and lane, at worst a handful of times per install per day. | `label` — `anthropic`, `openai`, or `google`. `detail` — `short` or `long`, the lane. `plan` — the mapped plan vocabulary. `factorBand` — the log-spaced dollars-per-percent band. `residualBand` — how far the meter and the estimate disagree. No dollar amount, percentage, account, or timestamp. |
| `antiburn.resource_usage_observed`       | The first sample occurs when resource analytics starts, followed by a five-minute cadence. Missed ticks are skipped, and reads never overlap or catch up. A delayed read can be followed soon by the next scheduled tick. The first sample at or after one monotonic hour closes the reporting window, which can be longer after suspension or a delayed tick. At most 24 summaries occur in 24 continuously running hours. Disabling analytics immediately resets the window and baseline, and late work cannot report. There is no exit flush. This background health event does not establish a user visit or engagement. | `resourceUsage` — the closed thirteen-field summary above. No renderer count, whole-machine information, work content, path, credential, exact byte count, or exact percentage. The summary enters the existing bounded analytics queue; it creates no separate request path.                                                                                                                                |

Several events are deliberately not sent once per occurrence. A full scan result
that repeats the last bucket is dropped, so a machine left running does not
report the same range after every reconciliation and a machine stuck failing
does not report the same failure after every full pass. What survives is the
first full pass of each run, every crossing of a bucket boundary, and every move
into or out of failure. Scoped watcher passes emit neither scan event and do not
change this comparison. The unrecognized-record event likewise reports only a
changed `(label, bucket)` outcome. A clean cohort updates that in-memory
comparison without sending an event, so a later return to unknown vocabulary is
visible.

Visible-use events start only after native visibility succeeds. A surface state
is reported at most once per distinct state in one exposure, and hidden or stale
asynchronous results emit nothing. The ten-second loading timeout runs only
while the surface is visible. Settings and its selected pane establish the
Insights exposure, so `settings_pane_viewed` with `insights` qualifies as
deliberate core use and `surface_state_observed` records its result. Insights
emits no separate `surface_viewed` event. Other Settings-only visits remain
outside the return-use denominator.

Provider states are reported only on Activity and for deliberate provider
previews and user-opened HUD exposures. The renderer suppresses duplicate
provider states in an exposure, and the shell suppresses the same provider and
state within a bounded 30-minute deliberate visit.
`no_credentials` is part of the closed schema but remains dormant until a
product boundary can prove that state.

The Claude limit-reset event reports the first observation in a run and then
only a changed observation. It calls the same provider usage endpoint through a
separate `at_wall=1&skip_spend=1` GET after the ordinary usage result has already
been published. It runs only when analytics is configured and enabled, live
usage is active, and Claude is visible. A 429 from either Claude usage request
defers the probe; the probe also honors a numeric `Retry-After` value. The app
does not invoke the reset operation. The diagnostic response can
therefore affect only this analytics event, never the displayed usage result,
notifications, or provider cache.

The event's usage band comes from the diagnostic response itself. That keeps an
80–99% or at-limit observation aligned with the reset fields it describes. The
event preserves missing, null, and malformed field states, and keeps eligibility,
experiment membership, arm, and availability separate so contradictory server
states remain visible. It sends only the presence of `next_available_at`, because
the exact date adds little diagnostic value and reveals more timing detail.

`antiburn.usage_observed` answers a narrower, provider-agnostic question the
Claude reset diagnostic cannot: how often installs run near or at a limit, per
provider and per window, across every provider antiburn can meter — not only
Claude. It fires wherever the Claude reset probe's own gates already fire —
analytics configured and enabled, live usage active, the provider not
hidden — but at the boundary every ordinary refresh publishes its snapshot
from, background ticks and popover-triggered refreshes alike. Only a
provider's short (five-hour or session) and long (weekly or billing-period)
windows are in scope; a supplemental or provider-specific window is not part
of this coarse picture. A provider with both windows reports both, once each,
the first time they are seen in a run and again only when a window's band
changes; a provider that fails a pass contributes nothing and does not clear
what was last reported for it, so a transient failure cannot be mistaken for a
drop back below the limit. Windows the provider marked non-authoritative —
derived rather than stated — still report, because a coarse band tolerates
that imprecision better than the exact percentage the Usage surface shows.

`antiburn.limit_factor_observed` reports on the background learning pass that
turns meter readings and priced turns into the dollars-per-percent factor the
session list's badge uses (see
[`docs/plans/limit-factor-estimation.md`](plans/limit-factor-estimation.md)).
It fires once per `(provider, lane)` pair the pass touched, mapping the
provider's own plan string through the closed vocabulary above rather than
sending it verbatim, and reducing the factor and its residual to bands. A
provider account is never named: an install with more than one account on the
same provider and lane reports only whichever account the pass reaches first,
so the event answers "what plans and accuracy bands exist across installs",
never "which account". The 24-hour floor between events for the same pair
holds even across a genuine band change, so a factor bouncing between two
adjacent bands cannot report more than once a day.

The `inert_capped` label covers either too many distinct unknown types or one
type name that exceeds the local string limit. The event never sends those
names. They are runtime schema vocabulary, while analytics properties use a
closed vocabulary reviewed in this file. New names remain visible in the local
Insights coverage note and diagnostics only.

This event is sampled only when a reader opens Settings → Insights. A request
can also join a report reduction already in flight. Its buckets are therefore
not a population rate and must not be interpreted as one.

That is the complete list of what this build sends. More events may be added
later, and the table is not a courtesy when they are: a test
(`the_documented_catalog_matches_the_code`) fails the build if an event exists
in the code and not in this table, so what you are reading is enforced rather
than maintained.

## What the endpoint also records

The first-party endpoint stores the request IP address and user-agent with each
raw event. Raw events have no automatic deletion schedule and are retained until
the operator deletes them. See the [privacy policy](privacy-policy.md).

## Verifying this yourself

Everything above is checkable on your own machine.

**Confirm a default development build sends nothing.** Run `pnpm dev` from
`apps/desktop`. The analytics client is excluded from the build, so no analytics
UI appears and nothing is queued or sent.

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
cd apps/desktop
ANTIBURN_ANALYTICS_URL=http://127.0.0.1:8787 \
ANTIBURN_ANALYTICS_OPERATOR="Local development" \
pnpm tauri dev --features analytics --config src-tauri/tauri.debug.conf.json
```

An empty queue makes no periodic network request. The first queued event starts
a one-minute delivery timer; later arrivals do not move it. Reaching 50 queued
events makes delivery due immediately. A pass sends at most 50 requests. If
rows remain after a successful pass, the next pass waits a protected minute.

A failed request stops the pass and retries after 1, 2, 4, 8, then at most 15
minutes. Five failed attempts discard a row. The queue keeps the newest 500
rows. Consent is checked before every request. If consent, configuration, or the
store cannot be read while a backlog remains, no request is sent and another
check is scheduled after 15 minutes. Opt-out clears the queue and delivery
parks.

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
| Opt-out gates, queue, delivery                            | [`analytics/mod.rs`](../apps/desktop/src-tauri/src/analytics/mod.rs)       |
| The payload, and the closed field set                     | [`analytics/event.rs`](../apps/desktop/src-tauri/src/analytics/event.rs)   |
| Endpoint configuration, and why a clean checkout is inert | [`analytics/config.rs`](../apps/desktop/src-tauri/src/analytics/config.rs) |
| The setting, and the upgrade rule for existing installs   | [`store/mod.rs`](../apps/desktop/src-tauri/src/store/mod.rs)               |
| The reader-facing copy                                    | [`PrivacyPane.tsx`](../apps/desktop/src/views/settings/PrivacyPane.tsx)    |
