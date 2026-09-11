# Product analytics audit and measurement plan

Audited on 2026-09-08 at `3ce2b1da`. This is a source audit, not an analysis of
production event volumes. The collector, warehouse, release secrets, and live
dashboards were not inspected. The findings below preserve the state at that
revision. [analytics.md](analytics.md) is the catalog of implemented events.

At the audited revision, events established that some installations ran,
completed setup, and opened sessions. They could not establish how often people
deliberately used the app, which surfaces provided value, or whether a feature
was unused because its data never loaded.

## Implementation status

Phase 1 is implemented in the current source. The closed catalog now records
successful surface and Settings-pane visibility, visible surface outcomes,
deliberately viewed provider states, and classified setup starts and
completions. `no_credentials` is an accepted provider-state value but remains
dormant because the current product boundary cannot prove it. The queue now
wakes at a bounded depth and uses protected drain and retry delays. The reviewed
Burn Checks part of Phase 3 is implemented as of 2026-09-10. Other Phase 2 and
Phase 3 proposals remain unimplemented. Collector verification, production
reports, and cohort review remain operational work.

## Coverage at the audited revision

The closed catalog had nine events. Envelope fields provided event IDs, rotating
installation IDs, application analytics session IDs, capture and send times,
app version, OS, and architecture. Properties were fixed labels and coarse
buckets; the Claude diagnostic had nine additional optional fields.

| Current event                            | Actual trigger and dimensions                                                                                                                                      | What it answers and misses                                                                                                              |
| ---------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------- |
| `antiburn.app_launched`                  | Shell setup; no event-specific properties.                                                                                                                         | Reports launches, including unattended startup. Does not measure visits, continued use, or a new installation.                          |
| `antiburn.onboarding_step_viewed`        | `OnboardingSession.noteOnboardingStep`; four fixed steps, once per step per flow instance.                                                                         | Shows setup progress. Has no new-versus-restarted flow distinction or failure reason.                                                   |
| `antiburn.onboarding_finished`           | After the finish command saves settings. Explicit setup restarts can emit another completion.                                                                      | Shows completed setup, not first useful data or unique new installations.                                                               |
| `antiburn.scan_completed`                | Full discovery pass; first outcome or changed count bucket relative to the previous reported outcome. Scoped passes emit nothing.                                  | Shows coarse discovery health and inventory. Is not an interaction or a scan-attempt counter.                                           |
| `antiburn.setting_toggled`               | Saved changes to `live_usage`, `notifications`, `launch_at_login`, or `discovery_paused`; key only.                                                                | Shows use of four controls. Does not show direction, current adoption, other settings, or success of OS integration.                    |
| `antiburn.session_opened`                | Activity-card handler before analysis loads; agent category and native/WSL.                                                                                        | Measures list-to-detail intent. Does not establish that detail loaded, or cover related sessions, subagents, or newer/older navigation. |
| `antiburn.error_occurred`                | Full scan failure, with `scan_failed`; repeated identical outcomes suppressed.                                                                                     | Shows some discovery failures. Misses scoped failures and other feature failures; cannot supply an operation failure rate.              |
| `antiburn.unrecognized_records_observed` | Nonempty unknown-record summary returned to Settings Insights; changed category/count bucket within the process. Clean results reset suppression without an event. | Diagnoses reader-selected cohorts. Does not count all Insights visits or population parser failure rates.                               |
| `antiburn.claude_limit_reset_observed`   | Changed Claude reset diagnostic after normal usage refresh, with a separate five-minute cooldown and additional enablement gates.                                  | Answers a narrow provider experiment question. Does not establish that anyone viewed usage or used a reset.                             |

Evidence: [event schema](../apps/desktop/src-tauri/src/analytics/event.rs),
[recording and delivery](../apps/desktop/src-tauri/src/analytics/mod.rs),
[shell commands](../apps/desktop/src-tauri/src/commands.rs),
[launch](../apps/desktop/src-tauri/src/lib.rs),
[scan scope](../apps/desktop/src-tauri/src/scan/mod.rs),
[onboarding](../apps/desktop/src/views/onboarding/OnboardingSession.ts), and
[activity-card handler](../apps/desktop/src/views/PopoverView.tsx).

## Findings, in priority order

1. **There is no deliberate-use denominator.** Popover reveals, Settings pane
   views, provider/check previews, and HUD detail views are absent. A person
   reading the tray meter can get value without any recorded interaction. A
   person opening the popover daily without a session-card click is also almost
   invisible. Do not equate background operation with engagement.
2. **Intent has almost no outcome coverage.** Session-card clicks precede
   analysis. There is no general distinction between useful data, empty data,
   loading that stalls, and a failed view. Live provider failures, source access
   trouble, and analysis failures can make a feature appear simply unwanted.
3. **The analytics session is an event-activity window.** Every queued event
   calls `current_session_id`, including background scans and diagnostics.
   Background changes can keep it alive; silence can split one process run.
   Neither its count nor its first-to-last timestamp measures engaged visits or
   time spent. Adding events would change these measures again.
4. **Feature adoption and intervention results are missing.** HUD enablement,
   notification actions, source setup, report refresh, session actions, and
   update actions have no complete measurement. Four setting keys without
   direction cannot show enablement or abandonment. A saved preference also
   does not prove that the corresponding feature works.
5. **More events can expose delivery limits.** The queue keeps the newest 500
   rows. After a 60-second initial delay, a flush sends up to 50 sequential
   requests, then waits 15 minutes. Healthy sustained capacity is approximately
   200 events/hour, less when requests are slow. One failed request stops a
   drain; five failed attempts discard its row. Short-lived installs can leave
   events queued until a later launch, and overflow drops earlier funnel steps.
   Increased volume needs a delivery load test, not just more call sites.
6. **Documentation and tests do not fully enforce semantics.** The catalog test
   checks that event names occur in the document; it does not validate trigger
   behavior, property vocabularies, or whether new features have coverage.
   `Facts` uses static strings, not event-specific property enums. The renderer
   IPC is more restrictive, but TypeScript still accepts any string for agent.
   Default Cargo tests exclude most analytics tests; CI does run the enabled
   suite on Linux.

Documentation discrepancies found at the audit revision:

- `CONTRIBUTING.md` said analytics waits until onboarding completes, whereas
  `allowed` has no onboarding gate. This audit corrects the contributor text to
  match the implementation and public policy.
- The public scan row says exact count changes and once-a-minute visible scans.
  The code uses count buckets from full passes; it runs full reconciliation
  about every five minutes, while watcher-driven scoped passes emit no scan
  analytics.
- Public identifier descriptions say `sessionId` is never written to disk. Its
  live generator state is memory-only, but each serialized queued payload
  includes the ID on disk. Public descriptions of inactivity also need to say
  analytics-event inactivity, not imply user inactivity.

The foundation documentation change corrected those disclosures. The Phase 1
implementation status is separate from these historical findings.

## Measurement definitions

Use reporting installations, not people. Installation IDs rotate after 30 days
and reset after opt-out/re-enable. Do not add a stable identifier or use IP,
user-agent, or device information to join rotations.

| Question                                           | Definition after the first implementation phase                                                                                                                                                                                                    | Decision supported                                               |
| -------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| Are people deliberately returning?                 | Daily/weekly distinct `anonymousId` with a user-initiated core `surface_viewed` event or `settings_pane_viewed` for `insights`. Exclude setup, other Settings-only visits, automatic restores, nudges merely appearing, and all background events. | Whether the core utility earns repeat attention.                 |
| Does setup lead to visible value?                  | Among distinct IDs completing a new setup flow, fraction that see a core surface with `ready` data within 24 hours of completion. Also report empty, error, and timeout outcomes.                                                                  | Whether to improve setup or the first data experience.           |
| Which features get used?                           | Distinct IDs viewing each core surface divided by engaged reporting IDs in the same interval and supporting versions/platforms. Separate ready-data reach from view reach.                                                                         | Which surfaces merit investment or improved discovery.           |
| Do users return after value?                       | Among IDs first reaching ready data after new setup, observed return on day 1 and day 7 using deliberate core views. Include only cohorts whose full observation window has elapsed.                                                               | Whether activation translates into observed repeat use.          |
| Where does the experience fail?                    | Per surface, fraction of reporting exposed IDs with empty, error, or loading-timeout states. For explicit operations, compare terminal outcomes with attempts separately.                                                                          | Which reliability issues block adoption.                         |
| Does passive monitoring remain enabled and useful? | Report HUD/meter enablement, successful automatic exposure, data availability, and deliberate detail use separately.                                                                                                                               | Whether passive display features are configured and functioning. |

Use capture time for behavior, deduplicate retries by `messageId`, and allow a
documented late-arrival window before finalizing cohorts. Segment by app version
and OS so older builds and unsupported features do not enter new-event
denominators. Installation rotation and missing delivery bias retention
downward; a first-seen ID is not necessarily a new install. Opted-out and
unconfigured builds are unobserved, so analytics cannot establish total users,
opt-out rates, uninstalls, or precise long-term retention.

For activation and return cohorts, require ready data on a user-initiated
exposure. Automatic HUD restoration can establish that the display works, but
does not establish that the user has reached value through deliberate use.

For visit frequency, group only deliberate interaction events by installation
with a documented 30-minute inactivity gap in analysis. Ignore background events
when constructing visits. Keep the existing wire `sessionId` unchanged initially
and document its actual meaning. Do not infer attention duration from gaps.
Neither tray visibility nor a persistent HUD proves that someone looked at it.

## Event additions and proposals

All names below use the `antiburn.` prefix. The Phase 1 dimensions are
implemented closed vocabularies, not arbitrary strings or permission to upload
payloads. Use event-specific Rust types even when serializing into existing
`label`, `detail`, and `bucket` fields. Update disclosures for new meanings even
when the wire field count stays unchanged.

### Phase 1: visible use and value (implemented)

| Event/change                                       | Trigger and safe dimensions                                                                                                                                                                                                              | Owner and volume rule                                                                                                                                                                                                                                                          |
| -------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `surface_viewed`                                   | Successful reveal or visible navigation. `label`: `activity`, `session_detail`, `provider_preview`, `checks_preview`, `hud`, `hud_detail`, or `settings`. `detail`: `user` or `automatic`.                                               | Shell visibility transition plus surface controllers. One per actual transition; no event for prewarm, repeated show requests, data refresh, or hidden navigation. Only deliberate transitions qualify as engagement.                                                          |
| `settings_pane_viewed`                             | Requested pane is selected and visible. `label`: the eight existing Settings pane IDs.                                                                                                                                                   | `SettingsWindowSession`, including first opening and external pane requests. One per visible pane transition, with duplicate requests suppressed.                                                                                                                              |
| `surface_state_observed`                           | Data state presented on a visible surface. Same surface vocabulary, plus `insights`; `detail`: `ready`, `empty`, `error`, or `loading_timeout`. Ready means a usable payload, not merely a mounted component or successful IPC response. | Surface controllers after both visibility and data readiness. At most once per distinct state per surface exposure; ignore stale asynchronous results. Use a documented 10-second visible initial-load timeout, canceled when hidden; later ready data can still emit `ready`. |
| `live_usage_state_observed`                        | A provider state is presented on Activity, a provider preview, or a user-opened HUD. `label`: `anthropic`, `openai`, or `google`; `detail`: `fresh`, `stale`, `authentication`, `rate_limited`, `unavailable`, or `no_credentials`.      | Map existing presentation states, without an analytics-only provider request. Deduplicate each provider/state within a deliberate visit. `no_credentials` remains dormant. No account, plan name, balance, quota value, or raw response.                                       |
| `onboarding_started`, extend `onboarding_finished` | Visible start or resume and committed completion of a setup flow. `label`: `new` or `restart`.                                                                                                                                           | Emit a start on the first visible start or resume in each app process. A quit and later resume emits another start with the persisted classification. Emit completion once per pending-to-complete transition. Preserve the four existing step events.                         |

`surface_state_observed` also needs a closed `properties.origin` value of `user`
or `automatic`, inherited from its exposure. This optional wire field lets
reports separate deliberate value from passive display without guessing from
neighboring timestamps. It stays absent on unrelated events.

`unrecognized_records_observed` also carries `properties.unrecognizedTypes`,
from the next release after 0.5.0: up to 16 unknown transcript record type
names, sanitized (ASCII, bounded length, a fixed character set, a shared
`<rejected>` sentinel for anything that fails that check) rather than mapped
to a reviewed closed vocabulary. This is the one field on the closed catalog
that is not a reviewed enum value, because the product question it answers —
which new record type names an agent has started writing — cannot be answered
by a value chosen in advance. Change-only suppression now compares the
sanitized type list alongside the label and bucket, so a newly observed name
is a reportable change even when both stay the same. It stays absent on every
other event, and on an event from an app version that predates it.

Define the start classification from the explicit restart path and existing
onboarding state, not from whether the analytics identifier has been seen.
Analyze setup progress by distinct installation and ordered times; repeated
flows in one observation window are not independent new installations. Earlier
`onboarding_finished` events have no classification label. Exclude those legacy
events from new-versus-restart cohorts instead of treating a missing label as
`new`.

Keep `session_opened` as the existing activity-card intent event. Pair it with
visible session detail and data state for the activation funnel. Record visible
detail navigation from related sessions and subagents without transmitting a
session ID or title. Cached ready data counts when actually shown; a background
cache refresh does not count as a view.

The existing seams are
[popover reveal/hide](../apps/desktop/src-tauri/src/popover.rs),
[popover data and navigation](../apps/desktop/src/views/popover/PopoverSession.ts),
[preview controller](../apps/desktop/src/views/popover/PopoverPeekController.ts),
[Settings navigation](../apps/desktop/src/views/settings/SettingsWindowSession.ts),
[Insights controller](../apps/desktop/src/views/settings/InsightsSession.ts), and
[HUD controller](../apps/desktop/src/views/overlay/OverlaySession.ts).
Carry an internal exposure generation through readiness and loading callbacks
to reject duplicates and stale results. It need not leave the process.

### Phase 2: adoption and actions

| Proposed event                                | Trigger and safe dimensions                                                                                                                                                                                                                                                                                                            | Owner and volume rule                                                                                                                                                                                                                         |
| --------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `setting_changed`                             | Successfully saved change; allowlist product toggles such as live usage, notifications, discovery, launch at login, and HUD enablement. Fixed `enabled`/`disabled` state.                                                                                                                                                              | The relevant persistence boundary. Emit only an actual transition. Keep existing `setting_toggled` semantics stable while migrating dashboards; do not count both as separate actions.                                                        |
| `action_requested`, `action_completed`        | A deliberate operation starts, then reaches success, failure, or cancellation. Initial action vocabulary: manual scan, report refresh, source add/remove, source permission request, reveal source, diagnostics export, delete local session data, and clear local index. `detail` on completion: `success`, `failed`, or `cancelled`. | UI handler owns intent; operation boundary owns outcome. One pair per explicit attempt; automatic retries remain one attempt. Do not send action arguments, file paths, exported data, or exact deletion counts.                              |
| `notification_shown`, `notification_actioned` | Actual display and explicit action; fixed notification kind and action vocabulary.                                                                                                                                                                                                                                                     | Nudge lifecycle after display succeeds, not when delivery is requested. Suppress duplicate display callbacks; separate test/location nudges from product alerts. Never send message text, actor, provider account, or usage milestone values. |
| `update_action_completed`                     | User-requested check, installation, or restart request reaches its known result; fixed action and outcome.                                                                                                                                                                                                                             | Update controller. Do not treat a restart request as verified installation; confirm running versions through later launch events. Suppress background polling and progress callbacks.                                                         |

The setting values and additional provider categories expand today's disclosure;
update it before shipping. Do not report analytics opt-out after withdrawal or
send a settings snapshot. Change events establish adoption among observed
changers, not prevalence across all installs. Use observed HUD exposure for
existing adopters; add a narrowly scoped state observation only if a specific
prevalence question still cannot be answered.

For operation success rates, correlate each attempt locally and emit exactly
one terminal outcome. Compare aggregate counts only after allowing in-flight
attempts and late delivery to settle. Process exits can leave unmatched
attempts; expose that category instead of treating every missing completion as
failure. Do not introduce persistent operation or work identifiers.

### Phase 3: targeted diagnostics and measurement quality

#### Burn Checks integration (implemented 2026-09-10)

| Product question and decision                                                                                                                                          | Metric and denominator                                                                                                                                                                                                   | Trigger and closed fields                                                                                                                                                                                                                                                                                      | Suppression and maximum volume                                                                                                                                                                                                                                 |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Does deliberate Burn Checks use reach usable results? Improve discovery or report reliability.                                                                         | Distinct reporting installations with `burn_checks` `ready`, `empty`, `error`, or `loading_timeout`, divided by installations with a user-origin `burn_checks` `surface_viewed`, on app versions from 2026-09-10 onward. | The main-window section is both selected and natively visible. `surface_viewed` uses `label=burn_checks`, `detail=user`. `surface_state_observed` uses `label=burn_checks`, the four existing state values, and `origin=user`. Ready requires a finding or clean result; an all-unassessed report is empty.    | Hidden sections, stale results, remounts, and refreshes do not create exposures. Each state occurs once per exposure. Maximum: one view and four state events per exposure.                                                                                    |
| Do readers proceed from Auto Fix review to confirmation, and what result follows? Improve review copy or operation reliability.                                        | Confirmations divided by `burn_check_auto_fix_reviewed` with `detail=ready`; each typed completion outcome divided by confirmations. Allow in-flight and late-delivery time before finalizing.                           | A Fix request completes with review `detail`: `ready`, `stale`, `expired`, `conflict`, `unavailable`, or `failed`. Apply selection emits `burn_check_auto_fix_confirmed`. Completion `detail`: `applied_awaiting_verification`, `recovery_needed`, `stale`, `expired`, `conflict`, `unavailable`, or `failed`. | Busy controls suppress duplicate concurrent requests. Each explicit retry is a new attempt. Maximum: one review event per Fix request, one confirmation and one completion per Apply request. No automatic retry emits another event.                          |
| Does prompt preparation reach a usable clipboard result? Improve prompt preparation or clipboard handling.                                                             | Successful `burn_check_prompt_copied` divided by `burn_check_prompt_prepared` with `detail=ready`. Report preparation failures separately.                                                                               | A Copy fix prompt request emits preparation `detail`: `ready`, `stale`, `expired`, `unavailable`, or `failed`. A successful clipboard write emits `burn_check_prompt_copied` with no properties.                                                                                                               | A clipboard failure emits no copy event. Clipboard retry reuses the prepared prompt and emits no second preparation event. Busy and copied controls suppress duplicate calls. Maximum: one preparation and one successful copy for one rendered action. A non-named check submits selected current targets once. |
| Do later improvements and recurrences become visible, and did their watch start passively or from an action? Improve verification coverage without claiming causation. | Distinct reporting installations with each `(detail, origin)` tuple among deliberate Burn Checks exposures. Use exposed installations as the denominator. Never use background transitions as visits.                    | `burn_check_outcome_observed`: `detail=verified` or `recurred`; `origin=passive` or `action`. Verified comes from the visible aggregate summary or an expanded target. Recurred comes only from a deliberately expanded visible target.                                                                        | Background evaluation, hidden sections, hidden target lists, and refresh callbacks emit nothing. Deduplicate each tuple per exposure. Maximum: four events per exposure, independent of finding count.                                                         |

These events use only the existing `detail` and `origin` wire fields. They never
include prompts, resource or model strings from work, paths, finding, action,
watch, reference, or sample IDs, exact token or cost values, private errors, or
evidence revisions. Product reports must segment at the 2026-09-10 app-version
boundary and show reporting installation counts.

Add scoped scan, analysis, storage, or provider diagnostics only for a named
reliability question that visible-state events cannot answer. Use fixed error
categories and changed-state suppression. Do not add an unconditional heartbeat
or events per poll, transcript record, chart hover, scroll tick, or token update.
Give temporary diagnostics, including the Claude reset probe, an owner and
review date so experiments do not become permanent noise or provider traffic.

The first narrow background-summary exception is
`antiburn.resource_usage_observed`. It answers: "Does a supported app version
introduce an antiburn resource regression?" Compare each fixed resource band by
app version and operating system. Use observed installation-hours as the
denominator and show `none`, `partial`, and `full` coverage beside each
distribution. Report the number of observed installations and hours. Do not
substitute event count for either denominator. Exclude this event from
engagement, activation, retention, visit, and time-spent definitions.

The shell samples only its own process CPU, platform-specific memory, process
read/write counters, and logical SQLite database and WAL file sizes. It uses a
five-minute cadence, skips missed ticks, never overlaps reads, emits no per-sample
event, and takes no resource measurement while analytics is disabled. A delayed
read can be followed soon by the next scheduled tick. The event contains one typed,
thirteen-field closed summary after at least one enabled monotonic hour with a
sample. Suspension or a delayed tick can make the reporting window longer.
Missing measurements stay unavailable and carry coverage; they never become
zero. Full coverage means every attempted sample or interval succeeded, not
that every instant was measured or no tick was missed. Counter rates are elapsed-time weighted over independently valid
intervals. The sampled memory maximum is not a true peak. CPU and memory cover
the shell process, not renderers or whole-app totals. Linux I/O can include
accounting inherited from waited-for child processes. Windows I/O is all shell-process
I/O, not physical disk traffic. No metric measures the whole machine, and the event
does not enumerate renderers. There is no shutdown catch-up summary.

Interpret changes with survivor and consent bias: a process must remain running
long enough to contribute usable samples, and opted-out installations are
unobserved. A distribution shift supports regression investigation, not a claim
about all users or a causal conclusion. Resource bands can reveal coarse app
work intensity and data volume, so the public catalog, privacy policy, and
in-product disclosure name that consequence.

Phase 1 adds bounded queue-depth-triggered draining, protected retry backoff, and
a request budget. Before expanding volume further, simulate normal repeated
visits, preview use, and an offline backlog against the 50-event drain and
500-row queue. Preserve the opt-out recheck before each request and silent
failure. Do not merely increase queue size or add a blocking exit flush. Monitor
delivery lag, duplicate message IDs, rejection counts, and event mix in the
collector separately from product engagement.

## Event review contract

Every added or changed event must document:

1. The product question, intended metric, denominator, and decision it supports.
2. The exact trigger and owning boundary, including whether it records intent,
   visibility, a completed outcome, or background health.
3. Every allowed property and value, serialization shape, and exclusion of
   user/work identifiers and content. Do not pass DTOs directly to analytics.
4. Duplicate suppression, cancellation, hidden-window behavior, retries,
   automatic restores, and expected maximum volume for the measured flow.
5. The public catalog/disclosure changes and the app-version boundary when
   semantics change. Do not silently reuse an event name for a new meaning.
6. Validation of the actual product path, not just a call to the tracker.

Feature work must answer this contract with existing coverage, added coverage,
or a concrete reason measurement is unnecessary. The goal is sufficient
coverage to make decisions, not a fixed event count per pull request.

Tests should prove that the user action produces the expected event, a failed
operation cannot emit success, and remounts, prewarm, refreshes, and duplicate
callbacks do not invent use. Exercise canceled and stale requests where the
flow has them. Validate exact allowed wire properties and rejected unknown
values at the Rust boundary. Verify opt-out, environment disablement, absent
configuration, and queue/retry bounds when those paths change. Keep tests in
the enabled-feature suite as well as relevant frontend tests.

Strengthen the catalog check in a follow-up: use event-specific property enums
and a machine-readable schema to validate exact names and values against the
public catalog. A schema check cannot establish that a view was truly visible;
behavior tests and PR review still own that requirement. Avoid a CI rule that
merely requires an analytics file to change in every feature PR.

## Delivery sequence and acceptance

1. The source now contains the disclosure corrections, Phase 1 events, and
   bounded delivery changes. Establish baseline collector queries by app version.
   Verify configured release ingestion, retry deduplication, and delivery delay.
   Do not claim production behavior from source alone.
2. Verify Phase 1 with a loopback recording of setup → activity → preview →
   session detail → Settings Insights → return visit. Also exercise empty/error
   data, hidden prewarm, renderer recreation, automatic HUD restore, and opt-out.
   Assert the expected sequence and absence of duplicate use events.
3. Build engagement, activation, feature reach, observed day-1/day-7 return, and
   visible reliability reports using the definitions above. Show reporting
   installation counts and observation limits alongside percentages.
4. Review at least two complete weeks of supported-version cohorts, then choose
   Phase 2 actions based on observed gaps. Assign each report and diagnostic a
   maintainer; review whether its events still support an actual decision.

The historical audit changed documentation, current public disclosures, and
contributor expectations. Phase 1 and the reviewed Burn Checks Phase 3
integration now add runtime instrumentation with bounded delivery behavior.
Collector configuration, dashboards, and the remaining Phase 2 and Phase 3
implementation remain work described above.
