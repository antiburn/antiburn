# Product analytics measurement contract

Current model-based check event contract: 2026-10-27. The historical foundation
audit below was made on 2026-09-08 at `3ce2b1da`. This is a source audit, not an analysis of
production event volumes. The collector, warehouse, release secrets, and live
dashboards were not inspected. The findings below preserve the state at that
revision. [analytics.md](analytics.md) is the catalog of implemented events.

At the audited revision, events established that some installations ran,
completed setup, and opened sessions. They could not establish how often people
deliberately used the app, which surfaces provided value, or whether a feature
was unused because its data never loaded.

## Implementation status

The first delivered analytics set is implemented in the current source. The closed catalog now records
successful surface and Settings-pane visibility, visible surface outcomes,
deliberately viewed provider states, and classified setup starts and
completions. `no_credentials` is reported only for the Claude Desktop-only state, which
the shell proves from a local presence check. The queue now
wakes at a bounded depth and uses protected drain and retry delays. The reviewed
Burn Checks instrumentation is implemented as of 2026-09-10. Its typed-operation
review UI is implemented as of 2026-09-15. Other proposals remain unimplemented. Collector verification, production
reports, and cohort review remain operational work.

The limit-factor plan mapping was refined on 2026-09-11. Claude's `max` plan
can report `max_5x` or `max_20x` from its reviewed tier, and OpenAI's
`prolite` plan now has its own value. Existing coarse values remain the
fallback for absent or unrecognized inputs. Reports must segment this change
by the first app version that contains it; earlier `max` and `other` events
cannot be split retrospectively.

The opt-out withdrawal event is implemented with the consent transition. Its
durable queue row records the fixed signal before the stored preference changes;
one bounded final drain may deliver it, then local analytics state is deleted.
An offline or failed drain is an accepted miss and never retries after disable.
Use `antiburn.analytics_opted_out` to count observed withdrawals among
configured installations that had analytics enabled, and exclude it from
engagement, activation, retention, visit, and time-spent denominators. Segment
reports at the app-version boundary where this event ships. Missing identity,
environment-disabled, unconfigured, and crashed-before-delivery cases remain
unobserved; the event carries no properties beyond the standard envelope.

Application interface size changes are implemented at the native persistence
boundary. The product question is which supported sizes readers choose and
whether they find the Settings, keyboard, or native-menu route. Reports use
the distribution of `antiburn.interface_scale_changed` by preset and source
among reporting installations. This guides preset and control placement; it
does not measure current adoption because automatic restoration emits nothing.
The event fires only after an atomic saved transition to another allowlisted
preset. Its `label` is one of `90`, `100`, `110`, `125`, `150`, `175`, or
`200`; its `detail` is `settings`, `shortcut`, or `menu`. Edge shortcuts,
repeated selections, startup restoration, generic Settings saves, failed
requests, and renderer reconciliation emit nothing. Maximum volume is one
event per successful user-requested change. It excludes window bounds, display
details, DPI, content, and work identifiers. Reports segment at the first app
version that includes this event.

## Coverage at the audited revision

### OpenCode V2 source review (2026-10-20)

V1 and V2 retain one `opencode` agent category. Existing session-open,
session-filter, and visible-surface events cover use of their shared product
surfaces. They do not distinguish source versions or measure live V2 connection
success. Source binding, authentication, continuity, and schema diagnostics stay
local. This is an intentional measurement gap: the event contract has no reviewed
V2 connection event, and arbitrary YAML check IDs must not enter native check
events. No session IDs, native IDs, source paths, endpoint URLs, registration
credentials, or event payloads enter analytics. A future connection metric needs
its own closed states, deduplication rule, and event review before instrumentation.

The closed catalog had nine events. Envelope fields provided event IDs, rotating
installation IDs, application analytics session IDs, capture and send times,
app version, OS, and architecture. Properties were fixed labels and coarse
buckets; the Claude diagnostic had nine additional optional fields.

| Current event                            | Actual trigger and dimensions                                                                                                                                                                                                                               | What it answers and misses                                                                                                                                                         |
| ---------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `antiburn.app_launched`                  | Shell setup; no event-specific properties.                                                                                                                                                                                                                  | Reports launches, including unattended startup. Does not measure visits, continued use, or a new installation.                                                                     |
| `antiburn.onboarding_step_viewed`        | `OnboardingSession.noteOnboardingStep`; four fixed steps, once per step per flow instance.                                                                                                                                                                  | Shows setup progress. Has no new-versus-restarted flow distinction or failure reason.                                                                                              |
| `antiburn.onboarding_finished`           | After the finish command saves settings. Explicit setup restarts can emit another completion.                                                                                                                                                               | Shows completed setup, not first useful data or unique new installations.                                                                                                          |
| `antiburn.scan_completed`                | Full discovery pass; first outcome or changed count bucket relative to the previous reported outcome. Scoped passes emit nothing.                                                                                                                           | Shows coarse discovery health and inventory. Is not an interaction or a scan-attempt counter.                                                                                      |
| `antiburn.setting_toggled`               | Saved changes to `live_usage`, `notifications`, `launch_at_login`, `tray_icon`, `dock_icon`, `discovery_paused`, or `include_non_repo_folders`; key only.                                                                                                   | Shows use of seven controls. Does not show direction, current adoption, other settings, or success of OS integration.                                                              |
| `antiburn.session_opened`                | Activity-card handler before analysis loads; agent category and native/WSL.                                                                                                                                                                                 | Measures list-to-detail intent. Does not establish that detail loaded, or cover related sessions, subagents, or newer/older navigation.                                            |
| `antiburn.error_occurred`                | Full scan failure, with `scan_failed`; repeated identical outcomes suppressed.                                                                                                                                                                              | Shows some discovery failures. Misses scoped failures and other feature failures; cannot supply an operation failure rate.                                                         |
| `antiburn.unrecognized_records_observed` | Nonempty unknown-record summary returned to Settings Insights; changed category/count bucket within the process. Clean results reset suppression without an event.                                                                                          | Diagnoses reader-selected cohorts. Does not count all Insights visits or population parser failure rates.                                                                          |
| `antiburn.claude_limit_reset_observed`   | Changed Claude reset diagnostic after normal usage refresh, with a separate five-minute cooldown and additional enablement gates.                                                                                                                           | Answers a narrow provider experiment question. Does not establish that anyone viewed usage or used a reset.                                                                        |
| `antiburn.quota_incidents_observed`      | Settings → Insights report with an assessed quota-pressure section; fires per limit kind, only when that kind's bucketed hit count changes within the process.                                                                                              | Diagnoses reader-selected cohorts read afterward. Does not count incidents at ingest time; see `antiburn.provider_incidents_ingested` below for that.                              |
| `antiburn.provider_incidents_observed`   | Settings → Insights report with an assessed provider-incidents section; fires per incident kind, only when that kind's bucketed hit count changes within the process.                                                                                       | Diagnoses reader-selected cohorts read afterward. Does not count incidents at ingest time; see `antiburn.provider_incidents_ingested` below for that.                              |
| `antiburn.provider_incidents_ingested`   | The durable evidence worker publishes a session whose evidence gained an incident within the last two hours that its previously published evidence did not carry; fires per incident kind present, content-deduplicated against the previous evidence blob. | Counts incidents at ingest time, independent of whether a reader ever opens Insights. Counts installations that ingested a failure, not requests; a closed laptop reports nothing. |

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
3. **The analytics session is an interaction-activity window.** User-oriented
   events refresh its 30-minute timeout; background scans and diagnostics do
   not. The exhaustive event classifier in `EventName::is_user_oriented` makes
   each event's session effect explicit. Silence can split one process run,
   while app restarts always start a new ID. Neither session count nor its
   first-to-last timestamp measures engaged visits or time spent.
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

The foundation documentation change corrected those disclosures. The implemented catalog
implementation status is separate from these historical findings.

## Measurement definitions

Use reporting installations, not people. Installation IDs rotate after 30 days
and reset after opt-out/re-enable. Do not add a stable identifier or use IP,
user-agent, or device information to join rotations.

| Question                                           | Definition after the first implementation                                                                                                                                                                                                        | Decision supported                                               |
| -------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------- |
| Are people deliberately returning?                 | Daily/weekly distinct `anonymousId` with a user-initiated core `surface_viewed` event or `settings_pane_viewed` for `checks`. Exclude setup, other Settings-only visits, automatic restores, nudges merely appearing, and all background events. | Whether the core utility earns repeat attention.                 |
| Does setup lead to visible value?                  | Among distinct IDs completing a new setup flow, fraction that see a core surface with `ready` data within 24 hours of completion. Also report empty, error, and timeout outcomes.                                                                | Whether to improve setup or the first data experience.           |
| Which features get used?                           | Distinct IDs viewing each core surface divided by engaged reporting IDs in the same interval and supporting versions/platforms. Separate ready-data reach from view reach.                                                                       | Which surfaces merit investment or improved discovery.           |
| Do users return after value?                       | Among IDs first reaching ready data after new setup, observed return on day 1 and day 7 using deliberate core views. Include only cohorts whose full observation window has elapsed.                                                             | Whether activation translates into observed repeat use.          |
| Where does the experience fail?                    | Per surface, fraction of reporting exposed IDs with empty, error, or loading-timeout states. For explicit operations, compare terminal outcomes with attempts separately.                                                                        | Which reliability issues block adoption.                         |
| Does passive monitoring remain enabled and useful? | Report HUD/meter enablement, successful automatic exposure, data availability, and deliberate detail use separately.                                                                                                                             | Whether passive display features are configured and functioning. |

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
with a 30-minute inactivity gap. Background events do not refresh the wire
`sessionId` timeout, but they can still appear inside an interaction session.
Ignore background events when constructing visits. Segment `sessionId`-based
reports at the first shipping app version with the user-oriented timeout;
earlier builds refresh the timeout on every captured event.
Do not infer attention duration from gaps.
Neither tray visibility nor a persistent HUD proves that someone looked at it.

## Event additions and proposals

All names below use the `antiburn.` prefix. The implemented dimensions are
implemented closed vocabularies, not arbitrary strings or permission to upload
payloads. Use event-specific Rust types even when serializing into existing
`label`, `detail`, and `bucket` fields. Update disclosures for new meanings even
when the wire field count stays unchanged.

### Visible use and value (implemented)

| Event/change                                       | Trigger and safe dimensions                                                                                                                                                                                                         | Owner and volume rule                                                                                                                                                                                                                                                          |
| -------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `surface_viewed`                                   | Successful reveal or visible navigation. `label`: `activity`, `session_detail`, `provider_preview`, `checks_preview`, `hud`, `hud_detail`, `settings`, or `quota`. `detail`: `user` or `automatic`.                                 | Shell visibility transition plus surface controllers. One per actual transition; no event for prewarm, repeated show requests, data refresh, or hidden navigation. Only deliberate transitions qualify as engagement.                                                          |
| `settings_pane_viewed`                             | Requested pane is selected and visible. `label`: the nine Settings pane IDs.                                                                                                                                                        | `SettingsWindowSession`, including first opening and external pane requests. One per visible pane transition, with duplicate requests suppressed.                                                                                                                              |
| `step_settings_viewed`                             | A progress step's settings become visible in the first-run takeover's "Show settings" disclosure. `label`: `agents`, `limits`, `sessions`, or `checks`; `detail`: `first_run`.                                                      | `StepSettingsDisclosure`. One per exposure (one open), not per re-render. `limits` only ever reports `first_run`.                                                                                                                                                              |
| `surface_state_observed`                           | Data state presented on a visible surface. Same surface vocabulary; `detail`: `ready`, `empty`, `error`, or `loading_timeout`. Ready means a usable payload, not merely a mounted component or successful IPC response.             | Surface controllers after both visibility and data readiness. At most once per distinct state per surface exposure; ignore stale asynchronous results. Use a documented 10-second visible initial-load timeout, canceled when hidden; later ready data can still emit `ready`. |
| `live_usage_state_observed` | A provider state is presented on Activity, a provider preview, or a user-opened HUD. `label`: `anthropic`, `openai`, or `google`; `detail`: `fresh`, `stale`, `authentication`, `login_recovering`, `rate_limited`, `unavailable`, `no_credentials`, `sign_in_required`, or `not_signed_in`. | Map existing presentation states without an analytics-only provider request. Deduplicate each provider/state within a deliberate visit. `login_recovering` means refresh may recover without a new sign-in. `no_credentials` means Claude Desktop is installed but Claude Code and Pi have no login. `sign_in_required` means a login exists but needs `/login`; `not_signed_in` means Claude Code is installed with no login. Send no account, plan, balance, quota value, or raw response. |
| `onboarding_started`, extend `onboarding_finished` | Visible start or resume and committed completion of a setup flow. `label`: `new` or `restart`.                                                                                                                                      | Emit a start on the first visible start or resume in each app process. A quit and later resume emits another start with the persisted classification. Emit completion once per pending-to-complete transition. Preserve the four existing step events.                         |

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

### Adoption and actions (proposed)

| Proposed event                                | Trigger and safe dimensions                                                                                                                                                                                                                                                                                                            | Owner and volume rule                                                                                                                                                                                                                         |
| --------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `setting_changed`                             | Successfully saved change; allowlist product toggles such as live usage, notifications, discovery, launch at login, app presence, and HUD enablement. Fixed `enabled`/`disabled` state.                                                                                                                                                | The relevant persistence boundary. Emit only an actual transition. Keep existing `setting_toggled` semantics stable while migrating dashboards; do not count both as separate actions.                                                        |
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

### Targeted diagnostics and measurement quality

#### Contextual Sessions filters (implemented 2026-09-22)

The product question is which filter facets reporting installations use and
remove. Measure distinct reporting installations with each action, divided by
reporting installations with a user-origin `activity` surface view in the same
interval and supporting app versions. Use action distributions to improve the
menu's organization and filter discoverability. This does not measure the
current saved configuration, the size of a result set, or combinations of
selected agents.

The activity controller emits `antiburn.session_filters_changed` once when a
deliberate gesture changes the canonical filter state. This records intent;
a settings write failure does not turn it into a persistence-success event.
The closed `label` values are `agent_added`, `agent_removed`, `agents_all`,
`result_failing`, `result_passing`, `result_all`, `spend_notable`,
`spend_material`, `spend_all`, and `cleared_all`. Only agent add/remove may carry
`detail`, using the existing known AgentKind vocabulary. Unknown agent slugs
remain local and omit `detail`. The Rust boundary rejects unknown actions and
agent values, drops agent detail for non-agent actions, and rejects extra fields.

Clear all and All agents each emit at most one event for the gesture, regardless
of chip count. Chip removal uses its facet's reset action or `agent_removed`.
Explicit search navigation that changes the facets records one filter gesture.
The action compares the previous and destination facets; an unchanged agent
selection does not override a changed result or spend action. Multi-facet
navigation uses clear-all first, then agent changes, result changes, and spend
changes. Agent detail identifies a single added or removed agent, never an
unchanged selection.
No-op reselection, automatic restoration or migration, history navigation, remounts,
hidden-window updates, settings replies, and refreshes emit nothing. Settings
writes retain gesture order; queued saves and failures do not emit retries.
There is no background filter event. Existing consent, environment-disablement,
endpoint, queue, and retry gates apply. Opted-out, unconfigured, and undelivered
installations remain unobserved.

Use the first shipping app version of this event as its reporting boundary.
Keep legacy `antiburn.session_filter_selected` reports separate: they measure
exclusive sidebar choices and cannot establish multi-agent or combined-facet
use. No selected-agent arrays, raw serialized filters, session identifiers,
titles, repository paths, result counts, cost values, or thresholds are sent.
No new wire properties are added.

Remote-source filtering extends this event at the first app version with remote
sessions. Its additional closed actions are `source_all`, `source_local_added`,
`source_local_removed`, `source_remote_all_added`, `source_remote_all_removed`,
`source_remote_host_added`, and `source_remote_host_removed`. They carry no
`detail`, host IDs, names, aliases, selected arrays, counts, or filter snapshots.
The existing deliberate canonical-state-change boundary and exclusions apply.
A mixed parent becoming fully selected records one remote-all addition;
removing a source chip records its corresponding removal. Removing a grouped
host chip records one remote-all removal and preserves Local. Opening a host
from Settings records a host addition when it changes the selected source. Inventory loading,
host removal reconciliation, wildcard expansion, migration, and history restores
do not record filter use. Measure source-action reach among reporting Sessions
viewers to evaluate filter discoverability; it is not a count of remote hosts or
successful syncs.

Validation covers the real controller mutations, canonical no-ops, legacy and
versioned restore, agent add/remove and resets, rapid saves, and save failure.
Rust tests cover every closed action, known/omitted agents, forbidden extra
fields, unknown values, and allowed event facts. Existing enabled analytics
tests cover consent and delivery gates.

#### Legacy Sessions sidebar filter (implemented 2026-09-12)

The following contract applies to earlier releases. The contextual controls
emit only `antiburn.session_filters_changed`.

The product question is which Sessions sidebar filters readers actually use,
and whether harness filters concentrate on one agent. The metric is the
distribution of `antiburn.session_filter_selected` by `label` (and, for the
`agent` label, by `detail`), using reporting installations as the denominator.
This supports deciding whether the fixed filter set earns its sidebar space and
whether a harness filter is worth the row it takes.

The owning boundary is the setter behind the sidebar selection (in the main
window's activity controller). It fires only when the persisted filter
actually changes — never when the window loads and restores the persisted
selection, and never twice for a click that reselects the current filter. The
closed `label` vocabulary is `notable`, `material`, `agent`, `failing`,
`passing`, `all`. `detail` carries the selected harness slug only when
`label=agent`, and only when the harness is one of antiburn's fixed agent
enum's values; an unrecognized harness — a slug newer than this build's
agent list — reports `label=agent` with no `detail` rather than widening the
vocabulary or dropping the event. No session id, title, repository, count, or
cost travels with it.

Validation: a test resolves the interaction for a known agent (`detail` set)
and for a non-agent filter and an unrecognized agent (both with no `detail`),
and a frontend test confirms the setter is a no-op — no persistence write, no
event — when the requested filter matches the current one, including the
initial load path that only restores the persisted value.

#### Limit-factor plan mapping (implemented 2026-09-11)

The product question is whether learned factor accuracy differs across the
provider plans that the Usage surface already identifies. The metric is the
distribution of `antiburn.limit_factor_observed` by provider, lane, and mapped
plan, using reporting installations as the denominator. This supports plan
mix and estimate-quality decisions without sending account or billing data.

The factor learner owns the trigger. It passes the plan and tier from the
latest learned point to the analytics boundary. The event carries only the
closed `plan` values `free`, `pro`, `max`, `max_5x`, `max_20x`, `team`,
`enterprise`, `plus`, `prolite`, `business`, `edu`, `unknown`, and `other`.
Claude's Max tier values and OpenAI's Pro Lite value require their matching
provider; unknown or absent input uses the existing coarse fallback.

The existing first-per-provider/lane rule and 24-hour minimum remain in force.
The deduplication tuple includes the mapped plan, so a tier change is eligible
under the existing changed-tuple rule. Background learning keeps its current
cadence and maximum volume. Historical coarse values cannot be reclassified;
reports segment the refined mapping at its first app-version boundary.

#### Model-scoped weekly lane detail (implemented 2026-09-16)

The factor learner now tracks a lane for a provider's supplemental weekly
window scoped to one model, such as Claude's "Fable" limit, alongside the
existing five-hour and weekly account lanes. The product question is whether
this app's estimate accuracy for a model-scoped lane differs from the
account-wide ones, without ever naming the model itself.

`antiburn.limit_factor_observed`'s `detail` property gains one closed value,
`model`, reported for every `model:<slug>` lane the learning pass touches; the
model name and its slug never reach the event. The existing `short` and `long`
values are unchanged. The first-per-pair rule, the 24-hour minimum between
events for the same pair, and the plan and band mappings all apply to the
`(provider, "model")` pair itself: every model-scoped lane for one provider
maps to the single detail value `model`, so all of a provider's model lanes
share this one dedup slot, and the model name is never part of the key.
Reports segment `detail=model` from `short` and `long` at this app version's
boundary; no historical event can be reclassified.

#### Burn Checks integration (implemented 2026-09-10)

| Product question and decision                                                                                                                                                            | Metric and denominator                                                                                                                                                                                                   | Trigger and closed fields                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Suppression and maximum volume                                                                                                                                                                                                                                                                                                                                              |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Does deliberate Burn Checks use reach usable results? Improve discovery or report reliability.                                                                                           | Distinct reporting installations with `burn_checks` `ready`, `empty`, `error`, or `loading_timeout`, divided by installations with a user-origin `burn_checks` `surface_viewed`, on app versions from 2026-09-10 onward. | The main-window section is both selected and natively visible. `surface_viewed` uses `label=burn_checks`, `detail=user`. `surface_state_observed` uses `label=burn_checks`, the four existing state values, and `origin=user`. Ready requires a finding or clean result; an all-unassessed report is empty.                                                                                                                                                                     | Hidden sections, stale results, remounts, and refreshes do not create exposures. Each state occurs once per exposure. Maximum: one view and four state events per exposure.                                                                                                                                                                                                 |
| Do readers proceed from Auto Fix review to confirmation, and what result follows? Improve review copy or operation reliability.                                                          | Confirmations divided by `burn_check_auto_fix_reviewed` with `detail=ready`; each typed completion outcome divided by confirmations. Allow in-flight and late-delivery time before finalizing.                           | A Fix request completes with review `detail`: `ready`, `stale`, `expired`, `conflict`, `unavailable`, or `failed`. Apply selection emits `burn_check_auto_fix_confirmed`. Completion `detail`: `applied_awaiting_verification`, `applied_verification_unavailable`, `recovery_needed`, `stale`, `expired`, `conflict`, `unavailable`, or `failed`.                                                                                                                              | Busy controls suppress duplicate concurrent requests. Each explicit retry is a new attempt. Maximum: one review event per Fix request, one confirmation and one completion per Apply request. No automatic retry emits another event.                                                                                                                                       |
| Does prompt preparation reach a usable clipboard result? Improve prompt preparation or clipboard handling.                                                                               | Successful `burn_check_prompt_copied` divided by `burn_check_prompt_prepared` with `detail=ready`. Report preparation failures separately.                                                                               | Each accepted explicit Copy fix prompt click prepares fresh text and emits one terminal preparation `detail`: `ready`, `stale`, `expired`, `unavailable`, or `failed`. Native clipboard write success emits `burn_check_prompt_copied` without properties. Neither event carries a check label.                                                                                                                                                                                 | Busy controls suppress concurrent requests. An internal stale-data retry remains one click and emits no extra preparation. Clipboard failure emits no copy event. Each later click prepares fresh text, even after failure or success. Maximum: one preparation and one successful copy per accepted explicit click.                                                        |
| Do later Passed transitions and recurrences become visible, and did their independent watch start passively or from an action? Improve verification coverage without claiming causation. | Distinct reporting installations with each `(detail, origin)` tuple among deliberate Burn Checks exposures. Use exposed installations as the denominator. Never use background transitions as visits.                    | `burn_check_outcome_observed`: `detail=verified` originates only from a backend active win for a visibly Passing, unsnoozed detector. The backend win is an exact current fixed cycle with a fixed result and verified boundary. `detail=recurred` records a visible regression from a deliberately expanded, unsnoozed target and does not require that detector to remain Passed. `origin=passive` or `action` describes how that watch started and does not claim causation. | Background evaluation, hidden sections, snoozed checks, unloaded target rows, and refresh callbacks emit nothing. Deduplicate the coarse `(detail, origin)` tuple per exposure, without sending target or remediation-cycle identifiers. Multiple active cycles with the same tuple collapse to one event. Maximum: four events per exposure, independent of finding count. |

These events use only the existing `detail` and `origin` wire fields. They never
include prompts, resource or model strings from work, paths, finding, action,
watch, reference, or sample IDs, exact token or cost values, private errors, or
evidence revisions. Product reports must segment at the 2026-09-10 app-version
boundary and show reporting installation counts.

Segment prompt counts at the first shipping app version with fresh preparation
per click. Earlier versions use different retry and repeat rules. Copy events
measure native clipboard success, not prompt use, verification, or savings.

The target-based M/B/K flow needs no new event or property. Existing surface,
Auto Fix, prompt, and outcome events measure the same user actions without
resource names, kinds, estimates, scopes, paths, definitions, or session IDs.

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

The second background exception is `antiburn.provider_incidents_ingested`. It
answers: "When a provider degrades, is a given user's failure part
of a wider outage?" The metric is distinct reporting installations per
`(agent, incident kind)` per hour, compared against installations that sent
any event that hour; the decision it supports is telling a reader in-app and
telling support, during a live incident, whether a capacity or server failure
is fleet-wide or local to them — something the report-time
`quota_incidents_observed` and `provider_incidents_observed` events cannot do,
because they fire only when a reader happens to open Settings → Insights,
possibly days later. The trigger and owning boundary is a background
completed outcome: the durable evidence worker publishes a session's evidence
(`insights_worker::apply_outcome`, the `PassOutcome::Published` arm) — never a
per-record, per-poll, per-scan, or UI-driven trigger. Its properties are
`label` (the agent that recorded the session, the same fixed list
`session_opened` uses), `detail` (the incident kind — `capacity`,
`server_error`, `connection`, `rate_limit`, or `usage_limit`), and `bucket`
(the bucketed count of newly reported incidents for that pair in this
publish); it carries no model name, session id, timestamp, path, or message
text. Two mandatory filters bound its dedup and volume: an incident is new
only when its `(ts_ms, kind)` pair is absent from the session's previously
published evidence (so a revision-bump re-ingest of unchanged evidence
reports nothing, and file growth reports only the appended incidents), and it
must fall within `INGESTED_INCIDENT_FRESHNESS_MS` (two hours) of the worker's
publish clock (bounding first-install backfill, a parser reclassification of
old records, and an unreadable previous evidence blob). There is no
in-memory suppression map — dedup is entirely by content — and the maximum
volume is five events per publish, one per kind present. The owner is Dave
Slutzkin; review date 2026-12-14, to confirm whether the signal is used in a
fleet status view or a support workflow, or should be removed. Validation
drives the real `apply_outcome` path against a store and asserts its computed
result, the same boundary `insights_worker::process_next_work` hands to
`analytics::record_provider_incidents_ingested` — this codebase has no
`tauri::AppHandle` test harness, so, like `record_quota_incidents` and
`record_provider_incidents` before it, the `allowed(app)` gate itself is
exercised by inspection rather than by an app-driven test.

This event counts installations that ingested a failure, not requests: a
closed laptop, or an installation that never re-scans the affected session,
reports nothing even during a real outage. The two-hour freshness window
means a late ingest can report up to two hours after the actual incident, and
opted-out installations are unobserved. Segment reports at the app version
this event ships in.

The implemented delivery changes add bounded queue-depth-triggered draining, protected retry backoff, and
a request budget. Before expanding volume further, simulate normal repeated
visits, preview use, and an offline backlog against the 50-event drain and
500-row queue. Preserve the opt-out recheck before each request and silent
failure. Do not merely increase queue size or add a blocking exit flush. Monitor
delivery lag, duplicate message IDs, rejection counts, and event mix in the
collector separately from product engagement.

## Remote sessions measurement

The product questions are whether reporting installations can connect a host,
whether sync continues to work, and whether Source filtering is discoverable.
Compare the distribution of connection-check outcomes among reporting
installations that make a check. Compare scan outcomes by manual/automatic
origin among installations with remote sync outcomes. Use persisted host-change
events to measure configuration actions and Source gestures among Sessions
viewers to measure filter use. These metrics guide setup guidance and reliability
work; they do not count people, hosts, SSH attempts on the wire, or live remote
activity. Segment at the first app version that ships these events.

The backend owns three completed-outcome boundaries:

- `remote_host_connection_checked`: one result for each explicit check or
  preflight required by add/alias change. Its closed labels are `ready`,
  `authentication`, `host_key`, `helper_missing`, `incompatible`, `timeout`,
  `connection`, `invalid`, and `storage`. Timeout/storage are reserved values;
  current unavailable-SSH failures map to connection. Checking and then saving
  may legitimately perform two checks; this is not a unique-host funnel.
- `remote_host_changed`: `added`, `edited`, `removed`, `sync_enabled`, or
  `sync_disabled` only after durable
  success, with configured-host count in the existing bucket. A canceled form,
  rejected request, failed write, or refresh emits no configuration success.
- `remote_sync_completed`: `succeeded` or `failed`, `manual` or `automatic`, and
  the cached-session-count bucket after a current scan terminates. Removed,
  canceled, or superseded generations do not report a completed outcome. A
  failed scan can retain previously cached sessions; its bucket is not an
  imported-session count.

Connection and configuration volume is bounded by explicit operations. The
single-flight scheduler coalesces queued scans; automatic runs follow the
selected completion-based interval (at least one minute per host). Repeated
polls, event subscriptions, renderer remounts, and hidden-window refreshes emit
nothing. Background sync can emit while Settings is closed, so exclude it from
engagement and visit metrics. Retries are new completed operations; delivery
retries retain normal event-ID deduplication.

Source filter actions follow the canonical gesture contract above. Session-open
events add a closed `remote` environment alongside `native` and `wsl`. No host
ID, alias, address, user, path, transcript, selected-source array, exact count,
or raw error is serialized. No new envelope fields are added. Existing opt-out,
unconfigured-build, environment-disablement, queue, and delivery rules remain
unchanged. Validation must exercise the actual operation boundaries and their
failure/cancellation branches as well as the closed wire schema.

### Claude login refresh (implemented 2026-10-09)

Question: does delegating the Claude token refresh to the Claude CLI keep
Claude usage readable, and how often must a reader sign in again? Decision: keep,
change, or remove the background and pre-expiry refresh, and tune its
cooldowns. Metric: the share of reporting installations with a
`refreshed` outcome among those with any `antiburn.claude_login_observed`
event, split by `detail`; and the share with `sign_in_required` or
`cli_missing`. `keychain_metadata_failed` and `keychain_secret_missing` count
installations where the macOS Keychain read was not reliable.

The shell emits the event after a live-usage collection, from fixed outcome
values the Claude source keeps. Only a changed `(label, detail)` pair is
queued, so a stable state reports once per run. The values carry no token,
path, account identifier, or CLI output; the Rust boundary drops any value
outside the closed lists (`claude_login_observations_use_closed_vocabularies`).
The existing `live_usage_state_observed` event also accepts the renderer
states `sign_in_required` and `not_signed_in`. Owner: the live-usage
maintainer. Review this diagnostic by 2027-01-31.

## Event review contract

### Model-based check enablement and provider operations

The native `ModelCheckLifecycle` contract keeps the existing
`antiburn.ignored_instruction_lifecycle` wire name for master enablement and
decision-provider operations. `DecisionProvider` accepts only `jev`,
`cloudflare`, `ollama`, and `custom`. It does not carry endpoint, model, account,
or credential identity. The renderer `Interaction` union does not accept this
native lifecycle event. See the [public catalog](analytics.md#model-based-check-enablement-and-provider-operations).

| Product question and decision                                                                      | Metric and denominator                                                                                                                                                                                              | Completed boundary and owner                                                                                                                                                       |
| -------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Do readers deliberately enable or pause model-based checks? Improve control placement.             | Reporting installations with saved master transitions, split by enabled/disabled. This measures transitions, not all current enabled installations.                                                                 | Native settings persistence emits `label=enablement`, `detail=enabled` or `disabled` after a changed value is saved.                                                               |
| Can readers save, select, remove, and test decision connections? Improve setup reliability.        | Reporting installations with each provider outcome. Test success fraction is successful settled explicit tests divided by all settled explicit tests, by provider. Saves are not tests or selections.               | Native `jev/settings.rs` commands emit a provider label and `provider_saved`, `provider_switched`, `connection_removed`, `credential_removed`, `test_succeeded`, or `test_failed`. |
| Do generic fix-prompt actions reach preparation and clipboard success? Improve action reliability. | Successful `burn_check_prompt_copied` relative to `burn_check_prompt_prepared` with `detail=ready`. Report other preparation outcomes separately and allow late delivery. No per-check classification is available. | Existing deliberate prompt handlers emit preparation once per accepted explicit click and clipboard success at most once for that click. Neither event carries a check label.      |

Emit saved transitions only after durable success and an actual change. Failed
saves, no-ops, startup restoration, migration, and Settings refresh emit no saved
transition. Read-only limit refresh emits no provider lifecycle event. Emit one
outcome per explicit settled test, including validation or
credential failure after the command starts. Internal transport retries are not
extra tests. Cancellation or process exit can leave an outcome unobserved.
No analytics-only request is allowed. The provider controls under **Model-based checks** select Jev, Ollama,
Cloudflare, or Custom, subject to each route's verified capabilities. Selecting
an unsaved draft and refreshing its limits are not saved transitions. Both
Custom response modes share the existing `custom` category. These events do not
measure capability admission or prove that a YAML assessment ran.

`provider_saved` records a durable connection create or edit. Editing an inactive
connection does not select it. `provider_switched` records an explicit saved
connection selection. Creating the first connection selects it without a separate
switch event or an enablement event. `connection_removed` records a durable
profile removal, not key revocation. Removing the selected connection also saves
master disablement and clears selection; emit its changed enablement outcome at
that durable boundary. Removing an inactive connection does not emit disablement.
Canceled confirmations, stale revisions, missing profiles, failed removals, and
readback requests emit no removal success. Shared credentials can remain after
profile removal. Only explicit credential removal uses `credential_removed`.
The event carries no connection ID, revision, profile count, or selected state.
Removal totals measure completed operations, not a removal success rate: canceled
and failed removals have no setup outcome. Segment releases with separate Save
and Use controls from earlier releases where Save and use also changed selection.
Do not infer connection selections from `provider_saved` in the current contract.

The four native-check labels, finding/evidence observations, assessment outcomes,
and history-window/run events are retired from the runtime contract. They are
not aliases for YAML definitions. Generic prompt events carry no `label`;
preparation retains `ready`, `stale`, `expired`, `unavailable`, and `failed`, while
copy has no event-specific properties. Each accepted explicit click prepares
fresh text and reports one terminal preparation result, including a click after
clipboard failure or copy success. Internal stale-data retries emit no extra
event. Native clipboard write success emits one copy event for that click. A
copy does not prove execution, repair, or savings.

Exclude provider setup/tests and saved enablement from engagement, visits, and
retention. They measure operations, not assessed sessions, model accuracy, or
finding prevalence. Show distinct reporting installation counts. Segment at the
first release that removes the native four-check contract; historical tuples
cannot be reclassified. Missing delivery and opted-out installations remain
unobserved. Only closed `label`/`detail` values are sent, without work content,
paths, URLs, model names, keys, private errors, or work identifiers.

Enabled-feature tests must cover the exact remaining serialized tuples, reject
retired forms and extra fields, and exercise saved transitions, failed saves,
no-op suppression, test validation/credential failures, and generic prompt retries
at their owning boundaries. Payload acceptance alone does not prove emission.

### YAML input fitting and definition opens (implemented 2026-10-16)

The existing Burn Checks surface view/state events measure workspace reach and
visible usable results. Generic prompt-preparation and decision-provider
lifecycle events describe different actions. They cannot establish whether an
authored YAML input fits a local request budget or whether opening its definition
works. The two native completion events below fill those gaps without classifying
arbitrary authored checks. There is no new diagnostic-exposure event: reading a
report or refreshing it does not prove that a diagnostic was visible.

| Product question and decision                                                                                                                              | Numerator and denominator                                                                                                                                                                                                                                                                                                                                                                  | Owner and completed-outcome boundary                                                                                                                                                                                                          |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Does input fitting let eligible candidates reach a ready request, and which local failures block it? Improve fitting guidance and preparation reliability. | Ready fraction: `unchanged` plus `shortened` events divided by all `burn_check_input_prepared` events. Shortened share: `shortened` divided by ready events. Report each failure category over the same all-outcome denominator and show distinct reporting installations.                                                                                                                 | The shared engine calls `DecisionEvaluator::preparation_recorded` only after a new preparation write returns current. The installed desktop evaluator maps the record to one closed outcome and records `antiburn.burn_check_input_prepared`. |
| Can readers open a check definition to address a diagnostic? Improve file-action discovery and native-open guidance.                                       | Accepted-open fraction: `succeeded` divided by all `burn_check_definition_opened` completions. Report each failure category separately. For action reach, use distinct reporting installations with a completion divided by deliberate Burn Checks viewers on supporting versions. A completion from a popover or an earlier exposure can fall outside that view cohort; keep it separate. | `installed_checks/opener.rs` owns `antiburn.burn_check_definition_opened` after the explicit command settles, including reference, access, and worker failures before native open.                                                            |

Both events serialize only `{ "detail": "<closed outcome>" }` in addition to
the standard envelope and architecture. `label`, `bucket`, `origin`, and every
other optional property are absent. The native outcome types accept no authored
name, arbitrary diagnostic code, source reference, path, input, token value,
omission count, YAML value, provider response, or private error. The renderer
interaction union rejects both event kinds.

Preparation outcomes are `unchanged`, `shortened`, `input_too_large`,
`invalid_fit`, `input_unavailable`, `resource_limit`, `invalid_input`, and
`invalid_request`. Ready records use `shortened` when any decision's omission
report is nonempty. The event describes the complete prepared set once per
candidate, not once per decision or question. Typed `NoFit` becomes
`input_too_large`; typed `PreparationResourceLimit`
becomes `resource_limit`. Neither is a provider context rejection. Invalid fitting,
input-expression, and deterministic request failures stay distinct. Required-input,
provider-preparation, and incompatible-profile unavailability map to
`input_unavailable`. A known false condition is skipped and emits nothing.
Recovery profile checks do not create new preparation records or events.

Decision-free local execution is outside this request-fit denominator. Its typed
local preparation maps to no input-prepared event, and the local engine path does
not call the model preparation observer. Existing visible workspace events can
show local result reach but cannot separate local starts or failures from model
assessments. Keep that measurement limit explicit; do not count local work as
unchanged provider input or create analytics-only provider work.

The maximum preparation volume is one event per newly committed non-skipped
candidate, independent of its decision count, question count, target count, or
reduction turns. This is background
reliability measurement even while the window is hidden. It never refreshes the
analytics session timeout or enters engagement, activation, visits, retention, or
time-spent counts. Discovery, polls, coalesced candidates that never prepare,
failed preparation writes, stale claims, saved-request retries, recovery, and
report refreshes add no event. A later dispatch-intent or local-settlement write
failure preserves the preparation and does not emit again on reuse. Later
cancellation can follow a valid committed preparation. A crash between preparation
commit and event capture is an accepted miss; opt-in does not backfill saved work.
There is no new hourly cap, persistent deduplication table, or analytics-only work.
The existing 500-row queue can drop earlier events during candidate bursts; this
can bias outcome distributions. Delivery retries deduplicate by `messageId`.

Definition-open outcomes are `succeeded`, `missing`, `unavailable`, and
`open_failed`. Missing covers an unknown/stale reference or a removed file.
Unavailable covers rejected local-file/path/access checks or unavailable folder
settings. Open failed covers the native opener and blocking-worker failures.
One settled command emits exactly one completion. The UI prevents duplicate
in-flight presses, and each explicit retry is a new attempt; there is no internal
retry event. Remounts, discovery, automatic restoration, and report refreshes do
not open files. Hiding the window does not cancel an already started request.
Process exit before settlement can leave it unobserved. These deliberate actions
refresh the session timeout but do not establish a deliberate core view by
themselves. OS acceptance does not prove editor visibility, file editing, repair,
or a subsequent successful assessment.

Rates use observed completed outcomes, not all candidates, distinct checks,
provider calls, true model-context failures, accuracy, or people. Reports use
capture time, deduplicate delivery by `messageId`, allow late arrivals, and segment
by app version and OS starting with the first release that ships these producers.
Older prompt-preparation and fixed-check events cannot be reclassified. Unconfigured,
environment-disabled, opted-out, crashed-before-capture, and undelivered cases
remain unobserved. There are no stable work identifiers to join preparation and
open events or attribute a later result to an open action.

Validation covers exact serialized properties for every outcome, typed failure
mapping with content-rich synthetic records, native-only IPC rejection, catalog
agreement, and user/background session classification. Real engine/SQLite tests
exercise failed preparation writes, later settlement/intent write failure,
recovery without duplicate observation, stale claims, and missing input before
the preparer runs. Native-open tests exercise issued-file resolution, missing and
unissued files, rejected cache/access states, opener success/failure, and one
completion per request. Shared capture-gate tests cover disabled consent,
unreadable consent, environment opt-out, and missing configuration. Queue cleanup
includes both new event kinds. The Tauri `AppHandle` adapters are reviewed at
their call sites; tests inject the OS opener and event sink without launching an
application or making provider requests. The public catalog in
[analytics.md](analytics.md#yaml-input-fitting-and-definition-opens) names the same
boundaries, vocabularies, and limits.

### Check folder opens (implemented 2026-10-27)

Product question: can readers open a discovered check folder from Settings?
Use the result to improve folder-action discovery and native-open errors.

Measure `succeeded` divided by all `burn_check_directory_opened` completions,
with `missing`, `unavailable`, and `open_failed` reported separately. Show
distinct reporting installations and segment at the first supporting app version
and OS. The denominator is observed settled explicit requests, not all Settings
views, folders, files, or people. A completion does not prove a file edit, a
successful check, or visible Finder content.

The native installed-check directory command records one outcome after the
request finishes, including validation failures before the native opener runs.
Validation covers the issued reference, current repository membership, grants,
canonical directory binding, and remote-cache exclusion. The event serializes only
`{ "detail": "<closed outcome>" }` with the standard envelope. It contains no
path, reference, name, count, input, model, cost, or private error. Renderer
interaction input rejects it. It does not reuse the definition-open event.

Maximum volume is one completion per settled explicit request. The UI suppresses
same-folder concurrent presses; each later explicit retry is another request.
Reads, passive discovery, search, dialog opening, and disclosure expansion do not
emit it. There is no analytics-only filesystem or provider work. A hidden window
can finish a started open. A crash before completion, opt-out, absent endpoint,
and delivery loss remain unobserved. Existing capture, queue, and retry gates
apply. Delivery deduplicates by `messageId`; use capture time and allow late
arrivals. Do not join this event to check outcomes with invented identifiers.

The action refreshes the analytics session timeout but does not establish a
deliberate core view by itself. Native tests inject a folder opener and event
sink. Cover all fixed outcomes, one completion, exact property serialization,
native-only IPC, user-oriented classification, and consent/queue gates without
opening a file manager or making provider requests. See the
[public catalog](analytics.md#check-folder-opens).

### Legacy Ignored Instructions tuples

This section is historical measurement context only. The runtime no longer
accepts or emits these observation, assessment, or history tuples.

Earlier `ignored_instruction_observed` events used `finding`, `evidence`, or
`prompt` stage labels with visible/available/copied or failure outcomes. Later
releases classified finding/evidence observations by one of four native checks
and added a check label to generic prompt events. These events measured deliberate
reader actions, not unique findings or model accuracy. Background assessments
and history outcomes never established visits or automatic adoption.

Earlier `ignored_instruction_lifecycle` events also carried `history_window`,
`backfill`, and `execution` labels or check-specific `execution_`/`backfill_`
terminal outcomes. They measured published attempts, not provider requests or
whole-session correctness. Current master enablement and provider tuples retain
the same wire name but not these retired meanings. Keep each historical app-version
boundary. Never map older check labels or unlabeled prompts to authored YAML IDs.

### Per-check enablement

The product question is which checks people deliberately keep active, so the
team can improve individual checks and the placement of their controls. Report
distinct reporting installations with `antiburn.check_enablement_saved`, split
by the closed check `label` and `detail`. Use reporting installations on app
versions that contain per-check controls as the denominator. This measures
saved transitions, not current adoption: installations that keep their defaults
or never change a check produce no event.

The persistence command owns the event. It emits once only after one check's
stored preference changes successfully and any required transition work
completes. `label` is `sessions_over_depth`, `model_overthinking`,
`overpowered_subagents`, `unused_mcp_servers`, `unused_built_in_tools`,
`unused_skills`, `old_model_usage`, `overuse_of_fast_mode`, or `cache_churn`;
`detail` is `enabled` or `disabled`. A closed Rust enum
maps every supported detector to this allowlist. No-op requests, failed or
canceled saves, automatic migration and restoration, remounts, refreshes, and
provider or master-gate changes emit nothing. Maximum volume is one event per
successful persisted transition. No finding, session, project, path, key,
provider state, source evidence, or work content is sent.

Segment reporting at the first app version that ships this event. Do not merge
it with `ignored_instruction_lifecycle`: that event measures master enablement
and provider operations, not individual deterministic-check preferences.

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

1. The source now contains the disclosure corrections, implemented events, and
   bounded delivery changes. Establish baseline collector queries by app version.
   Verify configured release ingestion, retry deduplication, and delivery delay.
   Do not claim production behavior from source alone.
2. Verify the implemented events with a loopback recording of setup → activity → preview →
   session detail → Settings Insights → return visit. Also exercise empty/error
   data, hidden prewarm, renderer recreation, automatic HUD restore, and opt-out.
   Assert the expected sequence and absence of duplicate use events.
3. Build engagement, activation, feature reach, observed day-1/day-7 return, and
   visible reliability reports using the definitions above. Show reporting
   installation counts and observation limits alongside percentages.
4. Review at least two complete weeks of supported-version cohorts, then choose
   follow-up actions based on observed gaps. Assign each report and diagnostic a
   maintainer; review whether its events still support an actual decision.

The historical audit changed documentation, current public disclosures, and
contributor expectations. The implemented catalog and the reviewed Burn Checks
integration now add runtime instrumentation with bounded delivery behavior.
Collector configuration, dashboards, and the remaining proposed work remain
described above.

### Project folder action review — 2026-09-16

Question: do readers use project folder open/copy, and which action fails?
The renderer reports each settled explicit action with closed `open`/`copy` and
`succeeded`/`failed` values. No hover event or path-derived property is collected.
Session details and Burn Check project rows share the same action reporting.
Concurrent presses are suppressed until an action settles; a later retry is a
new attempt. Full paths stay in local IPC and the clipboard or native opener.
The denominator is observed attempts per action, not all sessions or panel
views. Native opener acceptance does not prove file-manager visibility. The
existing consent and build gates apply; offline delivery and opt-out remain
unobserved. Tests cover clipboard and opener success/failure separately from
transcript actions and reject unknown analytics values and extra path fields.

### Quota screen — 2026-09-16

Question: do readers open the new Quota screen, and does it present usable
data? The main window's Quota section adds a `quota` value to the existing
`surface_viewed` and `surface_state_observed` label vocabularies, following
the same rules as every other main-window section: one `surface_viewed` per
deliberate show, and at most one `ready`/`empty`/`error`/`loading_timeout` per
distinct state per exposure. Selecting an account, lane, or range preset does
not start a new exposure; it stays the same visit. No provider, account,
lane, dollar figure, or session identity is collected. The existing consent
and build gates apply.

### Memories view — 2026-10-08

Question: do readers open the Memories audit, and does it find memories? The
main window's Memories section adds a `memories` value to the existing
`surface_viewed` and `surface_state_observed` label vocabularies, with the
same rules as every other main-window section: one `surface_viewed` per
deliberate show, and at most one `ready`/`empty`/`error`/`loading_timeout`
per distinct state per exposure. `ready` means at least one project is
listed, `empty` means none is, and `error` means the command failed. The
tracker owns `loading_timeout`. Expanding a row, collapsing a project, and
reloading on re-entry stay inside the same visit. No project path, memory
name, count, or content leaves the device. The existing consent and build
gates apply.

Question: do people act on what the audit shows them? One `memory_action`
event fires after each Reveal, Delete, Undo, or Remove line settles. The
denominator is installations with a `surface_viewed` for `memories` in the
period. Read the event by `label` and `detail`:

| `label`             | `detail` values                                         | Reading                                                                             |
| ------------------- | ------------------------------------------------------- | ----------------------------------------------------------------------------------- |
| `reveal`            | `succeeded`, `failed`                                   | How often people open a file to read it in full.                                    |
| `archive`           | `succeeded`, `failed`, `changed_on_disk`, `unsupported` | How often people delete a memory. `changed_on_disk` counts stale views, not errors. |
| `restore`           | `succeeded`, `failed`, `changed_on_disk`, `unsupported` | Undo use. A high restore-to-archive ratio means deletes are regretted.              |
| `remove_index_line` | `succeeded`, `failed`, `changed_on_disk`, `unsupported` | How often people clear a dangling index line.                                       |
| `open_from_session` | `succeeded`                                             | How often a session detail row opens a memory in the Memories view.                 |

Exclusions: `unsupported` installations (Windows) cannot act, so leave them
out of the archive, restore and remove_index_line rates. The event carries no count, path, name,
or content. Rates use action counts per viewing installation, not per memory.

### Quota window accuracy — 2026-09-18

Question: for each closed quota window, did the dollars-only estimate land
close to the provider's own meter, and how much of the meter's rise stayed
unexplained under the shared-meter model? The metric is the distribution of
`antiburn.quota_window_closed`'s `estimateBias` and `unexplainedBand` by
provider, lane, and mapped plan, using reporting installations as the
denominator. This supports decisions about factor-learning accuracy and the
shared-meter model without sending dollar amounts, exact percentages, or
account identifiers.

The existing background factor-learning pass (`usage_alerts.rs`'s scheduled
pass and `provider_usage/live/mod.rs`'s live pass) owns the trigger, the same
pass that already reports `antiburn.limit_factor_observed`. It now also names
every `(provider, account, lane)` pair it touched. For each touched pair, the
event boundary looks for windows that closed within the last 14 days, carry
an observed `period_id` (never a cadence-extrapolated or turn-gap-inferred
one), and have no durable `quota_window_reported` marker yet. This records a
completed outcome for the window, not a live estimate: the badge and forecast
a user actually saw during the window are not reconstructed or corrected
after the fact.

The event carries the same closed `label`/`detail`/`plan`/`usageBand`
vocabulary as `antiburn.limit_factor_observed`, plus three fields scoped to
this event alone: `estimateBias` (the dollars-only estimate against the
window's last reading, signed and banded by powers of five and twenty, or
`unknown` with no reading or no factor point), `unexplainedBand` (the
shared-meter model's unexplained rise as a share of the last reading), and
`readingCoverage` (whether a reading covered the window's own end, within an
hour). `bucket` reports the window's count of
meter regressions. No dollar amount, exact percentage, account, model name,
session, or timestamp reaches the event; the pure `share_period` function
that derives the shared-vs-estimated split never receives an account key,
only readings, bucketed dollars, and factor points already scoped to one
lane.

A window reports at most once, ever: the store marks it reported before the
event is handed to the tracker, so a crash, a disabled build, or a later pass
over the same lane cannot report it twice, and a window whose facts could not
be computed is left unmarked for a later retry rather than reported blind. A
window that never closes, or closes outside the 14-day lookback, never
reports. There is no cancellation path — the pass either finds the window
already marked or does not, and consent is checked once per pass rather than
per window. Expected volume is bounded by the number of quota windows an
install closes, at most a handful per lane per week, well under the existing
factor-observed volume.

This is a new event, not a reused name, so there is no prior semantic to
preserve. `docs/analytics.md` and `docs/privacy-policy.md` list the event and
its three new properties; the wire-payload test now pins thirty-one total
properties and the catalog tests (`the_documented_catalog_matches_the_code`,
`every_document_that_counts_the_fields_counts_the_same_number`,
`no_variant_escapes_the_catalog`) enforce that the code and the documented
catalog agree.

Tests prove: an open window reports nothing
(`an_open_window_reports_nothing`); a window closed outside the lookback
reports nothing (`a_window_closed_more_than_the_lookback_ago_reports_nothing`);
a disabled pass reports nothing and writes no marker
(`a_disabled_pass_reports_nothing_and_writes_no_marker`); a closed, eligible
window reports exactly once and leaves a marker row
(`a_closed_window_reports_once_and_the_marker_row_exists`); and a second pass
over the same window, with its marker already written, reports nothing
(`a_second_pass_over_the_same_closed_window_reports_nothing`). The three new
banding functions (`estimate_bias`, `unexplained_band`, `reading_coverage`)
are unit tested directly in `event.rs` against their boundary values. The
marker table itself is proven durable and idempotent
(`marking_a_quota_window_reported_is_idempotent_and_durable`) and pruned with
its period, alongside the existing residual and learn-cursor rows
(`finite_retention_prunes_the_residual_and_learn_cursor_with_their_period`).
This validates the store-level path the real pass takes; there is no Tauri
`AppHandle` mock in this codebase, so the consent-gated wrapper itself is
exercised the same way every other analytics emitter's wrapper is — by
review, not by an automated test — following existing precedent.

## Main-window navigation and search (2026-09-17)

These three events use the existing opt-out and closed-property pipeline. Segment results
at the app version that introduces this catalog; older versions have no comparable signal.

| Question and decision                                                                     | Metric and denominator                                                                                              | Trigger, owner, and exclusions                                                                                                                                                                                                                                                                   |
| ----------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Does unified history help navigation? Improve its placement and keyboard discoverability. | Reporting installations with `navigation_history_moved` among main-window viewers.                                  | Navigation store after a successful Back/Forward move, `label=back/forward`. No boundary, disabled-control, ordinary destination, external target, or automatic restore event.                                                                                                                   |
| Is global search discoverable? Improve the trigger and shortcut.                          | Installations with `app_search_opened` among main-window viewers.                                                   | Main-window open handler, once per closed-to-open user transition, no properties. No remount, repeated shortcut while open, or background event.                                                                                                                                                 |
| Which destinations does search help readers reach? Improve catalog coverage.              | `app_search_result_opened` divided by `app_search_opened`, grouped by category; also report distinct installations. | Main-window result handler after navigation is accepted or Settings open resolves. `label=view/setting/check`. No query, identifier, title, path, or setting value. A failed Settings request emits no success. The palette suppresses concurrent activation; explicit retries are new attempts. |

Each interaction emits at most one event of its type. Closing or canceling search adds no event.
These events measure navigation, not execution, time spent, successful data loading, or changed
settings. Test user paths, rejected navigation, boundary no-ops, and strict Rust
property validation. Queries and session data never enter the analytics payload.

## First-run Overview funnel (2026-10-02)

The onboarding window is gone. The main window's Overview now runs the first
run on a new install: the pitch line, the Agents→Sessions→Checks steps, and the fixes
result. `antiburn.onboarding_started`, `antiburn.onboarding_step_viewed`, and
`antiburn.onboarding_finished` are retired. Earlier app versions still send
them, so a funnel across versions must keep the two event families apart.

Question: where does a new install's first run lose readers — never starting,
or dropping between Agents, Read, Check, and the result — and which result
(empty, checks disabled, not assessed, clean, fixes found) do readers who finish actually land on? Metric:
reporting installations reaching each `antiburn.first_run_step_reached` label,
as a funnel with `started` at the base, divided by reporting installations
overall in the same app-version cohort. This decides whether the steps block's
pacing or copy needs work before the result shows; it does not measure time
spent on any step, or distinguish a slow pass from a fast one.

Each step's own work now waits for the reader's Next or Show/Skip press
before it starts (the backend first-run gate: discovery waits for the welcome
step's Next, reading waits for the live limits step that follows Agents, and
checks wait for Sessions' Next). The elapsed time between two `first_run_step_reached` events therefore
includes however long the reader spent reading that step's card, not only
the work's own running time. A funnel built from this event was always a
step-reached count, never a timing metric, so this does not change what the
metric answers — it only means a time-between-steps figure computed from
raw event timestamps is reading time plus work time, not work time alone.

`overviewProgressStore.ts` owns the trigger. One flag per step
(`started`/`found`/`read`/`checked`), set the first time that step's own
latched `done` flag turns true while the store is in `firstRun` mode;
`steady`-mode installations — a device that already finished a first run —
never enter this mode and never report any of it. `result` fires once when the
Fixes step first opens. Done or Enhance then commits `onboardingCompleted`
through `finish_first_run`; a failed command keeps the step open for a retry. The shell's `finish_first_run` records
`antiburn.first_run_finished` itself, only when its save turns
`onboardingCompleted` from false to true, so a failed save reports no
finish. A wipe
(`ftue:reset`) clears every flag, because it starts a genuinely new first run
that must report its own funnel.

Properties: `label` is `started`, `found`, `read`, `checked`, or `result` —
the step. `bucket` is the discovery pass's session count, bucketed, present
only with `found`. `detail` is `empty`, `checks_disabled`, `not_assessed`, `clean`,
or `fixes_found`, present only with `result`. `empty` means the window has no
sessions. `checks_disabled` means sessions exist but no checks are enabled.
A confirmed finding produces `fixes_found`, even with passes or other non-results.
Otherwise, `clean` requires at least one confirmed passing check with a positive
clean count. A passing lifecycle alone is insufficient. Waiting, errors, and
assessments without a status write alone produce `not_assessed`.
A mixed report with a confirmed pass and waiting, errored, or unassessed checks
can report `clean` when it has no findings. This means the assessed subset has
no findings; it does not establish complete check or session coverage.
Exclude `checks_disabled` and `not_assessed` from the Clean numerator. Keep all
result categories in the result-step denominator to expose missing assessment
reach. This reuses the result event and adds no event or wire property. It keeps
the one-result-per-first-run bound; later refreshes do not revise the event.
`antiburn.first_run_action` currently reports
four closed labels — `folder_access_requested`, `folder_access_granted`,
`live_usage_started`, and `live_usage_skipped` — only after the corresponding
action fires. The final completion card has one Get Started button. A successful
finish emits `first_run_finished`; a failed finish keeps the card open for retry.
The schema retains `enhance_opened` and `include_non_repo_folders` for
compatibility, but the current onboarding flow emits neither. The Sessions
setting still emits `setting_toggled`.
No folder path, session identity, finding
detail, or account identity reaches any of these three events. The Rust
boundary rejects an unlisted step, action, or result, and rejects any extra
field on any of the three shapes, the same way every other closed
`Interaction` variant does.

This is new instrumentation, not a reused name, so `docs/analytics.md` lists
all three events fresh and marks the two retired ones legacy — mirroring the
existing `antiburn.session_filter_selected` precedent — rather than deleting
their rows. Segment reports at the first app version that ships this change;
earlier versions have no `first_run_step_reached`/`first_run_action`/
`first_run_finished` events to compare against, and a `first_run` funnel
cannot be joined against the retired step-viewed event's four-step vocabulary,
which had no `found`/`checked` distinction and no result step at all.
Segment `checks_disabled` from the first app version that adds per-check
enablement; earlier versions cannot produce that detail.
Segment `not_assessed` and the confirmed-pass requirement at their first shipping
app version. Earlier `clean` results can include all-unassessed reports and cannot
be reclassified from the coarse event alone.

Tests: `event.rs` proves each step's facts are scoped to that step alone
(`first_run_step_reached_carries_only_the_facts_its_own_step_defines`), the
action vocabulary accepts its six labels, including the retained compatibility value
(`first_run_action_uses_closed_vocabulary`), the finished event carries no
properties (`first_run_finished_carries_no_properties`), and the Rust boundary
rejects an unknown step, action, or result and an extra field on any of the
three shapes (`first_run_interactions_are_refused_at_the_boundary_for_unknown_values`).
`overviewProgressStore.live.test.ts` drives the real store through a full
first-run pass and asserts each step fires exactly once, in order, with the
right `sessions`/`result` payload, that `steady` mode reports nothing, and
that a failed finish can be retried without reporting a successful Enhance.
The live-store tests also cover the live-usage command and refresh order.
The removed onboarding window has no renderer that can emit the retired
step-viewed event.

## Step settings exposure (2026-10-07)

The Sources pane is gone. Its rows, and the former Checks pane rows, are in
the Agents, Sessions, and Checks steps. Each step's settings open in two
places: the first-run takeover's collapsed "Show settings" disclosure, and
the Agents, Sessions, and Checks Settings panes. After the first run, the
shortcut pills in the main-window sidebar open those Settings panes. Search
opens the Settings panes too.

Question: do readers who are not in the first run still open a step's own
settings, and does the first-run "Show settings" disclosure get used at all?
Metric: reporting installations with a user-initiated
`antiburn.settings_pane_viewed` event for `agents`, `sessions`, or `checks`,
and with `antiburn.step_settings_viewed` (`detail: "first_run"`), split by
`label`. The `checks` pane exposure also supplies the Checks share of "Are
people deliberately returning?" above.

`StepSettingsDisclosure` reports `detail: "first_run"` each time a reader
opens it (collapsed by default, so a mount alone reports nothing; closing it
and opening it again is a new deliberate exposure). Re-renders of an
already-open disclosure report nothing. `label` is the step (`agents`,
`limits`, `sessions`, or `checks`). The Settings panes report through
`SettingsWindowSession`, like every other pane. Search results for these
controls report `app_search_result_opened` with `setting`. No control
identity, setting value, or search query reaches either event. The Rust
boundary rejects an unlisted label or detail, and any extra field (`step_settings_viewed_uses_closed_vocabulary`).

### Agents summary exposure

The main-window Agents page uses the existing `surface_viewed` and
`surface_state_observed` events with the closed `agents` surface label. It reports
only while the page is selected and the main window is visible. A discovery row
with sessions is `ready`; completed discovery without sessions is `empty`.
Unsettled discovery can report `loading_timeout` after ten seconds. Leaving the
page ends the exposure. These events contain no agent names, counts, or paths and
do not establish that a session link was followed.
