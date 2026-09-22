# Analytics measurement definitions

This document defines how to interpret the current analytics events and review
changes to them. The [public analytics catalog](analytics.md) owns the complete
event inventory, payload fields, triggers, delivery behavior, and disclosure.
Source instrumentation does not by itself establish collector ingestion or a
production report.

## Reporting population and time

Report **installations observed through delivered events**, not people or all
installations. `anonymousId` rotates after 30 days and resets after opt-out and
re-enable. A first-seen ID is not necessarily a new install. Do not add a stable
identifier or join rotations using IP, user-agent, or device information.
Opted-out and unconfigured builds are unobserved, and offline or failed delivery
can omit events. Analytics therefore cannot establish total users, opt-out
rates, uninstalls, or precise long-term retention.

Use `originalTimestamp` for behavior, deduplicate retries by `messageId`, and
allow a documented late-arrival window before finalizing cohorts. Show the
number of reporting installations with each rate. Segment by app version and OS;
restrict each denominator to versions and platforms that support the feature.
When an event or its vocabulary changes, earlier events cannot be reclassified
from a coarse or missing value. In particular, older setup completions have no
`new`/`restart` label, older limit-factor events cannot identify Claude Max
tiers or OpenAI Pro Lite, and older unknown-record events have no
`unrecognizedTypes` list.

The wire `sessionId` groups captured analytics events. Background events can
extend it, while silence can split one process run. It is neither a visit nor
attention duration. For visit frequency, group only deliberate interaction
events by installation with a documented 30-minute inactivity gap; exclude
background events. Do not infer time spent from gaps, tray visibility, or a
persistent HUD.

Core surfaces are `activity`, `session_detail`, `provider_preview`,
`checks_preview`, `burn_checks`, `hud`, `hud_detail`, and `quota`. Count only
their `surface_viewed` events with `detail=user` as deliberate core views.
Settings-pane views do not qualify. Earlier builds could report an `insights`
Settings pane and state; exclude that legacy value when comparing current
core-use cohorts.

## Core product measures

| Question                          | Definition and decision limit                                                                                                                                                                                                                                                                                    |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Do readers deliberately return?   | Daily and weekly distinct reporting IDs with a user-origin core `surface_viewed`. Exclude setup, Settings-only visits, automatic restores, mere nudge display, and all background events. This measures observed repeat attention, not passive meter reading.                                                    |
| Does setup lead to visible value? | Among distinct IDs completing a `new` setup flow, count those who see `ready` data on a user-initiated core exposure within 24 hours. Report `empty`, `error`, and `loading_timeout` alongside `ready`. A resumed flow can emit another start; repeated flows from one ID are not independent new installations. |
| Which features reach readers?     | For each core surface, divide distinct IDs with a deliberate view by engaged reporting IDs on supported builds. Report `ready` reach separately from view reach. A visible view does not imply useful data.                                                                                                      |
| Do readers return after value?    | Among IDs first reaching deliberate `ready` data after new setup, report observed return on day 1 and day 7 through deliberate core views. Include only cohorts whose full observation window has elapsed. Rotation and delivery loss bias return downward.                                                      |
| Where does visible use fail?      | For each surface, report distinct exposed IDs with `empty`, `error`, or `loading_timeout` states against distinct exposed IDs. A later `ready` can follow a timeout; state categories can overlap. Compare explicit operation attempts with their typed terminal outcomes separately.                            |
| Does passive monitoring work?     | Report successful automatic HUD exposure, available data, and deliberate detail use separately. Current events do not establish HUD enablement prevalence or whether a reader looked at a persistent display. A saved preference does not prove its OS integration works.                                        |

`session_opened` records an Activity-card intent before detail data loads. Use
the visible session-detail view and state for the outcome. A cached result
counts only when shown, and a background refresh is not engagement. Burn Checks
`ready` requires a finding or clean result; an all-unassessed report is
`empty`.

## Feature and diagnostic measures

Use these denominators for current events. The catalog defines each event's
exact fields, suppression, and allowed values.

| Question                                                 | Metric and limit                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              |
| -------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Which Sessions filters are selected?                     | Distribution of `session_filter_selected` by filter and, for recognized agent filters, agent, among reporting installations that changed a filter. Restoration and reselecting the current filter emit nothing; this is choice among changers, not current filter prevalence.                                                                                                                                                                                                                                                                 |
| Which interface sizes and routes are chosen?             | Distribution of `interface_scale_changed` by preset and Settings, shortcut, or menu route among reporting installations that saved a change. Restoration emits nothing, so this is not current preset adoption.                                                                                                                                                                                                                                                                                                                               |
| Are project folder actions used and successful?          | Count settled `project_folder_action` attempts and failures by `open` or `copy`. The denominator is observed attempts, not sessions or panel views. OS acceptance of an open request does not prove the file manager displayed the folder.                                                                                                                                                                                                                                                                                                    |
| Do Burn Checks actions complete?                         | Report Auto Fix reviews by typed result, confirmations against `ready` reviews, and typed completions against confirmations. Report successful prompt copies against `ready` preparations, with preparation failures separate. Allow in-flight attempts and late delivery to settle; a clipboard failure emits no copy success. An explicit retry is another attempt.                                                                                                                                                                         |
| Do verified checks and recurrences become visible?       | Count distinct exposed installations by `(verified or recurred, passive or action)` from `burn_check_outcome_observed`, against deliberate Burn Checks exposures. The watch origin describes how it started; it does not establish that the action caused the result. Multiple targets with one tuple collapse within an exposure.                                                                                                                                                                                                            |
| Is main-window navigation useful?                        | Count installations with `navigation_history_moved` among main-window viewers; count `app_search_opened` among those viewers. Compare `app_search_result_opened` with search opens by result category and report distinct installations too. A search result means accepted navigation, not loaded data, executed work, or a changed setting.                                                                                                                                                                                                 |
| How close do usage windows run to limits?                | Report `usage_observed` bands by provider and short or long window. These are changed-state observations, not request counts or exact percentages. Provider failures and supplemental windows do not contribute. Non-authoritative windows may still report a coarse band.                                                                                                                                                                                                                                                                    |
| How accurate are learned factors?                        | Distribute `limit_factor_observed` factor and residual bands by provider, lane, and mapped plan among reporting installations. Its 24-hour floor and first-account-per-provider/lane narrowing make event counts unsuitable as account or window counts. Keep model-scoped lanes separate without attempting to identify a model.                                                                                                                                                                                                             |
| How accurate are closed quota windows?                   | Distribute `quota_window_closed` estimate bias, unexplained band, and reading coverage by provider, lane, and mapped plan, with reporting installations shown. This is a dollars-only estimate against the last provider reading after a window closes; it does not reconstruct the badge or forecast a reader saw. Only windows with an observed period ID, closed within the 14-day lookback, can report. A durable marker prevents retries, so a crash after marking can leave a missed event.                                             |
| How common are unknown records or report-time incidents? | `unrecognized_records_observed`, `quota_incidents_observed`, and `provider_incidents_observed` describe completed Checks report requests from the popover or main window. They can record before the renderer presents the report. Their changed-state buckets do not measure population parser or incident rates and do not establish deliberate use. Missing `unrecognizedTypes` on an older event means the build predates that field, not that no unknown type existed.                                                                   |
| Is a provider failure widespread?                        | For `provider_incidents_ingested`, report distinct installations by agent and incident kind per hour against installations that sent any event that hour. This is an ingest-time fleet signal, not a request failure rate: closed laptops, stale sessions, and opted-out installs contribute nothing. Its two-hour freshness window permits delayed reporting.                                                                                                                                                                                |
| Did app resource use shift?                              | Compare `resource_usage_observed` bands by app version and OS using observed installation-hours, with installation counts and `none`/`partial`/`full` coverage alongside each distribution. Never use event count as the denominator. The shell process must run long enough to sample; CPU and memory omit renderers, the memory maximum is sampled rather than a true peak, I/O has platform-specific meanings, and missing measurements are not zero. A shift warrants regression investigation, not a claim about all users or causation. |
| How many withdrawals were observed?                      | Count `analytics_opted_out` among configured, previously enabled installations that delivered the signal. Exclude it from engagement, activation, retention, visit, and time-spent measures. Offline, failed, unconfigured, environment-disabled, and crashed-before-delivery withdrawals can be missed; the signal cannot establish an opt-out rate for all installs.                                                                                                                                                                        |

Surface state and Burn Checks outcome events require visible exposures. Hidden
results, prewarm, remounts, refreshes, and background verification do not count
as use. Provider states are shown only on Activity, deliberate previews, or a
user-opened HUD; `no_credentials` is an allowed but currently dormant state.
The Claude reset diagnostic is a separate, gated provider probe and does not
measure use of a reset operation. It must not be mixed with the broader
`usage_observed` measure. Avoid interpreting one event per changed state or
bucket as the number of underlying occurrences.

## Event review contract

Every added or changed event must document:

1. The product question, intended metric, denominator, and decision it supports.
2. The exact trigger and owning boundary: intent, visibility, completed outcome,
   or background health.
3. Every allowed property and value, serialization shape, and exclusion of work
   identifiers and content. Do not pass product DTOs directly to analytics.
4. Duplicate suppression, cancellation, hidden-window behavior, retries,
   automatic restores, and expected maximum volume for the measured flow.
5. The [public catalog](analytics.md) and disclosure changes, plus the app-version
   boundary when semantics change. Do not reuse an event name for a new meaning.
6. Validation on the actual product path, not only a call to the tracker.

Feature work must answer this contract with existing coverage, added coverage,
or a concrete reason measurement is unnecessary. Avoid an unconditional
heartbeat or events per poll, record, chart hover, scroll, or token update.
For a new background diagnostic, state the owner, review date, and decision
that warrants its volume and any provider traffic.

Tests should prove that the user action produces the expected event, a failed
operation cannot emit success, and remounts, prewarm, refreshes, and duplicate
callbacks do not invent use. Exercise cancellation and stale results where the
flow has them. Validate exact allowed wire properties and rejected unknown
values at the Rust boundary. Verify opt-out, environment disablement, absent
configuration, and queue and retry bounds when those paths change. Run the
analytics feature suite and relevant frontend tests. The catalog test checks
event names; it cannot prove triggers, allowed values, or visible use.
